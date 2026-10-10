//! Where a 640×480 dialog goes on a screen that is not 640×480.
//!
//! Pure geometry, like the HUD's [`layout`](crate::ui::hud::layout), and
//! the same first rule: **scale by height**, so a dialog keeps the share of
//! the screen it had on a CRT; on a screen narrower than 4:3 the width
//! decides instead, so nothing is cut off.
//!
//! **Anchored left, centred vertically.** Every dialog hangs off the panel
//! strip at its left edge — pipes and bolts run from the strip to each
//! button — so the design moves as one piece, and the strip is where the
//! screen's edge is. On a wide screen the extra width is on the right; the
//! splash behind the front end is anchored the same way, so the two line
//! up as they did at 640×480. On a screen taller
//! than the canvas (narrower than 4:3), the canvas is centred and the
//! strips, which are 768 lines drawn at `y = −144`, fill above and below
//! instead of being cut: that is what the tall art is for.

use bevy::math::{Rect, Vec2};

/// The screen the dialogs were drawn for. `dialog.zrd` does not say; the
/// strips do: 768 lines placed 144 above the top leave 480 between.
pub const DIALOG_CANVAS: Vec2 = Vec2::new(640.0, 480.0);

/// The screen a dialog is laid out on, and where its canvas lands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DialogFrame {
    /// The window, in logical pixels.
    pub screen: Vec2,
    /// Logical pixels per canvas pixel.
    pub scale: f32,
    /// The canvas's top-left, on screen.
    pub origin: Vec2,
}

impl DialogFrame {
    /// Lay the canvas out on a screen.
    pub fn new(screen: Vec2) -> DialogFrame {
        let by_height = screen.y / DIALOG_CANVAS.y;
        let by_width = screen.x / DIALOG_CANVAS.x;
        let scale = by_height.min(by_width).max(0.01);
        DialogFrame {
            screen,
            scale,
            origin: Vec2::new(0.0, ((screen.y - DIALOG_CANVAS.y * scale) / 2.0).max(0.0)),
        }
    }

    /// The canvas's rectangle on screen.
    pub fn canvas(&self) -> Rect {
        Rect::from_corners(self.origin, self.origin + DIALOG_CANVAS * self.scale)
    }

    /// A canvas rectangle on screen.
    pub fn place(&self, rect: Rect) -> Rect {
        Rect::from_corners(
            self.origin + rect.min * self.scale,
            self.origin + rect.max * self.scale,
        )
    }

    /// Whether the scale is a whole number, where nearest sampling keeps
    /// the art's pixels square.
    pub fn is_integer_scale(&self) -> bool {
        (self.scale - self.scale.round()).abs() < 1e-3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREENS: [(f32, f32); 6] = [
        (640.0, 480.0),
        (1920.0, 1080.0),
        (3440.0, 1440.0),
        (3840.0, 2160.0),
        (1280.0, 800.0),
        (800.0, 1280.0),
    ];

    /// On every form factor the whole canvas is on screen, against the left
    /// edge, as big as it can be.
    #[test]
    fn the_canvas_fits_every_screen_against_the_left_edge() {
        for (width, height) in SCREENS {
            let screen = Vec2::new(width, height);
            let frame = DialogFrame::new(screen);
            let canvas = frame.canvas();

            assert_eq!(canvas.min.x, 0.0, "{screen}: left edge");
            assert!(canvas.max.x <= width + 0.01, "{screen}: fits across");
            assert!(canvas.min.y >= 0.0 && canvas.max.y <= height + 0.01);
            // Either the height or the width is used up.
            let full =
                (canvas.height() - height).abs() < 0.01 || (canvas.width() - width).abs() < 0.01;
            assert!(full, "{screen}: {canvas:?}");
        }
    }

    /// Scaling by height keeps a dialog's share of the screen: at 1080p a
    /// button is 2.25 times its art, at 4K 4.5 times, and on 21:9 the same
    /// as on 16:9 of that height.
    #[test]
    fn dialogs_scale_by_height() {
        assert_eq!(DialogFrame::new(Vec2::new(640.0, 480.0)).scale, 1.0);
        assert_eq!(DialogFrame::new(Vec2::new(1920.0, 1080.0)).scale, 2.25);
        assert_eq!(DialogFrame::new(Vec2::new(3840.0, 2160.0)).scale, 4.5);
        assert_eq!(
            DialogFrame::new(Vec2::new(3440.0, 1440.0)).scale,
            DialogFrame::new(Vec2::new(2560.0, 1440.0)).scale
        );
        assert!(DialogFrame::new(Vec2::new(1280.0, 960.0)).is_integer_scale());
        assert!(!DialogFrame::new(Vec2::new(1920.0, 1080.0)).is_integer_scale());
    }

    /// On a portrait screen the canvas is centred and a strip drawn from
    /// 144 lines above it reaches toward the top of the screen.
    #[test]
    fn a_tall_screen_centres_the_canvas_and_the_strip_fills_above() {
        let frame = DialogFrame::new(Vec2::new(800.0, 1280.0));
        assert_eq!(frame.scale, 1.25);
        assert_eq!(frame.origin.y, (1280.0 - 600.0) / 2.0);

        let strip = frame.place(Rect::new(0.0, -144.0, 84.0, 624.0));
        assert!(strip.min.y < frame.canvas().min.y);
        assert!(strip.max.y > frame.canvas().max.y);
    }
}
