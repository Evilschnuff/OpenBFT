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
//!   sight, so the edge of what is seen is soft. The viewpoint is not a
//!   point but a patch [`lean`](Sight::lean) metres round it, as a head
//!   leans to look past what is in the way: something narrow (a pillar, a
//!   post) hides only a short shadow right behind it, while a wall hides
//!   all behind it ([`Sight::leans_past`]);
//! - one that is not seen but **hides** space the viewpoint sees from the
//!   [`eye`](Cutaway::eye) is cut: the roof over a tunnel and the wall on
//!   the eye's side of it, the far side of a hill in front of a valley.
//!   Which space it sees is kept as [`OpenColumns`], the heights it sees
//!   over each patch of ground, and the eye's line behind a surface is
//!   walked across every column it crosses ([`OpenColumns::walk`]), so a
//!   low tunnel deep under the ground is not stepped over. A point a
//!   column says is open is confirmed against the sight's own depths
//!   ([`Sight::reaches`]): a column also holds the air beside a crate, but
//!   the sight stops at the crate;
//! - any other surface is drawn darkened and greyed: on ground seen before
//!   to [`explored_shade`](Cutaway::explored_shade), elsewhere to the darker
//!   [`unseen_shade`](Cutaway::unseen_shade), so the side of a crate the
//!   viewpoint cannot see is a dark side, not a window onto what is inside;
//! - anything further across the ground than the sight's
//!   [`reach`](Sight::reach) is not drawn, its edge dissolved in a fine
//!   grain fixed to the world (in the shader; [`Cutaway::cuts`] takes it
//!   halfway) rather than a hard line.
//!
//! A cut does not stop at a line. Toward its edge the eye's line behind a
//! surface passes less and less of the space the viewpoint sees
//! ([`Cutaway::hidden_behind`]), and the surface thins out over that band,
//! dissolved in the same world-fixed grain as the level's rim, what is left
//! of it tinted toward [`rim_colour`](Cutaway::rim_colour): the edge of
//! every cut, and of the level, shows as cut rather than as ragged pixels.
//!
//! With [`only_seen`](Cutaway::only_seen) the rule is simpler: only what the
//! viewpoint sees is drawn, and everything else is cut, the ground seen
//! before too.
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

use std::any::TypeId;

use bevy::asset::{RenderAssetUsages, embedded_asset, uuid_handle};
use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::{VisibilitySystems, VisibleEntities, check_visibility_cpu_culling};
use bevy::mesh::MeshTag;
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

/// The `MeshTag` of a mesh judged like the level but never cut for hiding
/// what the viewpoint sees: something small, a post or a crate, that a
/// viewer would lean to look past rather than want taken away. Where the
/// viewpoint does not see it, it is darkened.
pub const SPARE: u32 = 3;

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

/// The widest angle a lean looks aside by, radians: a surface right beside
/// the viewpoint would otherwise send the test round to its back.
const LEAN_MAX_ANGLE: f32 = 0.35;

/// How much less the viewpoint leans up or down than sideways: a head
/// moves further side to side than it rises.
const LEAN_UPRIGHT: f32 = 0.5;

/// How many directions round a hidden point a lean tries.
const LEAN_SAMPLES: u32 = 6;

/// How much further than what hides a point a direction beside it has to
/// see before it counts as seeing past, metres...
const LEAN_CLEAR: f32 = 2.0;

/// ...and as a share of that distance.
const LEAN_CLEAR_SHARE: f32 = 0.1;

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

/// The most columns a line behind a surface is walked across looking for
/// open space ([`OpenColumns::walk`]): 240 m of ground at the usual
/// 2.5 m column, more than a line looking down at a model crosses.
const WALK_COLUMNS: u32 = 96;

/// How far behind a surface the walk starts, metres: not the surface.
const WALK_LEAD: f32 = 0.1;

/// How much open space the viewpoint sees has to lie behind a surface, along
/// the eye's line, for it to be cut through, metres; less thins the cut out
/// ([`Cutaway::hidden_behind`]).
const RIM_LENGTH: f32 = 3.0;

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
const HEADER_BYTES: usize = 160;

/// The size of the large buffer: the sight's depths, the explored bits and
/// the open columns.
const DATA_BYTES: usize =
    4 * (MAX_DIRECTIONS + MAX_CELLS / 32 + (COLUMNS * COLUMNS) as usize + MAX_DIRECTIONS);

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
    /// What it saw before [`sight`](Self::sight), while the view fades over
    /// from one to the other ([`fade`](Self::fade)): a sight is built over
    /// several frames and replaced whole, and swapped at once every
    /// surface it changes would jump. Only how seen a surface is fades;
    /// what is cut and which bodies show go by `sight` alone.
    pub previous: Option<Sight>,
    /// How far the view has faded from [`previous`](Self::previous) to
    /// [`sight`](Self::sight), 0–1.
    pub fade: f32,
    /// The ground it has seen.
    pub explored: Explored,
    /// How bright ground seen before but not now is drawn, 0–1.
    pub explored_shade: f32,
    /// How bright what has never been seen is drawn, 0–1.
    pub unseen_shade: f32,
    /// Whether to draw only what the viewpoint sees and cut everything
    /// else, rather than cut only what hides it.
    pub only_seen: bool,
    /// The colour the edge of a cut is tinted toward, linear: the band in
    /// which a surface thins out to nothing, and the level's rim, so that
    /// what is cut away shows as cut.
    pub rim_colour: LinearRgba,
}

impl Default for Cutaway {
    fn default() -> Cutaway {
        Cutaway {
            enabled: false,
            min_view_scale: 1.0,
            eye: Vec3::ZERO,
            sight: Sight::default(),
            previous: None,
            fade: 1.0,
            explored: Explored::default(),
            explored_shade: 0.5,
            unseen_shade: 0.25,
            only_seen: false,
            rim_colour: LinearRgba::rgb(0.6, 0.6, 0.65),
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
    /// How far round [`origin`](Self::origin) the viewpoint may move to see
    /// past what is in the way, metres; less up and down than sideways. 0
    /// sees from the origin alone.
    pub lean: f32,
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
            lean: 0.0,
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

    /// How far the nearest of the four directions round `offset` reaches:
    /// at the edge of something, where the blend of
    /// [`depth_towards`](Self::depth_towards) lies between it and what is
    /// behind it, this is the thing itself.
    pub fn nearest_depth_towards(&self, offset: Vec3) -> f32 {
        let (depths, _) = self.around(offset);
        depths.into_iter().fold(f32::INFINITY, f32::min)
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
        let direct = 1.0 - smoothstep(start, end, distance);
        let seen = if direct < 1.0 {
            direct.max(self.leans_past(offset, distance))
        } else {
            direct
        };
        facing * seen
    }

    /// How much a viewpoint leaning up to [`lean`](Self::lean) round the
    /// origin sees past what hides the point `offset`, `distance` away, 0–1.
    ///
    /// Leaning `lean` aside, the line to the point crosses the distance of
    /// what hides it at an angle `lean · (1/depth − 1/distance)` off the
    /// point's own direction, as the origin sees it. So the directions that
    /// far round it are tried: where one sees well past `depth`, what hides
    /// the point ends before that, and a lean sees round it. Something
    /// narrower than the lean, near the point, hides it from no lean at all;
    /// a wall wider than that hides it from every one.
    ///
    /// What hides the point is the nearest of the directions round it
    /// ([`nearest_depth_towards`](Self::nearest_depth_towards)): at the edge
    /// of something the blended depth lies halfway to what is behind it,
    /// where nothing is, and a lean measured from there would hardly look
    /// aside, leaving a dark line round every shadow.
    pub fn leans_past(&self, offset: Vec3, distance: f32) -> f32 {
        let depth = self.nearest_depth_towards(offset);
        if self.lean <= 0.0 || depth <= f32::EPSILON || depth >= distance {
            return 0.0;
        }
        let angle = (self.lean * (1.0 / depth - 1.0 / distance)).min(LEAN_MAX_ANGLE);
        let toward = offset / distance;
        let side = toward.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
        let up = side.cross(toward);
        let clear = depth + LEAN_CLEAR + depth * LEAN_CLEAR_SHARE;
        let full = clear + LEAN_CLEAR + depth * LEAN_CLEAR_SHARE;
        (0..LEAN_SAMPLES)
            .map(|sample| {
                let around = sample as f32 / LEAN_SAMPLES as f32 * std::f32::consts::TAU;
                let aside = side * around.cos() + up * (around.sin() * LEAN_UPRIGHT);
                let beside = toward * angle.cos() + aside * angle.sin();
                smoothstep(clear, full, self.depth_towards(beside))
            })
            .fold(0.0, f32::max)
    }

    /// Whether the viewpoint sees the surface at `point` facing `normal`, at
    /// least half.
    pub fn sees(&self, point: Vec3, normal: Vec3) -> bool {
        self.seen(point, normal) >= 0.5
    }

    /// Whether the viewpoint's sight reaches `point`: it lies short of the
    /// first surface in its direction, in the open space the sight passes
    /// through. Not past it: a point there is inside what the sight met, and
    /// a cut made for it would open onto nothing.
    pub fn reaches(&self, point: Vec3) -> bool {
        let offset = point - self.origin;
        let distance = offset.length();
        distance <= self.reach && distance <= self.depth_towards(offset)
    }

    /// Whether the viewpoint sees the body of something at `origin`: the
    /// point `lift` over it, no further than `margin` past the first surface
    /// in its direction. A vehicle's middle is over its origin, and half of
    /// it may stand past a wall's edge and still be seen; a pickup in a
    /// crate is just past the crate's side.
    ///
    /// A lean ([`Sight::leans_past`]) sees a body past something narrow too.
    pub fn sees_body(&self, origin: Vec3, lift: f32, margin: f32) -> bool {
        let offset = origin + Vec3::Y * lift - self.origin;
        let distance = offset.length();
        if distance > self.reach {
            return false;
        }
        let depth = self.depth_towards(offset);
        distance <= depth + margin || self.leans_past(offset, distance) >= 0.5
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

    /// Walk the line from `start` along `direction` (a unit vector) for
    /// `length` metres column by column, and add up how far of it runs
    /// through open space: each stretch through a column's open span that
    /// `accept` takes, tried at its middle and its far end. Stops once it
    /// has `enough`.
    ///
    /// Every column the line crosses is tried, so a tunnel only a few metres
    /// tall is not stepped over, at most [`WALK_COLUMNS`] of them.
    pub fn walk(
        &self,
        start: Vec3,
        direction: Vec3,
        length: f32,
        enough: f32,
        mut accept: impl FnMut(Vec3) -> bool,
    ) -> f32 {
        let mut open = 0.0;
        if self.cell <= 0.0 || self.spans.is_empty() || length <= WALK_LEAD {
            return open;
        }
        let at = (start.xz() - self.corner) / self.cell;
        let mut cell = at.floor();
        let across = direction.xz();
        // Per axis: which way the line steps through columns, how far along
        // it the next boundary is, and how far between boundaries.
        let step = Vec2::new(across.x.signum(), across.y.signum());
        let boundary = |at: f32, cell: f32, along: f32| {
            if along.abs() <= 1e-6 {
                f32::INFINITY
            } else {
                let next = if along > 0.0 { cell + 1.0 } else { cell };
                (next - at) * self.cell / along
            }
        };
        let mut next = Vec2::new(
            boundary(at.x, cell.x, across.x),
            boundary(at.y, cell.y, across.y),
        );
        let delta = Vec2::new(
            if across.x.abs() <= 1e-6 {
                f32::INFINITY
            } else {
                self.cell / across.x.abs()
            },
            if across.y.abs() <= 1e-6 {
                f32::INFINITY
            } else {
                self.cell / across.y.abs()
            },
        );
        let mut enter = WALK_LEAD;
        for _ in 0..WALK_COLUMNS {
            let leave = next.x.min(next.y).min(length);
            if leave > enter
                && cell.x >= 0.0
                && cell.y >= 0.0
                && cell.x < COLUMNS as f32
                && cell.y < COLUMNS as f32
            {
                let (bottom, top) =
                    unpack_span(self.spans[(cell.y as u32 * COLUMNS + cell.x as u32) as usize]);
                let (bottom, top) = (bottom as f32 * HEIGHT_UNIT, top as f32 * HEIGHT_UNIT);
                // Where along the line its height is within the span; an
                // empty column's bottom is over its top.
                let (low, high) = if bottom > top {
                    (1.0, 0.0)
                } else if direction.y.abs() <= 1e-6 {
                    if (bottom..=top).contains(&start.y) {
                        (enter, leave)
                    } else {
                        (1.0, 0.0)
                    }
                } else {
                    let to_bottom = (bottom - start.y) / direction.y;
                    let to_top = (top - start.y) / direction.y;
                    (
                        to_bottom.min(to_top).max(enter),
                        to_bottom.max(to_top).min(leave),
                    )
                };
                if low <= high
                    && (accept(start + direction * ((low + high) / 2.0))
                        || accept(start + direction * high))
                {
                    open += high - low;
                    if open >= enough {
                        return open;
                    }
                }
            }
            if leave >= length {
                return open;
            }
            enter = leave;
            if next.x < next.y {
                cell.x += step.x;
                next.x += delta.x;
            } else {
                cell.y += step.y;
                next.y += delta.y;
            }
        }
        open
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
    /// How much the viewpoint sees the surface at `point` facing `normal`,
    /// 0–1, as the view shows it: [`Sight::seen`], faded from the previous
    /// sight's to the current one's.
    pub fn seen(&self, point: Vec3, normal: Vec3) -> f32 {
        let current = self.sight.seen(point, normal);
        match &self.previous {
            Some(previous) if self.fade < 1.0 && !previous.is_empty() => {
                let before = previous.seen(point, normal);
                before + (current - before) * self.fade.clamp(0.0, 1.0)
            }
            _ => current,
        }
    }

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
        let unseen = self.seen(point, normal) < 0.5;
        edge < 0.5 || (unseen && (self.only_seen || self.hides_sight(point)))
    }

    /// Whether `point` stands in front of open space the viewpoint sees, as
    /// the eye looks: whether at least half of [`RIM_LENGTH`] of open space
    /// lies behind it along the eye's line ([`Cutaway::hidden_behind`]).
    pub fn hides_sight(&self, point: Vec3) -> bool {
        self.hidden_behind(point) >= 0.5
    }

    /// How much open space the viewpoint sees lies behind `point` along the
    /// eye's line, 0–1: the length of it, over [`RIM_LENGTH`].
    ///
    /// Deep in front of what the viewpoint sees it is 1, and the surface is
    /// cut. Toward the edge of a cut the line behind it passes less and less
    /// of that space, so the cut thins out to nothing over a band rather
    /// than stopping at a ragged line: the shader dissolves the surface in
    /// that band and tints what is left of it ([`rim_colour`](Self::rim_colour)).
    pub fn hidden_behind(&self, point: Vec3) -> f32 {
        let Ok(away) = Dir3::new(point - self.eye) else {
            return 0.0;
        };
        let floor = self.sight.origin.y - HIDE_DEPTH;
        let length = if away.y < -0.05 {
            ((point.y - floor) / -away.y).min(HIDE_LENGTH)
        } else {
            HIDE_LENGTH
        };
        // Every column behind it, so a low tunnel is not stepped over; the
        // sight's own depths confirm a point a column says is open.
        let open = self
            .sight
            .columns
            .walk(point, *away, length, RIM_LENGTH, |behind| {
                self.sight.reaches(behind)
            });
        (open / RIM_LENGTH).min(1.0)
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
                sight.lean,
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
        let previous = self
            .previous
            .as_ref()
            .filter(|previous| !previous.is_empty());
        let none = Sight::default();
        let before = previous.unwrap_or(&none);
        floats(&mut bytes, before.origin.extend(before.reach).to_array());
        let azimuths = previous.map_or(0, |previous| previous.azimuths);
        for count in [azimuths, before.elevations, 0, 0] {
            bytes.extend_from_slice(&count.to_le_bytes());
        }
        floats(
            &mut bytes,
            [
                self.fade.clamp(0.0, 1.0),
                before.lean,
                if self.only_seen { 1.0 } else { 0.0 },
                0.0,
            ],
        );
        floats(&mut bytes, self.rim_colour.to_f32_array());
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
        let previous = self.previous.as_ref();
        for index in 0..MAX_DIRECTIONS {
            let depth = previous
                .and_then(|previous| previous.depths.get(index))
                .copied()
                .unwrap_or(0.0);
            bytes.extend_from_slice(&depth.to_le_bytes());
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
            .add_systems(
                PostUpdate,
                (
                    write_params,
                    cull_beyond_reach
                        .in_set(VisibilitySystems::CheckVisibility)
                        .after(check_visibility_cpu_culling),
                ),
            );
    }
}

/// Take what lies wholly past the sight's reach out of the views the
/// cutaway applies to, before anything is queued for them.
///
/// The shader cuts every fragment further than the reach across the ground
/// from the sight's origin (the level's rim), but each mesh out there would
/// still be extracted, batched and drawn for nothing; on a table showing a
/// circle of a large level, that is most of it. A mesh is dropped only when
/// its bounding sphere is wholly past the rim, so nothing that could show a
/// fragment goes, and never a [`KEEP`] one. Views the cutaway leaves whole
/// (the flat window, the sensor) keep everything.
#[allow(clippy::type_complexity)]
fn cull_beyond_reach(
    cutaway: Res<Cutaway>,
    mut views: Query<(&Camera, &GlobalTransform, &mut VisibleEntities)>,
    meshes: Query<(&Aabb, &GlobalTransform, Option<&MeshTag>), With<MeshMaterial3d<ModelMaterial>>>,
) {
    if !cutaway.enabled || cutaway.sight.is_empty() {
        return;
    }
    let (origin, reach) = (cutaway.sight.origin, cutaway.sight.reach);
    for (camera, view, mut visible) in &mut views {
        let scale = view.affine().matrix3.x_axis.length();
        if !camera.is_active || scale < cutaway.min_view_scale {
            continue;
        }
        visible.get_mut(TypeId::of::<Mesh3d>()).retain(|&entity| {
            let Ok((aabb, transform, tag)) = meshes.get(entity) else {
                return true;
            };
            if tag.is_some_and(|tag| tag.0 == KEEP) {
                return true;
            }
            let centre = transform.affine().transform_point3a(aabb.center);
            let radius = transform.radius_vec3a(aabb.half_extents);
            !beyond_reach(origin, reach, centre.into(), radius)
        });
    }
}

/// Whether a sphere at `centre` lies wholly further than `reach` across the
/// ground from `origin`, where the cutaway's rim cuts everything.
fn beyond_reach(origin: Vec3, reach: f32, centre: Vec3, radius: f32) -> bool {
    centre.xz().distance(origin.xz()) - radius > reach
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
    mut written: Local<Option<(Sight, Option<Sight>, Explored)>>,
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
    // Only the fade changes from frame to frame; the large buffer is
    // written when what it holds does.
    let stale = written.as_ref().is_none_or(|(sight, previous, explored)| {
        *sight != cutaway.sight || *previous != cutaway.previous || *explored != cutaway.explored
    });
    if stale && let Some(mut data) = buffers.get_mut(&DATA) {
        data.data = Some(cutaway.data_bytes());
        *written = Some((
            cutaway.sight.clone(),
            cutaway.previous.clone(),
            cutaway.explored.clone(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sphere is past the rim only when none of it is within the reach
    /// across the ground; height does not count.
    #[test]
    fn only_what_lies_wholly_past_the_rim_is_culled() {
        let origin = Vec3::new(10.0, 5.0, 10.0);
        let at = |x: f32, y: f32| Vec3::new(10.0 + x, y, 10.0);
        assert!(!beyond_reach(origin, 100.0, at(50.0, 0.0), 1.0));
        assert!(
            !beyond_reach(origin, 100.0, at(104.0, 0.0), 5.0),
            "straddles the rim"
        );
        assert!(beyond_reach(origin, 100.0, at(106.0, 0.0), 5.0));
        assert!(
            !beyond_reach(origin, 100.0, at(50.0, 400.0), 1.0),
            "high above, but within"
        );
    }

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

    /// A viewpoint 4 m over flat ground at y = 0, and 10 m out along +x a
    /// post `width` metres across standing 6 m tall, with the depths cast
    /// against both.
    fn post_sight(width: f32, lean: f32) -> Sight {
        let mut sight = Sight::open(Vec3::new(0.0, 4.0, 0.0), 400.0, 256, 32);
        sight.lean = lean;
        let (low, high) = (
            Vec3::new(9.5, 0.0, -width / 2.0),
            Vec3::new(10.5, 6.0, width / 2.0),
        );
        for row in 0..sight.elevations {
            for column in 0..sight.azimuths {
                let direction = sight.direction(column, row);
                let mut depth = sight.reach;
                if direction.y < 0.0 {
                    depth = depth.min(4.0 / -direction.y);
                }
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
        sight
    }

    #[test]
    fn a_lean_sees_past_a_post_but_not_a_wall() {
        // Ground 12 m behind a post a metre across: a lean sees round it, so
        // the post throws no shadow at all.
        let behind = Vec3::new(22.0, 0.0, 0.0);
        assert!(!post_sight(1.0, 0.0).sees(behind, Vec3::Y));
        assert!(post_sight(1.0, 2.5).sees(behind, Vec3::Y));
        // A block 4 m across is wider than the lean: behind it a shadow
        // stays, shorter than without the lean.
        assert!(!post_sight(4.0, 2.5).sees(behind, Vec3::Y));
        assert!(!post_sight(4.0, 0.0).sees(Vec3::new(80.0, 0.0, 0.0), Vec3::Y));
        assert!(post_sight(4.0, 2.5).sees(Vec3::new(80.0, 0.0, 0.0), Vec3::Y));
        // A wall 20 m across hides what is behind it from any lean.
        assert!(!post_sight(20.0, 2.5).sees(behind, Vec3::Y));
    }

    /// The lean is measured from what hides the point, not from the
    /// depths' blend: where the blend runs from the post to the ground
    /// behind it, it lies where nothing is, and a lean measured from there
    /// looks aside too little to clear the post, leaving dark lines. With a
    /// lean narrow enough that the difference shows, ground right across
    /// the post's shadow is still seen.
    #[test]
    fn the_lean_is_measured_from_what_hides_the_point() {
        let sight = post_sight(1.0, 0.75);
        for step in 0..=30 {
            let point = Vec3::new(22.0, 0.0, step as f32 * 0.1);
            assert!(sight.sees(point, Vec3::Y), "{point} is dark");
        }
    }

    #[test]
    fn a_lean_sees_a_body_past_a_post() {
        let behind = Vec3::new(25.0, 0.0, 0.0);
        assert!(!post_sight(1.0, 0.0).sees_body(behind, 1.5, 3.0));
        assert!(post_sight(1.0, 2.5).sees_body(behind, 1.5, 3.0));
        assert!(!post_sight(20.0, 2.5).sees_body(behind, 1.5, 3.0));
    }

    /// While the view fades from one sight to the next, how seen a surface
    /// is goes over smoothly rather than at once.
    #[test]
    fn a_new_sight_fades_in_over_the_old() {
        let behind = Vec3::new(22.0, 0.0, 0.0);
        let mut cutaway = Cutaway {
            enabled: true,
            sight: post_sight(1.0, 0.0),
            previous: Some(post_sight(1.0, 6.0)),
            fade: 0.0,
            ..default()
        };
        let before = cutaway.previous.as_ref().unwrap().seen(behind, Vec3::Y);
        let after = cutaway.sight.seen(behind, Vec3::Y);
        assert!(before > 0.9 && after < 0.1);
        assert!((cutaway.seen(behind, Vec3::Y) - before).abs() < 1e-6);
        cutaway.fade = 0.5;
        assert!((cutaway.seen(behind, Vec3::Y) - (before + after) / 2.0).abs() < 1e-6);
        cutaway.fade = 1.0;
        assert!((cutaway.seen(behind, Vec3::Y) - after).abs() < 1e-6);
    }

    /// Every column a line crosses is tried: a stretch of open space far
    /// smaller than the line, deep along it, is found.
    #[test]
    fn the_walk_finds_a_narrow_span_deep_along_the_line() {
        let mut columns = OpenColumns::around(Vec3::ZERO, 400.0);
        // A tunnel 3 m tall, 40 m down, 60 m along x.
        columns.add_ray(Vec3::new(55.0, -39.0, 0.0), Vec3::X, 10.0);
        columns.add_ray(Vec3::new(55.0, -41.5, 0.0), Vec3::X, 10.0);
        let down = Vec3::new(1.5, -1.0, 0.0).normalize();
        let mut found = None;
        let open = columns.walk(Vec3::ZERO, down, 200.0, 100.0, |point| {
            found = found.or(Some(point));
            true
        });
        let point = found.unwrap();
        assert!((-41.5..=-39.0).contains(&point.y), "{point}");
        // The line crosses the tunnel's 2.5 to 3 m of height at a slant.
        assert!(open > 2.0 && open < 8.0, "{open}");
        // Nothing open along a line that misses it.
        let missed = columns.walk(
            Vec3::ZERO,
            Vec3::new(-1.5, -1.0, 0.0).normalize(),
            200.0,
            100.0,
            |_| true,
        );
        assert_eq!(missed, 0.0);
    }

    #[test]
    fn seeing_only_what_is_seen_cuts_the_rest() {
        let mut cutaway = cutaway();
        let beyond = Vec3::new(-30.0, 0.0, 0.0);
        cutaway.explored.mark(beyond.xz());
        assert!(!cutaway.cuts(beyond, Vec3::Y));
        cutaway.only_seen = true;
        assert!(cutaway.cuts(beyond, Vec3::Y));
        // What it sees is still drawn.
        assert!(!cutaway.cuts(Vec3::new(1.0, 0.0, 6.0), Vec3::Y));
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
