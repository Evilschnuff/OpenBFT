//! Where the original HUD goes on a screen that is not 640×480.
//!
//! Pure geometry, no Bevy systems: [`HudFrame`] maps a rectangle on the
//! art's 640×480 canvas to logical screen pixels, and [`strip_pieces`] cuts
//! a strip image into the pieces that stretch it across a wider screen.
//!
//! # The rules
//!
//! - **Scale by height**: `s = screen height / canvas height × hud_scale`,
//!   so the HUD keeps the share of the screen it had on a CRT. On a screen
//!   narrower than the canvas's aspect, or with a `hud_scale` that would
//!   make the canvas wider than the screen, the width decides instead, so
//!   clusters never overlap.
//! - **Anchor clusters, don't stretch the canvas.** Each element keeps its
//!   canvas offsets relative to an anchor picked from where its centre sits:
//!   the left, middle or right third of the width, the top or bottom half
//!   of the height. Nothing in the layout file says which anchor; the thirds
//!   are what the original's own wide art shows (below).
//! - **Strips span the width.** The original shipped `tops800`/`tops1024`
//!   and `iback800`/`iback1024` for 800- and 1024-wide screens. Measured
//!   against the 640 versions, each is the same art with the extra width
//!   inserted as plain metal *at the anchor seams*: the top strip's middle
//!   slot moves right by half the extra and its right slot by all of it
//!   (thirds anchoring, exactly), and the icon strip's panel stays centred
//!   with the extra split between its two ends. [`strip_pieces`] does the
//!   same for any width, taking the inserted metal from whichever variant's
//!   insert is closest in width to the gap and stretching it to fit.

use bevy::math::{Rect, Vec2};

/// Which third of the canvas width an element is anchored to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    Left,
    Centre,
    Right,
}

/// Which half of the canvas height an element is anchored to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Row {
    Top,
    Bottom,
}

/// A column and a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Anchor {
    pub column: Column,
    pub row: Row,
}

/// The screen the HUD is laid out on, and the scale it is drawn at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HudFrame {
    /// The window, in logical pixels.
    pub screen: Vec2,
    /// The art's canvas: `hud.zrd`'s full-screen `TYPEI` rectangle.
    pub canvas: Vec2,
    /// Logical pixels per canvas pixel.
    pub scale: f32,
}

impl HudFrame {
    /// Lay out a canvas on a screen at a user scale.
    pub fn new(screen: Vec2, canvas: Vec2, hud_scale: f32) -> HudFrame {
        let canvas = canvas.max(Vec2::ONE);
        let by_height = screen.y / canvas.y * hud_scale.max(0.01);
        let by_width = screen.x / canvas.x;
        HudFrame {
            screen,
            canvas,
            scale: by_height.min(by_width).max(0.01),
        }
    }

    /// The anchor for something covering `rect` on the canvas.
    pub fn anchor_of(&self, rect: Rect) -> Anchor {
        let centre = rect.center();
        let third = self.canvas.x / 3.0;
        let column = if centre.x < third {
            Column::Left
        } else if centre.x < 2.0 * third {
            Column::Centre
        } else {
            Column::Right
        };
        let row = if centre.y < self.canvas.y / 2.0 {
            Row::Top
        } else {
            Row::Bottom
        };
        Anchor { column, row }
    }

    /// A canvas x on screen, anchored to a column.
    pub fn x(&self, x: f32, column: Column) -> f32 {
        let s = self.scale;
        match column {
            Column::Left => x * s,
            Column::Centre => self.screen.x / 2.0 + (x - self.canvas.x / 2.0) * s,
            Column::Right => self.screen.x - (self.canvas.x - x) * s,
        }
    }

    /// A canvas y on screen, anchored to a row.
    pub fn y(&self, y: f32, row: Row) -> f32 {
        match row {
            Row::Top => y * self.scale,
            Row::Bottom => self.screen.y - (self.canvas.y - y) * self.scale,
        }
    }

    /// A canvas rectangle on screen, anchored as given.
    pub fn place_as(&self, rect: Rect, anchor: Anchor) -> Rect {
        let min = Vec2::new(
            self.x(rect.min.x, anchor.column),
            self.y(rect.min.y, anchor.row),
        );
        Rect::from_corners(min, min + rect.size() * self.scale)
    }

    /// A canvas rectangle on screen, anchored by where it sits.
    pub fn place(&self, rect: Rect) -> Rect {
        self.place_as(rect, self.anchor_of(rect))
    }

    /// How much wider than the canvas the screen is on each side, in canvas
    /// pixels: the width each anchor seam has to absorb.
    pub fn gap(&self) -> f32 {
        ((self.screen.x / self.scale - self.canvas.x) / 2.0).max(0.0)
    }

    /// Whether the scale is a whole number, where nearest sampling keeps the
    /// art's pixels square.
    pub fn is_integer_scale(&self) -> bool {
        (self.scale - self.scale.round()).abs() < 0.01
    }
}

/// A strip image, or one of its wide variants.
#[derive(Debug, Clone, PartialEq)]
pub struct StripArt {
    pub texture: String,
    /// Its size in texels.
    pub size: Vec2,
}

/// One piece of a strip: which part of which texture, and where on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct StripPiece {
    pub texture: String,
    /// In texels of `texture`.
    pub source: Rect,
    /// In logical pixels.
    pub screen: Rect,
}

/// A run of a strip's base texture, `(start, end, column)` in texels.
type Segment = (f32, f32, Column);
/// Where art is inserted, `(x, column left of it, column right of it)`.
type Seam = (f32, Column, Column);

/// Cut a strip into the pieces that draw it on this screen.
///
/// `base` sits at `pos` on the canvas. A strip spanning the whole canvas
/// width is split at the thirds, each piece anchored to its own third; any
/// other strip keeps one anchor, and when that is the middle its two ends
/// are extended to the side clusters. Either way there are two seams, and
/// each gets [`HudFrame::gap`] of inserted art, cut from the variant that
/// inserts the most similar width (see the module doc). `variants` may hold
/// any textures: only those as tall as `base` and wider count.
///
/// A strip with no usable variant, or a screen with no gap, draws `base`
/// whole in its pieces; a gap with no variant to fill it is stretched from
/// the column of `base` at the seam.
pub fn strip_pieces(
    frame: &HudFrame,
    base: &StripArt,
    pos: Vec2,
    variants: &[StripArt],
) -> Vec<StripPiece> {
    let width = base.size.x;
    let height = base.size.y;
    let canvas_rect = Rect::from_corners(pos, pos + base.size);
    let anchor = frame.anchor_of(canvas_rect);
    let full_width = pos.x <= 0.0 && width >= frame.canvas.x;

    // Segments of the base as (start, end, column), and the base x of each
    // seam with the columns either side of it.
    let (segments, seams): (Vec<Segment>, Vec<Seam>) = if full_width {
        let first = (frame.canvas.x / 3.0).round() - pos.x;
        let second = (2.0 * frame.canvas.x / 3.0).round() - pos.x;
        (
            vec![
                (0.0, first, Column::Left),
                (first, second, Column::Centre),
                (second, width, Column::Right),
            ],
            vec![
                (first, Column::Left, Column::Centre),
                (second, Column::Centre, Column::Right),
            ],
        )
    } else if anchor.column == Column::Centre {
        (
            vec![(0.0, width, Column::Centre)],
            vec![
                (0.0, Column::Left, Column::Centre),
                (width, Column::Centre, Column::Right),
            ],
        )
    } else {
        (vec![(0.0, width, anchor.column)], Vec::new())
    };

    let gap = frame.gap();
    let wanted = !seams.is_empty() && gap >= 0.5;

    // The variant whose per-seam insert is closest to the gap, by ratio.
    let chosen = if wanted {
        variants
            .iter()
            .filter(|variant| (variant.size.y - height).abs() < 0.5 && variant.size.x > width + 0.5)
            .map(|variant| {
                let insert = (variant.size.x - width) / seams.len() as f32;
                (variant, insert)
            })
            .min_by(|(_, a), (_, b)| {
                let cost = |insert: f32| (gap / insert).ln().abs();
                cost(*a).total_cmp(&cost(*b))
            })
    } else {
        None
    };
    let (art, insert) = chosen.unwrap_or((base, 0.0));

    let y = |row| {
        let top = frame.y(pos.y, row);
        (top, top + height * frame.scale)
    };
    let (top, bottom) = y(anchor.row);

    let mut pieces = Vec::new();
    let mut inserted = 0.0;
    let mut seam_index = 0;
    let push_seam =
        |pieces: &mut Vec<StripPiece>, inserted: &mut f32, seam: (f32, Column, Column)| {
            let (at, left, right) = seam;
            let x0 = frame.x(pos.x + at, left);
            let x1 = frame.x(pos.x + at, right);
            if wanted && x1 - x0 > 0.0 {
                let source = if insert > 0.0 {
                    Rect::new(at + *inserted, 0.0, at + *inserted + insert, height)
                } else {
                    // No variant: stretch the one column at the seam.
                    let column = at.clamp(0.0, (width - 1.0).max(0.0));
                    Rect::new(column, 0.0, column + 1.0, height)
                };
                pieces.push(StripPiece {
                    texture: art.texture.clone(),
                    source,
                    screen: Rect::new(x0, top, x1, bottom),
                });
            }
            *inserted += insert;
        };

    for (start, end, column) in segments {
        while seam_index < seams.len() && seams[seam_index].0 <= start {
            push_seam(&mut pieces, &mut inserted, seams[seam_index]);
            seam_index += 1;
        }
        if end > start {
            pieces.push(StripPiece {
                texture: art.texture.clone(),
                source: Rect::new(start + inserted, 0.0, end + inserted, height),
                screen: Rect::new(
                    frame.x(pos.x + start, column),
                    top,
                    frame.x(pos.x + end, column),
                    bottom,
                ),
            });
        }
    }
    while seam_index < seams.len() {
        push_seam(&mut pieces, &mut inserted, seams[seam_index]);
        seam_index += 1;
    }

    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    // A synthetic canvas and layout. Every number here is invented for the
    // test; the real layout is checked in `tests/real_ui.rs`.
    const CANVAS: Vec2 = Vec2::new(600.0, 450.0);

    fn frame(width: f32, height: f32) -> HudFrame {
        HudFrame::new(Vec2::new(width, height), CANVAS, 1.0)
    }

    /// The five form factors the layout is checked at.
    const SCREENS: [(f32, f32); 5] = [
        (600.0, 450.0),
        (1920.0, 1080.0),
        (3440.0, 1440.0),
        (3840.0, 2160.0),
        (1280.0, 800.0),
    ];

    /// Invented clusters: a monitor bottom left, an icon strip in the
    /// middle, a health gauge and a form grid bottom right, a score and a
    /// clock along the top.
    fn clusters() -> Vec<Rect> {
        vec![
            Rect::new(0.0, 330.0, 130.0, 450.0),
            Rect::new(130.0, 370.0, 480.0, 450.0),
            Rect::new(480.0, 370.0, 520.0, 450.0),
            Rect::new(520.0, 370.0, 600.0, 450.0),
            Rect::new(280.0, 4.0, 320.0, 16.0),
            Rect::new(510.0, 4.0, 560.0, 16.0),
        ]
    }

    #[test]
    fn the_hud_keeps_its_share_of_the_height_and_its_clusters_apart() {
        let sensor_view = Rect::new(10.0, 338.0, 110.0, 438.0);
        for (width, height) in SCREENS {
            let frame = frame(width, height);
            let placed: Vec<Rect> = clusters().into_iter().map(|r| frame.place(r)).collect();

            for (i, a) in placed.iter().enumerate() {
                assert!(
                    a.min.x >= -0.01 && a.max.x <= width + 0.01,
                    "{width}x{height}: cluster {i} leaves the screen: {a:?}"
                );
                for (j, b) in placed.iter().enumerate().skip(i + 1) {
                    let overlap = a.intersect(*b);
                    assert!(
                        overlap.width() <= 0.01 || overlap.height() <= 0.01,
                        "{width}x{height}: clusters {i} and {j} overlap"
                    );
                }
            }

            let sensor = frame.place(sensor_view);
            assert!(
                sensor.height() >= 0.2 * height,
                "{width}x{height}: the sensor is {} high",
                sensor.height()
            );
        }
    }

    #[test]
    fn clusters_sit_in_their_corners_at_any_width() {
        let wide = frame(3440.0, 1440.0);
        let monitor = wide.place(clusters()[0]);
        let forms = wide.place(clusters()[3]);
        let icons = wide.place(clusters()[1]);

        assert_eq!(monitor.min.x, 0.0);
        assert_eq!(monitor.max.y, 1440.0);
        assert!((forms.max.x - 3440.0).abs() < 0.01);
        assert!((icons.center().x - (3440.0 / 2.0 + 5.0 * wide.scale)).abs() < 0.01);
        assert!((wide.scale - 1440.0 / 450.0).abs() < 1e-5);
    }

    #[test]
    fn a_narrow_screen_or_a_big_hud_scales_by_width() {
        let portrait = frame(450.0, 800.0);
        assert!((portrait.scale - 0.75).abs() < 1e-6);

        let big = HudFrame::new(Vec2::new(600.0, 450.0), CANVAS, 1.5);
        assert!((big.scale - 1.0).abs() < 1e-6, "clamped to the width");

        let small = HudFrame::new(Vec2::new(1200.0, 900.0), CANVAS, 0.5);
        assert!((small.scale - 1.0).abs() < 1e-6);
        assert!(small.is_integer_scale());
        assert!(!frame(1920.0, 1080.0).is_integer_scale());
    }

    fn art(name: &str, width: f32, height: f32) -> StripArt {
        StripArt {
            texture: name.to_string(),
            size: Vec2::new(width, height),
        }
    }

    #[test]
    fn at_the_canvas_size_a_strip_is_drawn_whole() {
        let frame = frame(600.0, 450.0);
        let pieces = strip_pieces(&frame, &art("top", 600.0, 20.0), Vec2::ZERO, &[]);

        assert_eq!(pieces.len(), 3, "the three thirds, no gaps");
        let covered: f32 = pieces.iter().map(|piece| piece.source.width()).sum();
        assert_eq!(covered, 600.0);
        assert_eq!(pieces[2].screen.max.x, 600.0);
    }

    #[test]
    fn a_full_width_strip_takes_its_seams_from_the_closest_variant() {
        // 16:9 at 450 lines: 800 wide, so 100 canvas pixels per seam.
        let frame = frame(800.0, 450.0);
        let variants = [
            art("top_a", 760.0, 20.0),
            art("top_b", 1000.0, 20.0),
            art("tall", 900.0, 40.0),
        ];
        let pieces = strip_pieces(&frame, &art("top", 600.0, 20.0), Vec2::ZERO, &variants);

        // top_a inserts 80 per seam, top_b 200: 80 is closer to 100.
        assert!(pieces.iter().all(|piece| piece.texture == "top_a"));
        assert_eq!(pieces.len(), 5);
        let sources: Vec<(f32, f32)> = pieces
            .iter()
            .map(|p| (p.source.min.x, p.source.max.x))
            .collect();
        assert_eq!(
            sources,
            vec![
                (0.0, 200.0),
                (200.0, 280.0),
                (280.0, 480.0),
                (480.0, 560.0),
                (560.0, 760.0)
            ]
        );

        // Contiguous across the screen, edge to edge.
        assert_eq!(pieces[0].screen.min.x, 0.0);
        for pair in pieces.windows(2) {
            assert!((pair[0].screen.max.x - pair[1].screen.min.x).abs() < 1e-3);
        }
        assert!((pieces[4].screen.max.x - 800.0).abs() < 1e-3);
    }

    #[test]
    fn a_middle_strip_grows_at_both_ends() {
        let frame = frame(800.0, 450.0);
        let base = art("icons", 350.0, 80.0);
        let variants = [art("icons_wide", 510.0, 80.0)];
        let pieces = strip_pieces(&frame, &base, Vec2::new(130.0, 370.0), &variants);

        assert_eq!(pieces.len(), 3);
        assert_eq!(pieces[0].source, Rect::new(0.0, 0.0, 80.0, 80.0));
        assert_eq!(pieces[1].source, Rect::new(80.0, 0.0, 430.0, 80.0));
        assert_eq!(pieces[2].source, Rect::new(430.0, 0.0, 510.0, 80.0));
        // It meets the left cluster's edge and the right cluster's.
        assert!((pieces[0].screen.min.x - 130.0).abs() < 1e-3);
        assert!((pieces[2].screen.max.x - (800.0 - 120.0)).abs() < 1e-3);
        assert_eq!(pieces[1].screen.width(), 350.0);
    }

    #[test]
    fn a_seam_with_no_variant_stretches_the_base() {
        let frame = frame(800.0, 450.0);
        let pieces = strip_pieces(&frame, &art("top", 600.0, 20.0), Vec2::ZERO, &[]);

        assert_eq!(pieces.len(), 5);
        assert_eq!(pieces[1].source.width(), 1.0);
        assert!((pieces[1].screen.width() - 100.0).abs() < 1e-3);
    }

    #[test]
    fn a_corner_strip_is_left_alone() {
        let frame = frame(1920.0, 1080.0);
        let pieces = strip_pieces(
            &frame,
            &art("corner", 130.0, 30.0),
            Vec2::new(0.0, 330.0),
            &[art("corner_wide", 300.0, 30.0)],
        );
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].texture, "corner");
        assert_eq!(pieces[0].screen.min.x, 0.0);
    }
}
