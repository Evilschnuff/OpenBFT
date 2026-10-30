//! The cutaway: the level drawn as one viewpoint in it sees it, for a view
//! from outside, the way an isometric game cuts away the roofs and walls
//! between the camera and the player.
//!
//! Every model's material is a [`ModelMaterial`]: Bevy's `StandardMaterial`
//! with a step in front of its fragment shader. While the [`Cutaway`]
//! resource is enabled, that step judges every surface against a
//! [`Sight`], what a viewpoint (a tank's turret) sees all round it, and an
//! [`Explored`] map of the ground it has seen before:
//!
//! - a surface the viewpoint **sees** (it faces the viewpoint, or is turned
//!   only a little away, and nothing nearer in its direction hides it) is
//!   drawn as it is, fading toward unseen as it turns away or goes out of
//!   sight, so the edge of what is seen is soft;
//! - one that is not seen but **hides** space the viewpoint sees from the
//!   [`eye`](Cutaway::eye) is cut: the roof over a tunnel and the wall on
//!   the eye's side of it, the far side of a hill in front of a valley.
//!   Which space it sees is kept as [`OpenColumns`], the heights it sees
//!   over each patch of ground, so the shader tries a point behind a
//!   surface with a single read, and confirms one that reads open against
//!   the sight's own depths ([`Sight::reaches`]): a column also holds the
//!   air beside a crate, but the sight stops at the crate;
//! - any other surface is drawn darkened and greyed: on ground seen before
//!   to [`explored_shade`](Cutaway::explored_shade), elsewhere to the darker
//!   [`unseen_shade`](Cutaway::unseen_shade), so the side of a crate the
//!   viewpoint cannot see is a dark side, not a window onto what is inside;
//! - anything further across the ground than the sight's
//!   [`reach`](Sight::reach) is not drawn, its edge dissolved in a fine
//!   grain fixed to the world (in the shader; [`Cutaway::cuts`] takes it
//!   halfway) rather than a hard line.
//!
//! A mesh tagged [`KEEP`] (Bevy's `MeshTag`) is always drawn as it is (the
//! viewpoint's own vehicle). One tagged [`when_seen`] is drawn as it is
//! while the viewpoint sees a point over its origin, and not at all
//! otherwise ([`Sight::sees_body`]): other vehicles, pickups and shots, so
//! memory does not give away where they are.
//!
//! It applies only to views drawn at [`min_view_scale`](Cutaway::min_view_scale)
//! or more, a camera whose transform is scaled up as when a level is shown
//! as a model; ordinary cameras at scale 1 keep the whole level. Disabled
//! (the default), nothing is cut and the material draws as the
//! `StandardMaterial` it extends. Nothing in the game enables it: it is a
//! hook for whoever presents the level differently. The same judgement is
//! [`Cutaway::cuts`] on the CPU, for a pointer that should pass through
//! what is not drawn.
//!
//! Recoil drew the level whole, so this is presentation only.

use bevy::asset::{RenderAssetUsages, embedded_asset, uuid_handle};
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::render::storage::ShaderBuffer;
use bevy::shader::ShaderRef;

/// The material every model is drawn with: `StandardMaterial`, cut away by
/// [`Cutaway`].
pub type ModelMaterial = ExtendedMaterial<StandardMaterial, CutawayExtension>;

/// The `MeshTag` of a mesh always drawn as it is.
pub const KEEP: u32 = 1;

/// The low byte of a [`when_seen`] tag.
pub const WHEN_SEEN: u32 = 2;

/// The `MeshTag` of a mesh drawn only while the sight sees the point `lift`
/// metres over its origin, at most `margin` metres past the first surface
/// in that direction ([`Sight::sees_body`]). Both are kept to a tenth of a
/// metre, up to 25.5 m.
pub const fn when_seen(lift: f32, margin: f32) -> u32 {
    WHEN_SEEN | (tenths(lift) << 8) | (tenths(margin) << 16)
}

/// `metres` in tenths, rounded, held to a byte.
const fn tenths(metres: f32) -> u32 {
    let tenths = metres * 10.0 + 0.5;
    if tenths <= 0.0 {
        0
    } else if tenths >= 255.0 {
        255
    } else {
        tenths as u32
    }
}

/// The most directions a [`Sight`] holds: its azimuths times its
/// elevations. The buffer is this size whatever the sight's, so it never
/// grows and every material keeps its binding.
pub const MAX_DIRECTIONS: usize = 8192;

/// The most cells an [`Explored`] map holds, one bit each.
pub const MAX_CELLS: usize = 512 * 512;

/// How far past the depth a sight holds a surface is still fully seen,
/// metres: the sight is coarse, and a surface is never exactly where a ray
/// met it.
const SEEN_MARGIN: f32 = 1.5;

/// The same, as a share of the depth: a direction's cell is wider further
/// out.
const SEEN_MARGIN_SHARE: f32 = 0.03;

/// How far beyond that a surface fades from seen to remembered, metres...
const SEEN_SOFT: f32 = 3.0;

/// ...and as a share of the depth.
const SEEN_SOFT_SHARE: f32 = 0.05;

/// How far a surface may turn away from the viewpoint and still be seen a
/// little, as the cosine of the angle off edge-on: the level's terrain is
/// large flat facets, and judged strictly a slope comes out a patchwork of
/// seen and unseen triangles.
const FACING_AWAY: f32 = -0.3;

/// How far it turns toward the viewpoint before it is fully seen.
const FACING_TOWARD: f32 = 0.2;

/// The share of the reach over which the level fades out at its edge.
const EDGE_FADE: f32 = 0.08;

/// How far short of the depth space counts as open, metres: not the
/// surface itself.
const OPEN_MARGIN: f32 = 0.3;

/// How far past the sight's depth a point found open in its column still
/// counts as seen space, metres: a little into the wall it met, so a
/// tunnel is cut out to its walls, not as far as the inside of a crate.
const REACH_SLACK: f32 = 1.0;

/// How many points behind a surface are tried for open space.
const HIDE_STEPS: u32 = 8;

/// The side of an [`OpenColumns`] grid, in columns.
pub const COLUMNS: u32 = 512;

/// The step [`OpenColumns`] keeps heights in, metres.
const HEIGHT_UNIT: f32 = 0.25;

/// A column with no open space: its bottom over its top.
const NO_SPAN: u32 = pack_span(i16::MAX, i16::MIN);

/// How far behind a surface open space is looked for at most, metres.
const HIDE_LENGTH: f32 = 300.0;

/// How far below the viewpoint open space is looked for, metres.
const HIDE_DEPTH: f32 = 30.0;

/// The size of the small buffer: what changes every frame.
const HEADER_BYTES: usize = 96;

/// The size of the large buffer: the sight's depths, the explored bits and
/// the open columns.
const DATA_BYTES: usize = 4 * (MAX_DIRECTIONS + MAX_CELLS / 32 + (COLUMNS * COLUMNS) as usize);

/// The small buffer every [`ModelMaterial`] reads the cutaway from, written
/// whenever the cutaway changes.
const HEADER: Handle<ShaderBuffer> = uuid_handle!("6f70656e-6266-742d-6375-746177617921");

/// The large one, written only when the sight or the explored ground does.
const DATA: Handle<ShaderBuffer> = uuid_handle!("6f70656e-6266-742d-6375-746177617922");

/// How the level is cut. See the module docs.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct Cutaway {
    /// Whether anything is cut at all.
    pub enabled: bool,
    /// The smallest scale of a view's transform that is cut.
    pub min_view_scale: f32,
    /// Where the level is seen from, world space: what is cut is what hides
    /// the sight from here.
    pub eye: Vec3,
    /// What the viewpoint sees.
    pub sight: Sight,
    /// The ground it has seen.
    pub explored: Explored,
    /// How bright ground seen before but not now is drawn, 0–1.
    pub explored_shade: f32,
    /// How bright what has never been seen is drawn, 0–1.
    pub unseen_shade: f32,
}

impl Default for Cutaway {
    fn default() -> Cutaway {
        Cutaway {
            enabled: false,
            min_view_scale: 1.0,
            eye: Vec3::ZERO,
            sight: Sight::default(),
            explored: Explored::default(),
            explored_shade: 0.5,
            unseen_shade: 0.25,
        }
    }
}

/// What a viewpoint sees: how far it sees in each of a grid of directions
/// round it, `azimuths` round the horizon by `elevations` from straight
/// down to straight up, rows packed closer near the horizon
/// ([`Sight::direction`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sight {
    /// Where it sees from, world space.
    pub origin: Vec3,
    /// How far it sees at most, metres; also how far across the ground the
    /// level is drawn.
    pub reach: f32,
    /// Columns round the horizon.
    pub azimuths: u32,
    /// Rows from down to up.
    pub elevations: u32,
    /// The distance to the first surface per direction, `reach` where there
    /// is none; row by row from the lowest.
    pub depths: Vec<f32>,
    /// The open space those directions reach through, column by column.
    pub columns: OpenColumns,
}

impl Sight {
    /// A sight from `origin` with every direction open to `reach`, at most
    /// [`MAX_DIRECTIONS`] directions.
    pub fn open(origin: Vec3, reach: f32, azimuths: u32, elevations: u32) -> Sight {
        let azimuths = azimuths.clamp(1, MAX_DIRECTIONS as u32);
        let elevations = elevations.clamp(1, MAX_DIRECTIONS as u32 / azimuths);
        Sight {
            origin,
            reach,
            azimuths,
            elevations,
            depths: vec![reach; (azimuths * elevations) as usize],
            columns: OpenColumns::around(origin, reach),
        }
    }

    /// Record that direction `column`, `row` meets its first surface
    /// `depth` out: the depth, and the open space on the way.
    pub fn record(&mut self, column: u32, row: u32, depth: f32) {
        let index = (row * self.azimuths + column) as usize;
        if let Some(slot) = self.depths.get_mut(index) {
            *slot = depth;
            let direction = self.direction(column, row);
            self.columns
                .add_ray(self.origin, direction, depth - OPEN_MARGIN);
        }
    }

    /// Whether there are directions to judge by at all.
    pub fn is_empty(&self) -> bool {
        self.depths.is_empty()
    }

    /// The direction at the centre of column `column`, row `row`.
    ///
    /// Rows are even in the cube root of the direction's height, so they
    /// crowd round the horizon, where a step of elevation is the most
    /// ground.
    pub fn direction(&self, column: u32, row: u32) -> Vec3 {
        let angle = (column as f32 + 0.5) / self.azimuths as f32 * std::f32::consts::TAU
            - std::f32::consts::PI;
        let even = (row as f32 + 0.5) / self.elevations as f32 * 2.0 - 1.0;
        let height = even * even * even;
        let across = (1.0 - height * height).max(0.0).sqrt();
        Vec3::new(angle.cos() * across, height, angle.sin() * across)
    }

    /// The four depths round the direction of `offset`, the nearest
    /// columns and rows either side, and how far between them it lies.
    fn around(&self, offset: Vec3) -> ([f32; 4], Vec2) {
        let length = offset.length();
        if length <= f32::EPSILON || self.is_empty() {
            return ([self.reach; 4], Vec2::ZERO);
        }
        let (azimuths, elevations) = (self.azimuths as f32, self.elevations as f32);
        let column = (offset.z.atan2(offset.x) + std::f32::consts::PI) / std::f32::consts::TAU
            * azimuths
            - 0.5;
        let row = ((offset.y / length).cbrt() + 1.0) / 2.0 * elevations - 0.5;
        let row = row.clamp(0.0, elevations - 1.0);
        let column_0 = column.floor().rem_euclid(azimuths) as u32 % self.azimuths;
        let column_1 = (column_0 + 1) % self.azimuths;
        let row_0 = row.floor() as u32;
        let row_1 = (row_0 + 1).min(self.elevations - 1);
        let at = |column: u32, row: u32| {
            self.depths
                .get((row * self.azimuths + column) as usize)
                .copied()
                .unwrap_or(self.reach)
        };
        (
            [
                at(column_0, row_0),
                at(column_1, row_0),
                at(column_0, row_1),
                at(column_1, row_1),
            ],
            Vec2::new(column - column.floor(), row - row.floor()),
        )
    }

    /// How far the sight reaches toward `offset`, blended between the
    /// directions round it, so it changes smoothly across them.
    pub fn depth_towards(&self, offset: Vec3) -> f32 {
        let ([low_0, low_1, high_0, high_1], between) = self.around(offset);
        let low = low_0 + (low_1 - low_0) * between.x;
        let high = high_0 + (high_1 - high_0) * between.x;
        low + (high - low) * between.y
    }

    /// How much the viewpoint sees the surface at `point` facing `normal`,
    /// 0–1: fully when it faces the viewpoint and lies within the sight's
    /// depth, fading as it turns edge-on or lies beyond.
    pub fn seen(&self, point: Vec3, normal: Vec3) -> f32 {
        let offset = point - self.origin;
        let distance = offset.length();
        if distance > self.reach || distance <= f32::EPSILON {
            return 0.0;
        }
        let facing = smoothstep(FACING_AWAY, FACING_TOWARD, normal.dot(-offset / distance));
        let depth = self.depth_towards(offset);
        let start = depth + SEEN_MARGIN + depth * SEEN_MARGIN_SHARE;
        let end = start + SEEN_SOFT + depth * SEEN_SOFT_SHARE;
        facing * (1.0 - smoothstep(start, end, distance))
    }

    /// Whether the viewpoint sees the surface at `point` facing `normal`, at
    /// least half.
    pub fn sees(&self, point: Vec3, normal: Vec3) -> bool {
        self.seen(point, normal) >= 0.5
    }

    /// Whether the viewpoint's sight reaches `point`: no further than
    /// [`REACH_SLACK`] past the first surface in its direction.
    pub fn reaches(&self, point: Vec3) -> bool {
        let offset = point - self.origin;
        let distance = offset.length();
        distance <= self.reach && distance <= self.depth_towards(offset) + REACH_SLACK
    }

    /// Whether the viewpoint sees the body of something at `origin`: the
    /// point `lift` over it, no further than `margin` past the first surface
    /// in its direction. A vehicle's middle is over its origin, and half of
    /// it may stand past a wall's edge and still be seen; a pickup in a
    /// crate is just past the crate's side.
    pub fn sees_body(&self, origin: Vec3, lift: f32, margin: f32) -> bool {
        let offset = origin + Vec3::Y * lift - self.origin;
        let distance = offset.length();
        distance <= self.reach && distance <= self.depth_towards(offset) + margin
    }
}

/// `edge0`..`edge1` to 0..1, eased at both ends, as WGSL's `smoothstep`.
fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The open space a viewpoint sees, as the span of heights it sees over
/// each patch of ground: a [`COLUMNS`]-square grid centred on it, out to its
/// reach.
///
/// One span a column, so space seen at two heights over the same ground
/// (under an arch and over it) reads as open all the way between; a
/// viewpoint seldom sees both.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OpenColumns {
    /// The corner of the grid with the least x and z, world space.
    pub corner: Vec2,
    /// A column's side, metres.
    pub cell: f32,
    /// Per column, row by row: the lowest and highest height seen, in
    /// [`HEIGHT_UNIT`]s, packed low half first.
    pub spans: Vec<u32>,
}

/// Two heights in one word, the bottom in the low half.
const fn pack_span(bottom: i16, top: i16) -> u32 {
    (bottom as u16 as u32) | ((top as u16 as u32) << 16)
}

/// The two heights of a packed span.
fn unpack_span(span: u32) -> (i16, i16) {
    (span as u16 as i16, (span >> 16) as u16 as i16)
}

impl OpenColumns {
    /// No open space yet, over the ground `reach` round `centre`.
    pub fn around(centre: Vec3, reach: f32) -> OpenColumns {
        let cell = (2.0 * reach / COLUMNS as f32).max(0.1);
        OpenColumns {
            corner: centre.xz() - Vec2::splat(cell * COLUMNS as f32 / 2.0),
            cell,
            spans: vec![NO_SPAN; (COLUMNS * COLUMNS) as usize],
        }
    }

    /// The column over `point` (x and z), if the grid covers it.
    fn index(&self, point: Vec2) -> Option<usize> {
        if self.cell <= 0.0 || self.spans.is_empty() {
            return None;
        }
        let at = ((point - self.corner) / self.cell).floor();
        (at.x >= 0.0 && at.y >= 0.0 && at.x < COLUMNS as f32 && at.y < COLUMNS as f32)
            .then(|| (at.y as u32 * COLUMNS + at.x as u32) as usize)
    }

    /// Widen the columns a ray from `origin` along `direction` passes
    /// through, for `length` metres, to take in the heights it passes at.
    pub fn add_ray(&mut self, origin: Vec3, direction: Vec3, length: f32) {
        if length <= 0.0 {
            return;
        }
        // A column at a time across the ground, both ends at least; one cut
        // across a corner is filled in by [`widen`](Self::widen).
        let across = direction.xz().length() * length;
        let steps = (across / self.cell).ceil().max(1.0) as u32;
        for step in 0..=steps {
            let point = origin + direction * (length * step as f32 / steps as f32);
            let Some(index) = self.index(point.xz()) else {
                continue;
            };
            let height = (point.y / HEIGHT_UNIT).clamp(i16::MIN as f32, i16::MAX as f32);
            let (bottom, top) = unpack_span(self.spans[index]);
            self.spans[index] = pack_span(
                bottom.min(height.floor() as i16),
                top.max(height.ceil() as i16),
            );
        }
    }

    /// Grow every column's span over its neighbours `cells` columns round,
    /// so open space reaches up to the walls round it rather than stopping
    /// a column short.
    pub fn widen(&mut self, cells: u32) {
        let side = COLUMNS as usize;
        if self.spans.len() != side * side {
            return;
        }
        let merge = |a: u32, b: u32| {
            let ((a_bottom, a_top), (b_bottom, b_top)) = (unpack_span(a), unpack_span(b));
            pack_span(a_bottom.min(b_bottom), a_top.max(b_top))
        };
        // Across, then along: a square neighbourhood in two passes.
        for (step_x, step_z) in [(1, 0), (0, 1)] {
            let before = self.spans.clone();
            for z in 0..side {
                for x in 0..side {
                    let mut span = before[z * side + x];
                    for reach in 1..=cells as usize {
                        for sign in [-1isize, 1] {
                            let nx = x as isize + sign * (reach * step_x) as isize;
                            let nz = z as isize + sign * (reach * step_z) as isize;
                            if (0..side as isize).contains(&nx) && (0..side as isize).contains(&nz)
                            {
                                span = merge(span, before[nz as usize * side + nx as usize]);
                            }
                        }
                    }
                    self.spans[z * side + x] = span;
                }
            }
        }
    }

    /// Whether `point` is in open space the viewpoint sees.
    pub fn is_open(&self, point: Vec3) -> bool {
        self.index(point.xz()).is_some_and(|index| {
            let (bottom, top) = unpack_span(self.spans[index]);
            let height = point.y / HEIGHT_UNIT;
            bottom as f32 <= height && height <= top as f32
        })
    }
}

/// The ground seen so far, as a grid of cells over the level, one bit each.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Explored {
    /// The corner of the grid with the least x and z, world space.
    pub corner: Vec2,
    /// A cell's side, metres.
    pub cell: f32,
    /// Cells along x.
    pub columns: u32,
    /// Cells along z.
    pub rows: u32,
    /// One bit per cell, row by row, 32 to a word.
    pub bits: Vec<u32>,
}

impl Explored {
    /// An unexplored grid over the ground from `min` to `max` (x and z), of
    /// cells as small as [`MAX_CELLS`] allows.
    pub fn covering(min: Vec2, max: Vec2) -> Explored {
        let size = (max - min).max(Vec2::ONE);
        let mut cell = (size.x * size.y / MAX_CELLS as f32).sqrt().max(0.5);
        loop {
            let columns = (size.x / cell).ceil() as u32;
            let rows = (size.y / cell).ceil() as u32;
            if (columns * rows) as usize <= MAX_CELLS {
                return Explored {
                    corner: min,
                    cell,
                    columns,
                    rows,
                    bits: vec![0; (columns * rows).div_ceil(32) as usize],
                };
            }
            cell *= 1.01;
        }
    }

    /// The cell under `point`, if the grid covers it.
    fn index(&self, point: Vec2) -> Option<usize> {
        if self.cell <= 0.0 {
            return None;
        }
        let at = ((point - self.corner) / self.cell).floor();
        (at.x >= 0.0 && at.y >= 0.0 && (at.x as u32) < self.columns && (at.y as u32) < self.rows)
            .then(|| (at.y as u32 * self.columns + at.x as u32) as usize)
    }

    /// Mark the ground at `point` (x and z) as seen.
    pub fn mark(&mut self, point: Vec2) {
        if let Some(index) = self.index(point)
            && let Some(word) = self.bits.get_mut(index / 32)
        {
            *word |= 1 << (index % 32);
        }
    }

    /// How much of the ground round `point` (x and z) has been seen, 0–1,
    /// blended between the four nearest cells' middles, eased so the blend
    /// shows no creases along the cells.
    pub fn amount(&self, point: Vec2) -> f32 {
        if self.cell <= 0.0 {
            return 0.0;
        }
        let at = (point - self.corner) / self.cell - 0.5;
        let corner = at.floor();
        let between = (at - corner).map(|t| t * t * (3.0 - 2.0 * t));
        let seen = |dx: f32, dz: f32| {
            let middle = self.corner + (corner + Vec2::new(dx, dz) + 0.5) * self.cell;
            if self.is_explored(middle) { 1.0 } else { 0.0 }
        };
        let low = seen(0.0, 0.0) + (seen(1.0, 0.0) - seen(0.0, 0.0)) * between.x;
        let high = seen(0.0, 1.0) + (seen(1.0, 1.0) - seen(0.0, 1.0)) * between.x;
        low + (high - low) * between.y
    }

    /// Whether the ground at `point` (x and z) has been seen.
    pub fn is_explored(&self, point: Vec2) -> bool {
        self.index(point).and_then(|index| {
            self.bits
                .get(index / 32)
                .map(|word| word >> (index % 32) & 1)
        }) == Some(1)
    }
}

impl Cutaway {
    /// Whether the surface at `point`, facing `normal`, is cut from a view
    /// the cutaway applies to: the shader's judgement for an untagged mesh.
    pub fn cuts(&self, point: Vec3, normal: Vec3) -> bool {
        if !self.enabled {
            return false;
        }
        let sight = &self.sight;
        let edge = 1.0
            - smoothstep(
                sight.reach * (1.0 - EDGE_FADE),
                sight.reach,
                (point - sight.origin).xz().length(),
            );
        if sight.is_empty() {
            return edge < 0.5;
        }
        // The shader dissolves the edge; this takes it at half.
        edge < 0.5 || (sight.seen(point, normal) < 0.5 && self.hides_sight(point))
    }

    /// Whether `point` stands in front of open space the viewpoint sees, as
    /// the eye looks: whether open space lies behind it along the eye's line.
    pub fn hides_sight(&self, point: Vec3) -> bool {
        let Ok(away) = Dir3::new(point - self.eye) else {
            return false;
        };
        let floor = self.sight.origin.y - HIDE_DEPTH;
        let length = if away.y < -0.05 {
            ((point.y - floor) / -away.y).min(HIDE_LENGTH)
        } else {
            HIDE_LENGTH
        };
        // Closer together near the surface, where a thin wall leaves only a
        // sliver of open space behind it.
        length > 0.0
            && (1..=HIDE_STEPS).any(|step| {
                let along = step as f32 / HIDE_STEPS as f32;
                let behind = point + away * (length * along * along);
                // The column first, a single read; the sight's own depths
                // only for a point it says is open.
                self.sight.columns.is_open(behind) && self.sight.reaches(behind)
            })
    }

    /// The small buffer the shader reads (`cutaway.wgsl`'s `Cutaway`).
    fn header_bytes(&self) -> Vec<u8> {
        fn floats(bytes: &mut Vec<u8>, values: [f32; 4]) {
            for value in values {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        let (sight, explored, columns) = (&self.sight, &self.explored, &self.sight.columns);
        let mut bytes = Vec::with_capacity(HEADER_BYTES);
        let enabled = if self.enabled { 1.0 } else { 0.0 };
        floats(&mut bytes, self.eye.extend(enabled).to_array());
        floats(&mut bytes, sight.origin.extend(sight.reach).to_array());
        floats(
            &mut bytes,
            [
                self.min_view_scale,
                self.explored_shade,
                self.unseen_shade,
                0.0,
            ],
        );
        // No directions reads as no sight at all.
        let azimuths = if sight.is_empty() { 0 } else { sight.azimuths };
        for count in [azimuths, sight.elevations, explored.columns, explored.rows] {
            bytes.extend_from_slice(&count.to_le_bytes());
        }
        floats(
            &mut bytes,
            [explored.corner.x, explored.corner.y, explored.cell, 0.0],
        );
        floats(
            &mut bytes,
            [columns.corner.x, columns.corner.y, columns.cell, 0.0],
        );
        bytes
    }

    /// The large buffer the shader reads (`cutaway.wgsl`'s `CutawayData`).
    fn data_bytes(&self) -> Vec<u8> {
        let (sight, explored) = (&self.sight, &self.explored);
        let mut bytes = Vec::with_capacity(DATA_BYTES);
        for index in 0..MAX_DIRECTIONS {
            let depth = sight.depths.get(index).copied().unwrap_or(0.0);
            bytes.extend_from_slice(&depth.to_le_bytes());
        }
        for index in 0..MAX_CELLS / 32 {
            let word = explored.bits.get(index).copied().unwrap_or(0);
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        for index in 0..(COLUMNS * COLUMNS) as usize {
            let span = sight.columns.spans.get(index).copied().unwrap_or(NO_SPAN);
            bytes.extend_from_slice(&span.to_le_bytes());
        }
        bytes
    }
}

/// The cutaway's part of a [`ModelMaterial`]: the shared buffer.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct CutawayExtension {
    #[storage(100, read_only)]
    header: Handle<ShaderBuffer>,
    #[storage(101, read_only)]
    data: Handle<ShaderBuffer>,
}

impl Default for CutawayExtension {
    fn default() -> CutawayExtension {
        CutawayExtension {
            header: HEADER,
            data: DATA,
        }
    }
}

impl MaterialExtension for CutawayExtension {
    fn fragment_shader() -> ShaderRef {
        "embedded://bft_game/cutaway.wgsl".into()
    }
}

/// A [`ModelMaterial`] drawing as `base` does.
pub fn model_material(base: StandardMaterial) -> ModelMaterial {
    ExtendedMaterial {
        base,
        extension: CutawayExtension::default(),
    }
}

/// Draws models with [`ModelMaterial`] and keeps its buffer up to date.
pub struct CutawayPlugin;

impl Plugin for CutawayPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "cutaway.wgsl");
        app.add_plugins(MaterialPlugin::<ModelMaterial>::default())
            .init_resource::<Cutaway>()
            .add_systems(Startup, make_params)
            .add_systems(PostUpdate, write_params);
    }
}

/// Put the buffers at their fixed handles, so materials made before or
/// after can name them.
fn make_params(cutaway: Res<Cutaway>, buffers: Option<ResMut<Assets<ShaderBuffer>>>) {
    let Some(mut buffers) = buffers else {
        return;
    };
    for (handle, bytes) in [
        (HEADER, cutaway.header_bytes()),
        (DATA, cutaway.data_bytes()),
    ] {
        let buffer = ShaderBuffer::new(&bytes, RenderAssetUsages::RENDER_WORLD);
        if let Err(error) = buffers.insert(&handle, buffer) {
            warn!("cutaway: could not make its buffers: {error}");
        }
    }
}

/// Copy a changed [`Cutaway`] into the buffers: the small one whenever it
/// changes, the large one only when its sight or explored ground did. They
/// keep their sizes, so Bevy writes them in place and no material has to be
/// rebound.
fn write_params(
    cutaway: Res<Cutaway>,
    buffers: Option<ResMut<Assets<ShaderBuffer>>>,
    mut written: Local<Option<(Sight, Explored)>>,
) {
    if !cutaway.is_changed() {
        return;
    }
    let Some(mut buffers) = buffers else {
        return;
    };
    if let Some(mut header) = buffers.get_mut(&HEADER) {
        header.data = Some(cutaway.header_bytes());
    }
    let stale = written
        .as_ref()
        .is_none_or(|(sight, explored)| *sight != cutaway.sight || *explored != cutaway.explored);
    if stale && let Some(mut data) = buffers.get_mut(&DATA) {
        data.data = Some(cutaway.data_bytes());
        *written = Some((cutaway.sight.clone(), cutaway.explored.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A viewpoint 2 m over flat ground at y = 0, in a tunnel 10 m wide
    /// along z (walls at x = ±5) under a ceiling at y = 6, open to 400 m
    /// along it; the depths cast against those surfaces by hand.
    fn tunnel() -> Sight {
        let mut sight = Sight::open(Vec3::new(0.0, 2.0, 0.0), 400.0, 128, 32);
        for row in 0..sight.elevations {
            for column in 0..sight.azimuths {
                let direction = sight.direction(column, row);
                let mut depth = sight.reach;
                if direction.y < 0.0 {
                    depth = depth.min(2.0 / -direction.y);
                }
                if direction.y > 0.0 {
                    depth = depth.min(4.0 / direction.y);
                }
                if direction.x.abs() > 1e-4 {
                    depth = depth.min(5.0 / direction.x.abs());
                }
                sight.record(column, row, depth);
            }
        }
        sight
    }

    /// Looking down at 45° from the east (+x), a little south.
    fn cutaway() -> Cutaway {
        Cutaway {
            enabled: true,
            min_view_scale: 2.0,
            eye: Vec3::new(50.0, 55.0, 10.0),
            sight: tunnel(),
            explored: Explored::covering(Vec2::splat(-500.0), Vec2::splat(500.0)),
            ..default()
        }
    }

    #[test]
    fn the_tunnel_floor_and_far_wall_are_seen() {
        let cutaway = cutaway();
        assert!(!cutaway.cuts(Vec3::new(1.0, 0.0, 6.0), Vec3::Y));
        assert!(cutaway.sight.sees(Vec3::new(-5.0, 3.0, 2.0), Vec3::X));
        assert!(!cutaway.cuts(Vec3::new(-5.0, 3.0, 2.0), Vec3::X));
    }

    #[test]
    fn the_roof_over_it_and_the_near_wall_are_cut() {
        let cutaway = cutaway();
        // The roof's top, a metre of rock over the ceiling.
        assert!(cutaway.cuts(Vec3::new(0.0, 7.0, 3.0), Vec3::Y));
        // The near wall's outside, facing the eye.
        assert!(cutaway.cuts(Vec3::new(6.0, 3.0, 3.0), Vec3::X));
    }

    #[test]
    fn ground_beyond_the_far_wall_is_drawn_dark_not_cut() {
        let mut cutaway = cutaway();
        let beyond = Vec3::new(-30.0, 0.0, 0.0);
        assert!(!cutaway.sight.sees(beyond, Vec3::Y));
        assert!(!cutaway.cuts(beyond, Vec3::Y));
        cutaway.explored.mark(beyond.xz());
        assert!(!cutaway.cuts(beyond, Vec3::Y));
    }

    /// A crate 2 m on a side on flat ground at x = 8…10, seen from 4 m up at
    /// the origin, with the depths cast against it and the ground.
    fn crate_sight() -> Sight {
        let mut sight = Sight::open(Vec3::new(0.0, 4.0, 0.0), 400.0, 256, 32);
        let (low, high) = (Vec3::new(8.0, 0.0, -1.0), Vec3::new(10.0, 2.0, 1.0));
        for row in 0..sight.elevations {
            for column in 0..sight.azimuths {
                let direction = sight.direction(column, row);
                let mut depth = sight.reach;
                if direction.y < 0.0 {
                    depth = depth.min(4.0 / -direction.y);
                }
                // Slab test against the crate.
                let inverse = direction.recip();
                let near = (low - sight.origin) * inverse;
                let far = (high - sight.origin) * inverse;
                let enter = near.min(far).max_element();
                let leave = near.max(far).min_element();
                if enter <= leave && enter > 0.0 {
                    depth = depth.min(enter);
                }
                sight.record(column, row, depth);
            }
        }
        sight.columns.widen(1);
        sight
    }

    #[test]
    fn a_crates_far_side_is_dark_not_cut() {
        let cutaway = Cutaway {
            enabled: true,
            eye: Vec3::new(40.0, 30.0, 0.0),
            sight: crate_sight(),
            ..default()
        };
        // The side away from the tank, toward the eye, and the top.
        let side = Vec3::new(10.0, 1.0, 0.0);
        assert!(!cutaway.sight.sees(side, Vec3::X));
        assert!(!cutaway.cuts(side, Vec3::X));
        assert!(!cutaway.cuts(Vec3::new(9.5, 2.0, 0.5), Vec3::Y));
        // The side toward the tank is seen.
        assert!(cutaway.sight.sees(Vec3::new(8.0, 1.0, 0.0), Vec3::NEG_X));
    }

    #[test]
    fn nothing_past_the_reach_is_drawn() {
        let mut cutaway = cutaway();
        cutaway.explored.mark(Vec2::new(0.0, 450.0));
        assert!(cutaway.cuts(Vec3::new(0.0, 0.0, 450.0), Vec3::Y));
    }

    #[test]
    fn disabled_cuts_nothing() {
        let cutaway = Cutaway {
            enabled: false,
            ..cutaway()
        };
        assert!(!cutaway.cuts(Vec3::new(0.0, 7.0, 3.0), Vec3::Y));
        assert!(!cutaway.cuts(Vec3::new(1000.0, 0.0, 0.0), Vec3::Y));
    }

    #[test]
    fn a_direction_is_found_in_its_own_cell() {
        let sight = Sight::open(Vec3::ZERO, 100.0, 64, 16);
        for (column, row) in [(0, 0), (10, 7), (63, 15), (31, 8)] {
            let mut marked = sight.clone();
            marked.depths[(row * 64 + column) as usize] = 1.0;
            let (around, _) = marked.around(sight.direction(column, row) * 10.0);
            assert!(around.contains(&1.0), "{column} {row}: {around:?}");
        }
    }

    #[test]
    fn the_grid_marks_and_remembers() {
        let mut explored = Explored::covering(Vec2::new(1000.0, 1200.0), Vec2::new(2800.0, 3400.0));
        assert!((explored.columns * explored.rows) as usize <= MAX_CELLS);
        assert!(!explored.is_explored(Vec2::new(2000.0, 2000.0)));
        explored.mark(Vec2::new(2000.0, 2000.0));
        assert!(explored.is_explored(Vec2::new(2000.0, 2000.0)));
        assert!(!explored.is_explored(Vec2::new(2100.0, 2000.0)));
        assert!(!explored.is_explored(Vec2::new(0.0, 0.0)));
    }

    #[test]
    fn a_when_seen_tag_carries_its_lift_and_margin() {
        let tag = when_seen(1.5, 3.0);
        assert_eq!(tag & 0xff, WHEN_SEEN);
        assert_eq!((tag >> 8) & 0xff, 15);
        assert_eq!((tag >> 16) & 0xff, 30);
        assert_eq!((when_seen(-1.0, 100.0) >> 8) & 0xffff, 255 << 8);
    }

    #[test]
    fn the_buffers_are_always_the_same_size() {
        let empty = Cutaway::default();
        assert_eq!(empty.header_bytes().len(), HEADER_BYTES);
        assert_eq!(cutaway().header_bytes().len(), HEADER_BYTES);
        assert_eq!(empty.data_bytes().len(), DATA_BYTES);
        assert_eq!(cutaway().data_bytes().len(), DATA_BYTES);
        assert_eq!(HEADER_BYTES % 16, 0);
    }

    #[test]
    fn widening_spreads_a_column_to_its_neighbours() {
        let mut columns = OpenColumns::around(Vec3::ZERO, 100.0);
        columns.add_ray(Vec3::new(0.0, 2.0, 0.0), Vec3::Y, 4.0);
        let beside = Vec3::new(columns.cell * 1.1, 3.0, 0.0);
        assert!(!columns.is_open(beside));
        columns.widen(1);
        assert!(columns.is_open(beside));
        assert!(columns.is_open(Vec3::new(columns.cell, 3.0, columns.cell)));
        assert!(!columns.is_open(Vec3::new(columns.cell * 2.5, 3.0, 0.0)));
    }

    #[test]
    fn a_body_beside_the_tank_is_seen_and_one_behind_a_wall_is_not() {
        let sight = tunnel();
        assert!(sight.sees_body(Vec3::new(0.0, 0.0, 40.0), 1.5, 3.0));
        assert!(!sight.sees_body(Vec3::new(-30.0, 0.0, 0.0), 1.5, 3.0));
        // Just inside the wall, as a pickup in a crate is: seen only with
        // the slack a vehicle gets.
        assert!(sight.sees_body(Vec3::new(-6.0, 1.0, 0.0), 0.5, 3.0));
        assert!(!sight.sees_body(Vec3::new(-6.0, 1.0, 0.0), 0.5, 0.5));
    }

    #[test]
    fn seeing_fades_rather_than_stops() {
        let sight = tunnel();
        let wall = sight.seen(Vec3::new(-5.0, 3.0, 2.0), Vec3::X);
        assert!(wall > 0.99, "{wall}");
        // Turned away, and well behind the wall.
        assert_eq!(sight.seen(Vec3::new(-5.0, 3.0, 2.0), Vec3::NEG_X), 0.0);
        // Turned almost edge-on to the viewpoint.
        let floor = Vec3::new(1.0, 0.0, 6.0);
        let toward = (sight.origin - floor).normalize();
        let edge_on = toward.cross(Vec3::X).normalize();
        let turning = sight.seen(floor, (edge_on + toward * 0.05).normalize());
        assert!(turning > 0.0 && turning < 1.0, "{turning}");
        assert_eq!(sight.seen(Vec3::new(-20.0, 3.0, 2.0), Vec3::X), 0.0);
    }

    #[test]
    fn a_column_is_open_between_the_heights_a_ray_passed() {
        let mut columns = OpenColumns::around(Vec3::ZERO, 100.0);
        columns.add_ray(Vec3::new(0.0, 2.0, 0.0), Vec3::new(0.6, 0.8, 0.0), 10.0);
        // Along the ray.
        assert!(columns.is_open(Vec3::new(3.0, 6.0, 0.0)));
        // Under it, and past its end.
        assert!(!columns.is_open(Vec3::new(3.0, 1.0, 0.0)));
        assert!(!columns.is_open(Vec3::new(9.0, 14.0, 0.0)));
        // Off to the side.
        assert!(!columns.is_open(Vec3::new(3.0, 6.0, 20.0)));
    }
}
