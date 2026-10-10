//! The placeholder effect sprites, drawn procedurally at startup.
//!
//! A radial-gradient puff and a flash; drawing them in code rather than shipping PNGs keeps `assets/` free of binaries and
//! lets the frame count change with a constant. The original's own sprite
//! cycles (`effects.zrd`, 13 of them, textures in the texture archives) are
//! the real thing wherever the data names one; these are the fallback.

use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

/// Side of every sprite, pixels.
pub const SPRITE_SIZE: u32 = 64;

/// Frames in the puff animation.
pub const PUFF_FRAMES: usize = 8;

/// Frames in the flash animation.
pub const FLASH_FRAMES: usize = 4;

/// Frames in the flame loop.
pub const FLAME_FRAMES: usize = 6;

/// A white puff that grows and hollows out over `frames` frames; the effect
/// tints it. Premultiplied-looking falloff so it reads under additive and
/// alpha blending alike.
pub fn puff(frame: usize, frames: usize) -> Image {
    let t = frame as f32 / (frames.max(2) - 1) as f32;
    let radius = 0.55 + 0.45 * t;
    // Later frames thin in the middle, the way smoke rings open.
    let hollow = 0.6 * t;

    image(|x, y| {
        let r = (x * x + y * y).sqrt() / radius;
        if r >= 1.0 {
            return 0.0;
        }
        let body = (1.0 - r).powf(1.5);
        let ring = 1.0 - hollow * (1.0 - r).powi(4);
        // A little lumpiness so it is not a perfect disc.
        let angle = y.atan2(x);
        let lumps = 0.85 + 0.15 * (angle * 5.0 + t * 3.0).sin();
        (body * ring * lumps).clamp(0.0, 1.0)
    })
}

/// A four-spiked flash, brightest in its first frame.
pub fn flash(frame: usize, frames: usize) -> Image {
    let t = frame as f32 / (frames.max(2) - 1) as f32;
    let strength = 1.0 - 0.7 * t;

    image(|x, y| {
        let r = (x * x + y * y).sqrt();
        let core = (1.0 - r / 0.35).max(0.0);
        let spikes = (1.0 - (x.abs() * y.abs()).sqrt() * 6.0).max(0.0) * (1.0 - r).max(0.0);
        ((core + spikes) * strength).clamp(0.0, 1.0)
    })
}

/// A flame tongue: wide at the bottom, tapering and flickering upward.
pub fn flame(frame: usize, frames: usize) -> Image {
    let phase = frame as f32 / frames.max(1) as f32 * std::f32::consts::TAU;

    image(|x, y| {
        // y runs -1 (bottom) to 1 (top) in image space flipped below.
        let up = -y;
        let height = (up + 1.0) * 0.5;
        let sway = 0.12 * (height * 6.0 + phase).sin() * height;
        let width = 0.7 * (1.0 - height).max(0.0).powf(0.7);
        if width <= 0.0 {
            return 0.0;
        }
        let across = ((x - sway) / width).abs();
        ((1.0 - across).max(0.0) * (1.0 - height * 0.8)).clamp(0.0, 1.0)
    })
}

/// A white RGBA image whose alpha is `alpha(x, y)` over x, y in -1..1.
fn image(alpha: impl Fn(f32, f32) -> f32) -> Image {
    let size = SPRITE_SIZE as usize;
    let mut data = Vec::with_capacity(size * size * 4);
    for row in 0..size {
        for column in 0..size {
            let x = (column as f32 + 0.5) / size as f32 * 2.0 - 1.0;
            let y = (row as f32 + 0.5) / size as f32 * 2.0 - 1.0;
            let a = (alpha(x, y) * 255.0).round() as u8;
            data.extend_from_slice(&[255, 255, 255, a]);
        }
    }

    let mut image = Image::new(
        Extent3d {
            width: SPRITE_SIZE,
            height: SPRITE_SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(image: &Image, x: u32, y: u32) -> u8 {
        let data = image.data.as_ref().expect("CPU data");
        data[((y * SPRITE_SIZE + x) * 4 + 3) as usize]
    }

    #[test]
    fn a_puff_is_opaque_in_the_middle_and_clear_at_the_corners() {
        let first = puff(0, PUFF_FRAMES);
        let centre = SPRITE_SIZE / 2;
        assert!(alpha_at(&first, centre, centre) > 150);
        assert_eq!(alpha_at(&first, 0, 0), 0);
    }

    #[test]
    fn a_puff_grows_over_its_frames() {
        let x = SPRITE_SIZE / 2 + SPRITE_SIZE * 3 / 10;
        let y = SPRITE_SIZE / 2;
        let first = alpha_at(&puff(0, PUFF_FRAMES), x, y);
        let last = alpha_at(&puff(PUFF_FRAMES - 1, PUFF_FRAMES), x, y);
        assert!(last > first, "{first} → {last}");
    }

    #[test]
    fn a_flash_fades_over_its_frames() {
        let centre = SPRITE_SIZE / 2;
        let first = alpha_at(&flash(0, FLASH_FRAMES), centre, centre);
        let last = alpha_at(&flash(FLASH_FRAMES - 1, FLASH_FRAMES), centre, centre);
        assert!(first > last);
    }
}
