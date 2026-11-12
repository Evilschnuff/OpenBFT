//! A looped sound whose playback rate follows the game sample by sample.
//!
//! The engines are `sounds.zrd` records flagged `FREQUENCY`: the original
//! varies their playback rate as it runs (docs/data/sounds.md), and that
//! change is heard as smooth.
//!
//! rodio's own speed control cannot do that for a loop. It changes the rate
//! a source *reports*, and the mixer's resampler only reads that rate again
//! when it starts a new span of samples, which for a looping WAV comes a
//! tenth of a second to more than half a second apart. Each new rate
//! therefore lands as a jump. So the rate is applied here instead: a
//! [`VariLoop`] steps through its samples at whatever rate its
//! [`RateControl`] holds, interpolating between them, and reports a rate
//! that never changes, so the resampler has nothing to catch up on.
//!
//! The game sets the rate once a rendered frame. The source glides to each
//! new rate over [`GLIDE_SECONDS`] so those 60-a-second updates do not step
//! either. That glide is ours, not the original's: it only hides how often we
//! update.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use bevy::audio::{AudioSource, ChannelCount, Decodable, Sample, SampleRate, Source};
use bevy::prelude::*;

/// How long the source takes to reach most (1 − 1/e) of a new rate, seconds.
pub const GLIDE_SECONDS: f64 = 0.02;

/// A WAV decoded once into samples, shared by every loop that plays it.
#[derive(Debug)]
pub struct LoopSamples {
    /// Interleaved by channel.
    samples: Box<[Sample]>,
    channels: ChannelCount,
    sample_rate: SampleRate,
    /// The largest sample, as a fraction of full scale.
    peak: f32,
}

impl LoopSamples {
    /// Decode a sound with the same decoder Bevy plays it with. `None` for
    /// one with no samples.
    pub fn decode(source: &AudioSource) -> Option<LoopSamples> {
        let decoder = source.decoder();
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        let samples: Box<[Sample]> = decoder.collect();
        let frames = samples.len() / channels.get() as usize;
        (frames > 0).then(|| LoopSamples {
            peak: peak_of(&samples),
            samples,
            channels,
            sample_rate,
        })
    }

    /// Samples straight from memory, for tests.
    pub fn from_samples(samples: Vec<Sample>, channels: u16, sample_rate: u32) -> LoopSamples {
        LoopSamples {
            peak: peak_of(&samples),
            samples: samples.into(),
            channels: ChannelCount::new(channels).expect("at least one channel"),
            sample_rate: SampleRate::new(sample_rate).expect("a positive rate"),
        }
    }

    fn frames(&self) -> usize {
        self.samples.len() / self.channels.get() as usize
    }

    /// The largest sample, as a fraction of full scale: what a gain has to
    /// stay under for this sound not to clip. Interpolating between samples
    /// never exceeds it.
    pub fn peak(&self) -> f32 {
        self.peak
    }
}

fn peak_of(samples: &[Sample]) -> f32 {
    samples
        .iter()
        .fold(0.0, |peak, sample| peak.max(sample.abs()))
}

/// The playback rate of one playing loop, 1 being the WAV's own.
///
/// The game writes it and the audio thread reads it, so it is an atomic
/// rather than anything behind a lock.
#[derive(Debug, Clone)]
pub struct RateControl(Arc<AtomicU32>);

impl RateControl {
    pub fn new(rate: f32) -> RateControl {
        RateControl(Arc::new(AtomicU32::new(rate.to_bits())))
    }

    pub fn set(&self, rate: f32) {
        self.0.store(rate.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// Two controls are equal when they steer the same loop.
impl PartialEq for RateControl {
    fn eq(&self, other: &RateControl) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// One playing loop: its samples and the rate that steers it.
///
/// An asset because that is what Bevy plays; one per playing loop, since
/// each has a rate of its own. The samples are shared.
#[derive(Asset, TypePath, Debug, Clone)]
pub struct VariLoop {
    pub samples: Arc<LoopSamples>,
    pub rate: RateControl,
}

impl Decodable for VariLoop {
    type Decoder = VariLoopSource;

    fn decoder(&self) -> VariLoopSource {
        VariLoopSource::new(self.samples.clone(), self.rate.clone())
    }
}

/// The samples a [`VariLoop`] plays. Endless: it never runs out, so it is
/// played once rather than repeated, which would buffer it forever.
#[derive(Debug)]
pub struct VariLoopSource {
    samples: Arc<LoopSamples>,
    rate: RateControl,
    /// Where the next frame is read, in frames, `0..frames`.
    position: f64,
    /// Frames advanced per frame played: the rate, glided.
    step: f64,
    /// The share of the gap to a new rate closed each frame.
    glide: f64,
    /// The channel the next sample is for.
    channel: u16,
}

impl VariLoopSource {
    pub fn new(samples: Arc<LoopSamples>, rate: RateControl) -> VariLoopSource {
        let per_second = samples.sample_rate.get() as f64;
        VariLoopSource {
            // Start at the rate already asked for, rather than gliding up
            // from nothing.
            step: rate.get().max(0.0) as f64,
            glide: 1.0 - (-1.0 / (per_second * GLIDE_SECONDS)).exp(),
            samples,
            rate,
            position: 0.0,
            channel: 0,
        }
    }

    /// The rate it is playing at right now, after the glide.
    pub fn current_rate(&self) -> f64 {
        self.step
    }
}

impl Iterator for VariLoopSource {
    type Item = Sample;

    fn next(&mut self) -> Option<Sample> {
        let frames = self.samples.frames();
        let channels = self.samples.channels.get();
        let width = channels as usize;

        // Between this frame and the next, wrapping to the start: the loop
        // is seamless at any rate.
        let at = self.position as usize;
        let after = if at + 1 >= frames { 0 } else { at + 1 };
        let fraction = (self.position - at as f64) as f32;
        let channel = self.channel as usize;
        let a = self.samples.samples[at * width + channel];
        let b = self.samples.samples[after * width + channel];
        let sample = a + (b - a) * fraction;

        self.channel += 1;
        if self.channel == channels {
            self.channel = 0;
            let target = self.rate.get().max(0.0) as f64;
            self.step += (target - self.step) * self.glide;
            self.position = (self.position + self.step) % frames as f64;
        }
        Some(sample)
    }
}

impl Source for VariLoopSource {
    fn current_span_len(&self) -> Option<usize> {
        // One endless span: the format never changes.
        None
    }

    fn channels(&self) -> ChannelCount {
        self.samples.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.samples.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize) -> Arc<LoopSamples> {
        Arc::new(LoopSamples::from_samples(
            (0..frames).map(|i| i as f32).collect(),
            1,
            1000,
        ))
    }

    #[test]
    fn at_its_own_rate_it_plays_the_samples_and_loops() {
        let mut source = VariLoopSource::new(ramp(4), RateControl::new(1.0));
        let played: Vec<f32> = source.by_ref().take(10).collect();
        assert_eq!(
            played,
            vec![0.0, 1.0, 2.0, 3.0, 0.0, 1.0, 2.0, 3.0, 0.0, 1.0]
        );
    }

    #[test]
    fn at_half_rate_it_plays_between_the_samples() {
        let mut source = VariLoopSource::new(ramp(4), RateControl::new(0.5));
        let played: Vec<f32> = source.by_ref().take(6).collect();
        assert_eq!(played, vec![0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
        // The wrap interpolates from the last sample back to the first.
        let played: Vec<f32> = source.take(2).collect();
        assert_eq!(played, vec![3.0, 1.5]);
    }

    #[test]
    fn a_new_rate_is_reached_smoothly_and_quickly() {
        let rate = RateControl::new(1.0);
        let mut source = VariLoopSource::new(ramp(1000), rate.clone());
        rate.set(2.0);

        let mut previous = source.current_rate();
        // 1000 frames a second: 5 ms, then 100 ms.
        for _ in 0..5 {
            source.next();
            let now = source.current_rate();
            assert!(now > previous, "it rises every frame, not in a jump");
            assert!(now - previous < 0.1, "by small steps: {previous} → {now}");
            previous = now;
        }
        for _ in 0..95 {
            source.next();
        }
        assert!((source.current_rate() - 2.0).abs() < 0.01);
    }

    #[test]
    fn channels_stay_paired_through_a_rate_change() {
        // Left counts up, right counts down.
        let samples = (0..8).flat_map(|i| [i as f32, -(i as f32)]).collect();
        let source = VariLoopSource::new(
            Arc::new(LoopSamples::from_samples(samples, 2, 1000)),
            RateControl::new(1.5),
        );
        let played: Vec<f32> = source.take(40).collect();
        for pair in played.chunks(2) {
            assert_eq!(pair[0], -pair[1]);
        }
    }

    #[test]
    fn a_wav_decodes_with_its_own_format() {
        let bytes = bft_assets::sounds::synth_wav(22050, 2205);
        let samples = LoopSamples::decode(&AudioSource {
            bytes: bytes.into(),
        })
        .expect("decodes");
        assert_eq!(samples.frames(), 2205);
        assert!(samples.peak() > 0.0 && samples.peak() <= 1.0);
        assert_eq!(samples.channels.get(), 1);
        assert_eq!(samples.sample_rate.get(), 22050);
    }
}
