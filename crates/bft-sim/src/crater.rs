//! Craters: the ground a shell lands on is cut open and a bowl dropped in.
//!
//! # What the data says
//!
//! Six weapons carry `CRATER [a, b]` in `weapons.zrd` — the HE mortar, both
//! HE mines, the tether-guided missile and both nukes. The molecular
//! distortion variants carry `QUICKSAND` instead (not built here). Each
//! mission's `declient.zrd` configures the "deform client":
//!
//! ```text
//! CRATER  DEFAULT_TEXTURE [cratersand]  DEFAULT_ANIM [he_dirt.flt]
//!         TEXTURE_PARMS [1, 1]  SLOPE [15]  RADIUS [25]  DEPTH [1.7]
//!         POINTS [6]  TEXTURE_ANIM [mnsroad3 [craterroad] …]
//! ```
//!
//! `TEXTURE_ANIM` maps the texture of the ground that was hit to the crater
//! texture drawn in the hole, and optionally to a dirt anim other than the
//! default (m3's lava throws `lav_dirt.flt`). The crater textures are 32×32
//! tiles of a scorch blob that fades into the ground they replace
//! (`cratergrass` into `grass01`), so one tile spans the whole crater.
//!
//! The manual says why craters matter: "you can use the Mortar Cannon to
//! make craters, then hide in them": they are holes, not decals.
//!
//! # What this reads the numbers as
//!
//! None of the four scalars is settled; every reading below is an
//! `// approximation` and is marked where it is used.
//!
//! - weapon `CRATER`'s first number is the radius in metres, and its second
//!   the depth in decimetres (HEM `[10, 20]`: 10 m across the rim, 2 m
//!   deep). The second is always the larger, so it cannot be metres of depth.
//! - `declient.zrd`'s `RADIUS` and `DEPTH` cap them, so a nuke's 30 m is
//!   held to 25 m and every depth to 1.7 m.
//! - `SLOPE` is the crater wall's angle in degrees: at 15° a 1.7 m hole has
//!   a 6.3 m wall and a flat floor inside it.
//! - `POINTS` is how many sides the rim has. It is 6 in every mission, and
//!   the original's craters are hexagons: a flat floor and six walls (as
//!   recalled from the original).
//!
//! # How the cut works
//!
//! [`cut_crater`] takes a node's triangles (the same [`Triangulated`] its
//! collider and its meshes are built from) and returns new ones: every ground
//! triangle under the rim is split into the part outside the rim, which keeps
//! its material, and the part inside, which is lowered by the bowl's profile
//! and goes into a group of its own for the crater texture. The rim is a
//! regular polygon and the profile is linear in the polygon's own radius, so
//! within one sector-and-ring cell the drop is a plane — and so is a ground
//! triangle. Clipping each triangle to each cell therefore represents the
//! bowl exactly, with no extra vertices, and a bowl spanning two terrain
//! nodes meets itself at their shared edge because the height is continuous.
//!
//! # Which ground
//!
//! Only nodes whose gamez flags carry `can_modify` are cut. Soil cannot say
//! it: every textured material in all 13 levels is `Default` soil bar the
//! liquids, so concrete and sand read the same. `can_modify` is on a few
//! dozen `gNNN` nodes per level, and those draw with sand, dirt, grass, dirt
//! roads, lava, snow and ice — nearly every ground texture `TEXTURE_ANIM`
//! names is on them, and none of m1's docks, walls or metal is. m12 has no
//! such node, so it takes no craters. That matches the original's footage,
//! where concrete and metal stay whole.
//!
//! Within that ground, a node with a steep triangle under the rim is left
//! alone, and a crater whose hit node is like that does not form, which is
//! the "Clip Failed" case. [`CraterOverrides`] adds ground the flags leave out.

use std::sync::Arc;

use avian3d::prelude::*;
use bevy::math::Affine3A;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::ReaderValue;
use bft_assets::geometry::{TriangleGroup, Triangulated, material_per_triangle};
use bft_assets::materials::{MaterialDesc, Soil};
use bft_assets::models::ModelType;
use bft_assets::textures::texture_key;

use crate::anim::CallAnim;
use crate::level::GameLayer;
use crate::physics::SurfaceInfo;
use crate::schedule::SimSet;
use crate::weapons::{ProjectileHit, Surface, WeaponTable};

/// The mission reader entry the crater settings are in.
pub const DECLIENT_ENTRY: &str = "declient.zrd";

/// `POINTS` where `declient.zrd` does not say, and the fewest a rim can have.
pub const DEFAULT_POINTS: usize = 6;
const MIN_POINTS: usize = 3;

/// The steepest ground a crater cuts, in degrees.
///
/// // approximation: no reader field says it. Steep enough for a hillside,
/// shallow enough that a wall or a crate's side never counts as ground.
pub const GROUND_SLOPE_LIMIT_DEG: f32 = 35.0;

/// How far off the crater's own height a ground triangle may be and still be
/// cut, as a floor under the gentlest reach: see [`Crater::band`].
pub const MIN_BAND: f32 = 2.0;

/// `declient.zrd`'s `CRATER` block for one mission.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct CraterConfig {
    /// `DEFAULT_TEXTURE`: the crater texture for ground `texture_anim` does
    /// not name.
    pub default_texture: Option<String>,
    /// `DEFAULT_ANIM`: the dirt thrown up, an anim definition name
    /// (`he_dirt.flt`).
    pub default_anim: Option<String>,
    /// `TEXTURE_PARMS`: how many times the crater texture repeats across the
    /// crater, in u and v. `[1, 1]` everywhere.
    pub uv_scale: Vec2,
    /// `SLOPE`, degrees: the wall angle (see the module docs).
    pub slope_deg: f32,
    /// `RADIUS`, metres: the largest crater.
    pub max_radius: f32,
    /// `DEPTH`, metres: the deepest crater.
    pub max_depth: f32,
    /// `POINTS`: how many sides the rim has.
    pub points: usize,
    /// `TEXTURE_ANIM`: ground texture key → crater texture, and the dirt anim
    /// when it is not the default.
    pub texture_anim: HashMap<String, (String, Option<String>)>,
}

impl Default for CraterConfig {
    /// // approximation: no source says what happens without a
    /// `declient.zrd`, so these are the values every mission's file shares,
    /// and no texture.
    fn default() -> CraterConfig {
        CraterConfig {
            default_texture: None,
            default_anim: Some("he_dirt.flt".to_string()),
            uv_scale: Vec2::ONE,
            slope_deg: 15.0,
            max_radius: 25.0,
            max_depth: 1.7,
            points: DEFAULT_POINTS,
            texture_anim: HashMap::default(),
        }
    }
}

impl CraterConfig {
    /// Read the `CRATER` block of a mission's `declient.zrd`.
    ///
    /// `None` when the archive has no such entry or it has no `CRATER`; a
    /// key missing inside the block keeps its [`Default`].
    pub fn from_reader(reader: &bft_assets::ReaderData) -> Option<CraterConfig> {
        let crater = reader.read(DECLIENT_ENTRY)?.get("CRATER")?;
        Some(CraterConfig::parse(crater))
    }

    fn parse(crater: ReaderValue<'_>) -> CraterConfig {
        let defaults = CraterConfig::default();
        let float = |key: &str| crater.get(key).and_then(|v| v.float());
        let text = |key: &str| crater.get(key).and_then(|v| v.string()).map(str::to_string);

        let texture_anim = crater
            .get("TEXTURE_ANIM")
            .map(|map| {
                map.pairs()
                    .into_iter()
                    .filter_map(|(ground, value)| {
                        let names = value.strings();
                        let texture = texture_key(names.first()?);
                        let anim = names.get(1).map(|anim| anim.to_string());
                        Some((texture_key(ground), (texture, anim)))
                    })
                    .collect()
            })
            .unwrap_or_default();

        CraterConfig {
            default_texture: text("DEFAULT_TEXTURE").map(|name| texture_key(&name)),
            default_anim: text("DEFAULT_ANIM").or(defaults.default_anim),
            uv_scale: crater
                .get("TEXTURE_PARMS")
                .and_then(|v| v.float_array::<2>())
                .map(Vec2::from)
                .unwrap_or(defaults.uv_scale),
            slope_deg: float("SLOPE").unwrap_or(defaults.slope_deg),
            max_radius: float("RADIUS").unwrap_or(defaults.max_radius),
            max_depth: float("DEPTH").unwrap_or(defaults.max_depth),
            points: crater
                .get("POINTS")
                .and_then(|v| v.int())
                .map_or(defaults.points, |points| {
                    (points.max(MIN_POINTS as i64)) as usize
                }),
            texture_anim,
        }
    }

    /// The crater texture and dirt anim for ground drawn with `ground`.
    pub fn look_for(&self, ground: Option<&str>) -> (Option<String>, Option<String>) {
        match ground.and_then(|ground| self.texture_anim.get(&texture_key(ground))) {
            Some((texture, anim)) => (
                Some(texture.clone()),
                anim.clone().or_else(|| self.default_anim.clone()),
            ),
            None => (self.default_texture.clone(), self.default_anim.clone()),
        }
    }

    /// The crater a weapon's `CRATER [a, b]` makes at `center`.
    ///
    /// `a` is metres of radius and `b` decimetres of depth, both capped by
    /// this mission's `RADIUS` and `DEPTH` (see the module docs for why both
    /// readings are approximations).
    pub fn crater(&self, center: Vec3, weapon: (u32, u32), ground: Option<&str>) -> Crater {
        let (texture, _) = self.look_for(ground);
        let radius = (weapon.0 as f32).min(self.max_radius).max(0.0);
        let depth = (weapon.1 as f32 / 10.0).min(self.max_depth).max(0.0);
        let run = depth / self.slope_deg.clamp(1.0, 89.0).to_radians().tan();

        Crater {
            center,
            radius,
            floor_radius: (radius - run).max(0.0),
            depth,
            sides: self.points.max(MIN_POINTS),
            texture,
            uv_scale: self.uv_scale,
        }
    }
}

/// One crater, in world space.
#[derive(Debug, Clone, PartialEq)]
pub struct Crater {
    /// Where the rim is centred, on the ground as it was.
    pub center: Vec3,
    /// The rim's circumradius, metres.
    pub radius: f32,
    /// Where the wall meets the flat floor. Zero for a cone.
    pub floor_radius: f32,
    /// How far the floor is below the ground it replaced.
    pub depth: f32,
    /// How many sides the rim has: `POINTS`.
    pub sides: usize,
    /// The crater texture key, drawn over the whole bowl.
    pub texture: Option<String>,
    /// `TEXTURE_PARMS`.
    pub uv_scale: Vec2,
}

impl Crater {
    /// The unit direction of spoke `i` in the xz plane, as `(x, z)`.
    fn spoke(&self, i: usize) -> Vec2 {
        let angle = (i % self.sides) as f32 * std::f32::consts::TAU / self.sides as f32;
        Vec2::new(angle.cos(), angle.sin())
    }

    /// Where spoke `i` crosses a ring of polygon radius `r`, as `(x, z)`.
    fn corner(&self, i: usize, r: f32) -> Vec2 {
        self.center.xz() + self.spoke(i) * r
    }

    /// The polygon radius of a point: `R` on the rim, linear inside each
    /// sector, so the drop is a plane within each cell.
    fn polygon_radius(&self, at: Vec2) -> f32 {
        let offset = at - self.center.xz();
        let apothem = (std::f32::consts::PI / self.sides as f32).cos();

        (0..self.sides)
            .map(|i| {
                let angle = (i as f32 + 0.5) * std::f32::consts::TAU / self.sides as f32;
                offset.dot(Vec2::new(angle.cos(), angle.sin()))
            })
            .fold(f32::MIN, f32::max)
            / apothem
    }

    /// How far the ground at `at` is lowered.
    pub fn drop_at(&self, at: Vec2) -> f32 {
        let r = self.polygon_radius(at);
        if r >= self.radius {
            0.0
        } else if r <= self.floor_radius {
            self.depth
        } else {
            self.depth * (self.radius - r) / (self.radius - self.floor_radius)
        }
    }

    /// How far above or below the crater a ground plane may pass at its
    /// centre and still be the ground it cut: a hillside rises across the
    /// rim, a bridge deck overhead should not sink.
    pub fn band(&self) -> f32 {
        (self.radius * GROUND_SLOPE_LIMIT_DEG.to_radians().tan()).max(MIN_BAND)
    }

    /// The crater texture's coordinates at a point: one tile over the rim's
    /// bounding square.
    fn uv_at(&self, at: Vec2) -> Vec2 {
        let span = (self.radius * 2.0).max(f32::EPSILON);
        ((at - self.center.xz()) / span + Vec2::splat(0.5)) * self.uv_scale
    }

    /// The rim, as convex polygon corners.
    fn rim(&self) -> Vec<Vec2> {
        (0..self.sides)
            .map(|i| self.corner(i, self.radius))
            .collect()
    }

    /// The cells the bowl is cut into: per sector, the floor wedge and the
    /// wall band, each a convex polygon over which the drop is a plane.
    fn cells(&self) -> Vec<Vec<Vec2>> {
        let mut cells = Vec::with_capacity(self.sides * 2);
        let centre = self.center.xz();
        let floor = self.floor_radius;

        for i in 0..self.sides {
            let (a, b) = (i, i + 1);
            if floor > 1e-3 {
                cells.push(vec![centre, self.corner(a, floor), self.corner(b, floor)]);
                cells.push(vec![
                    self.corner(a, floor),
                    self.corner(a, self.radius),
                    self.corner(b, self.radius),
                    self.corner(b, floor),
                ]);
            } else {
                cells.push(vec![
                    centre,
                    self.corner(a, self.radius),
                    self.corner(b, self.radius),
                ]);
            }
        }

        cells
    }
}

// ---------------------------------------------------------------------------
// The cut
// ---------------------------------------------------------------------------

/// A node's triangles after its craters: what its collider and its meshes
/// are both rebuilt from.
#[derive(Debug, Clone)]
pub struct CutModel {
    pub triangulated: Triangulated,
    /// Parallel to `triangulated.groups`: which of the node's craters a
    /// group is the bowl of, or `None` for ground that kept its material.
    pub crater_of_group: Vec<Option<usize>>,
}

/// One corner being clipped: world position, the node-local normal and uv.
#[derive(Debug, Clone, Copy)]
struct Corner {
    world: Vec3,
    normal: Vec3,
    uv: Vec2,
}

impl Corner {
    fn lerp(self, other: Corner, t: f32) -> Corner {
        Corner {
            world: self.world.lerp(other.world, t),
            normal: self.normal.lerp(other.normal, t),
            uv: self.uv.lerp(other.uv, t),
        }
    }
}

/// A half-plane in xz: points with `normal·(p − origin) ≥ 0` are inside.
#[derive(Debug, Clone, Copy)]
struct HalfPlane {
    origin: Vec2,
    normal: Vec2,
}

impl HalfPlane {
    fn side(&self, at: Vec3) -> f32 {
        (at.xz() - self.origin).dot(self.normal)
    }

    fn flipped(self) -> HalfPlane {
        HalfPlane {
            origin: self.origin,
            normal: -self.normal,
        }
    }
}

/// A convex polygon's edges as inward half-planes, whichever way it winds.
fn half_planes(polygon: &[Vec2]) -> Vec<HalfPlane> {
    let mid = polygon.iter().copied().sum::<Vec2>() / polygon.len() as f32;

    (0..polygon.len())
        .map(|i| {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            let mut normal = (b - a).perp();
            if (mid - a).dot(normal) < 0.0 {
                normal = -normal;
            }
            HalfPlane {
                origin: a,
                normal: normal.normalize_or_zero(),
            }
        })
        .collect()
}

/// Sutherland–Hodgman against one half-plane, keeping the polygon's order.
fn clip(polygon: &[Corner], plane: HalfPlane) -> Vec<Corner> {
    let mut out = Vec::with_capacity(polygon.len() + 2);

    for i in 0..polygon.len() {
        let current = polygon[i];
        let next = polygon[(i + 1) % polygon.len()];
        let (a, b) = (plane.side(current.world), plane.side(next.world));

        if a >= 0.0 {
            out.push(current);
        }
        if (a >= 0.0) != (b >= 0.0) {
            out.push(current.lerp(next, a / (a - b)));
        }
    }

    out
}

fn clip_all(polygon: &[Corner], planes: &[HalfPlane]) -> Vec<Corner> {
    planes.iter().fold(polygon.to_vec(), |polygon, plane| {
        if polygon.len() < 3 {
            polygon
        } else {
            clip(&polygon, *plane)
        }
    })
}

/// Twice the xz area of a polygon, unsigned: the degenerate-piece test.
fn xz_area(polygon: &[Corner]) -> f32 {
    let mut sum = 0.0;
    for i in 0..polygon.len() {
        let (a, b) = (
            polygon[i].world.xz(),
            polygon[(i + 1) % polygon.len()].world.xz(),
        );
        sum += a.perp_dot(b);
    }
    sum.abs()
}

/// A piece smaller than this, in square metres (doubled), is dropped.
const MIN_PIECE_AREA: f32 = 1e-6;

/// The triangles being built.
struct Builder<'a> {
    source: &'a Triangulated,
    from_world: Affine3A,
    normal_to_local: Mat3,
    out: Triangulated,
    crater_of_group: Vec<Option<usize>>,
    /// Bowl groups by source material.
    bowls: HashMap<u32, usize>,
}

impl Builder<'_> {
    /// Append a convex piece as a fan into `group`.
    fn push_piece(&mut self, piece: &[Corner], group: usize) {
        // Clipping leaves repeated and collinear corners where an edge passes
        // through a vertex; dropped here, or the fan below makes zero-area
        // triangles whose facing is noise.
        let mut piece: Vec<Corner> = piece.to_vec();
        piece.dedup_by(|a, b| a.world.xz().distance_squared(b.world.xz()) < MIN_PIECE_AREA);
        while piece.len() > 1
            && piece[0]
                .world
                .xz()
                .distance_squared(piece[piece.len() - 1].world.xz())
                < MIN_PIECE_AREA
        {
            piece.pop();
        }
        let mut i = 0;
        while piece.len() >= 3 && i < piece.len() {
            let n = piece.len();
            let (a, b, c) = (
                piece[(i + n - 1) % n].world.xz(),
                piece[i].world.xz(),
                piece[(i + 1) % n].world.xz(),
            );
            let (ab, bc) = (b - a, c - b);
            if ab.perp_dot(bc).abs() <= 1e-4 * ab.length() * bc.length() {
                piece.remove(i);
            } else {
                i += 1;
            }
        }
        if piece.len() < 3 || xz_area(&piece) < MIN_PIECE_AREA {
            return;
        }
        let piece = piece.as_slice();
        let base = self.out.positions.len() as u32;
        for corner in piece {
            self.out
                .positions
                .push(self.from_world.transform_point3(corner.world));
            self.out.normals.push(corner.normal.normalize_or(Vec3::Y));
            self.out.uvs.push(corner.uv);
            self.out.colors.push([1.0; 4]);
        }
        let indices = &mut self.out.groups[group].indices;
        for i in 1..piece.len() as u32 - 1 {
            indices.extend_from_slice(&[base, base + i, base + i + 1]);
        }
    }

    fn bowl_group(&mut self, material_index: u32, crater: usize) -> usize {
        *self.bowls.entry(material_index).or_insert_with(|| {
            self.out.groups.push(TriangleGroup {
                material_index,
                indices: Vec::new(),
                show_backface: false,
            });
            self.crater_of_group.push(Some(crater));
            self.out.groups.len() - 1
        })
    }

    /// The local normal of a lowered piece, from its own plane.
    fn piece_normal(&self, piece: &[Corner]) -> Vec3 {
        let world = (piece[1].world - piece[0].world)
            .cross(piece[2].world - piece[0].world)
            .normalize_or(Vec3::Y);
        let world = if world.y < 0.0 { -world } else { world };
        (self.normal_to_local * world).normalize_or(Vec3::Y)
    }
}

/// Cut one crater into a node's triangles.
///
/// `source` is the node's current triangles (its model's, or the result of
/// its last cut) and `crater_of_group` its group tags; `to_world` is the
/// node's transform. `crater` is the index the new crater will have in the
/// node's list, which the bowl group is tagged with.
///
/// `None` when the node is not ground under this crater: no triangle of it
/// lies under the rim, or one that does is steeper than
/// [`GROUND_SLOPE_LIMIT_DEG`].
pub fn cut_crater(
    source: &Triangulated,
    crater_of_group: &[Option<usize>],
    to_world: &Affine3A,
    crater: &Crater,
    crater_index: usize,
) -> Option<CutModel> {
    let rim = half_planes(&crater.rim());
    let cells: Vec<Vec<HalfPlane>> = crater.cells().iter().map(|c| half_planes(c)).collect();
    let min_up = GROUND_SLOPE_LIMIT_DEG.to_radians().cos();
    let band = crater.band();

    let mut builder = Builder {
        source,
        from_world: to_world.inverse(),
        normal_to_local: Mat3::from(to_world.matrix3).transpose(),
        out: Triangulated {
            positions: source.positions.clone(),
            normals: source.normals.clone(),
            uvs: source.uvs.clone(),
            colors: source.colors.clone(),
            groups: source
                .groups
                .iter()
                .map(|group| TriangleGroup {
                    indices: Vec::with_capacity(group.indices.len()),
                    ..group.clone()
                })
                .collect(),
            skipped_polygons: source.skipped_polygons,
        },
        crater_of_group: (0..source.groups.len())
            .map(|g| crater_of_group.get(g).copied().flatten())
            .collect(),
        bowls: HashMap::default(),
    };
    let mut touched = false;

    for (g, group) in source.groups.iter().enumerate() {
        let (triangles, _) = group.indices.as_chunks::<3>();
        for tri in triangles {
            let corners = tri.map(|i| {
                let i = i as usize;
                Corner {
                    world: to_world.transform_point3(builder.source.positions[i]),
                    normal: builder.source.normals.get(i).copied().unwrap_or(Vec3::Y),
                    uv: builder.source.uvs.get(i).copied().unwrap_or_default(),
                }
            });

            let inner = clip_all(&corners, &rim);
            if inner.len() < 3 || xz_area(&inner) < MIN_PIECE_AREA {
                builder.out.groups[g].indices.extend_from_slice(tri);
                continue;
            }

            let cross =
                (corners[1].world - corners[0].world).cross(corners[2].world - corners[0].world);
            if cross.length_squared() < MIN_PIECE_AREA * MIN_PIECE_AREA {
                // A degenerate triangle has no side to be on.
                builder.out.groups[g].indices.extend_from_slice(tri);
                continue;
            }
            let face = cross.normalize();
            if face.y < min_up {
                // A wall, a crate's side, an overhang: whether it is in the
                // way depends on whether it reaches the crater's height.
                let low = corners.iter().map(|c| c.world.y).fold(f32::MAX, f32::min);
                let high = corners.iter().map(|c| c.world.y).fold(f32::MIN, f32::max);
                if high >= crater.center.y - band && low <= crater.center.y + band {
                    return None;
                }
                builder.out.groups[g].indices.extend_from_slice(tri);
                continue;
            }
            // The plane's height under the crater's centre.
            let offset = crater.center.xz() - corners[0].world.xz();
            let at_centre = corners[0].world.y - (face.x * offset.x + face.z * offset.y) / face.y;
            if (at_centre - crater.center.y).abs() > band {
                builder.out.groups[g].indices.extend_from_slice(tri);
                continue;
            }

            touched = true;

            // Outside the rim: T ∩ ¬H_i ∩ H_0 … H_{i−1}, one convex piece per
            // rim edge, which together are exactly T minus the rim.
            for i in 0..rim.len() {
                let mut piece = clip(&corners, rim[i].flipped());
                for plane in &rim[..i] {
                    if piece.len() < 3 {
                        break;
                    }
                    piece = clip(&piece, *plane);
                }
                builder.push_piece(&piece, g);
            }

            // Inside: one piece per cell, lowered and re-textured.
            let bowl = builder.bowl_group(group.material_index, crater_index);
            for cell in &cells {
                let mut piece = clip_all(&inner, cell);
                if piece.len() < 3 {
                    continue;
                }
                for corner in &mut piece {
                    corner.world.y -= crater.drop_at(corner.world.xz());
                    corner.uv = crater.uv_at(corner.world.xz());
                }
                let normal = builder.piece_normal(&piece);
                for corner in &mut piece {
                    corner.normal = normal;
                }
                builder.push_piece(&piece, bowl);
            }
        }
    }

    touched.then_some(CutModel {
        triangulated: builder.out,
        crater_of_group: builder.crater_of_group,
    })
}

// ---------------------------------------------------------------------------
// Components and the system
// ---------------------------------------------------------------------------

/// The level's materials, as far as the sim needs them: soil for the rebuilt
/// collider's `SurfaceInfo`, and texture for choosing a crater's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelSurfaces {
    pub soils: Vec<Soil>,
    pub textures: Vec<Option<String>>,
}

impl LevelSurfaces {
    /// From the level's material table.
    pub fn from_materials(materials: &[MaterialDesc]) -> LevelSurfaces {
        LevelSurfaces {
            soils: materials.iter().map(MaterialDesc::soil).collect(),
            textures: materials
                .iter()
                .map(|material| material.texture().map(str::to_string))
                .collect(),
        }
    }
}

/// `assets/tuning/craters.ron`: pieces of level geometry that take craters
/// although their flags do not say `can_modify`.
///
/// Per level directory name (`"m1"`), the node names to add. Empty is the
/// default and the data's own rule. It exists for ground the data leaves out
/// that plainly is ground — m1's sand road off the pier, `g943`, lies on the
/// deformable sand and is drawn in sand-road texture, but is not flagged. A
/// rule by texture was tried instead and rejected: across the 13 levels it
/// pulls in tunnel floors, rooftops, bridges and canyon rock.
#[derive(Asset, TypePath, serde::Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CraterOverrides {
    pub extra_ground: HashMap<String, Vec<String>>,
}

/// What a level node's collider was built from, kept so a crater can cut it.
///
/// On every colliding model node `spawn_level` makes whose flags say
/// `can_modify`: the deformable ground (see the module docs). A node without
/// it is never cut. The triangulation is
/// shared between the nodes that instance one model.
#[derive(Component, Debug, Clone)]
pub struct GroundSource {
    pub model_type: ModelType,
    pub triangulated: Arc<Triangulated>,
    pub surfaces: Arc<LevelSurfaces>,
}

impl GroundSource {
    /// What a node drawing `model` needs to take craters.
    pub fn for_model(
        model: &bft_assets::models::ModelDesc,
        axes: &bft_assets::axes::AxisConvention,
        surfaces: Arc<LevelSurfaces>,
    ) -> GroundSource {
        GroundSource {
            model_type: model.model_type,
            triangulated: Arc::new(bft_assets::geometry::triangulate(model, axes)),
            surfaces,
        }
    }
}

/// A node's craters, and its triangles with them cut in.
///
/// `bft-game` rebuilds the node's meshes from `cut` whenever this changes,
/// so the hole it draws is the hole the collider has.
#[derive(Component, Debug, Clone)]
pub struct Cratered {
    pub craters: Vec<Crater>,
    pub cut: Arc<CutModel>,
}

/// A crater formed.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct CraterFormed {
    pub crater: Crater,
    /// Every node it was cut into.
    pub nodes: Vec<Entity>,
}

/// Cut a crater where a `CRATER` weapon hit the ground.
///
/// The hit is re-probed along its own normal to find the ground triangle,
/// since a shell's `ProjectileHit` names the collider but not the face — and
/// a mine's, which is on the ground already, names nothing.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn form_craters(
    mut commands: Commands,
    mut hits: MessageReader<ProjectileHit>,
    mut formed: MessageWriter<CraterFormed>,
    mut calls: MessageWriter<CallAnim>,
    table: Option<Res<WeaponTable>>,
    config: Option<Res<CraterConfig>>,
    spatial: SpatialQuery,
    ground: Query<(
        Entity,
        &GroundSource,
        Option<&Cratered>,
        &GlobalTransform,
        &Collider,
    )>,
) {
    let Some(table) = table else {
        hits.clear();
        return;
    };
    let default_config = CraterConfig::default();
    let config = config.as_deref().unwrap_or(&default_config);

    for hit in hits.read() {
        let Some(size) = table.by_index(hit.weapon).and_then(|def| def.crater) else {
            continue;
        };
        if hit.surface != Surface::Default {
            continue;
        }

        // The ground under the hit, and which of its triangles.
        let normal = hit.normal.normalize_or(Vec3::Y);
        let origin = hit.at + normal * 0.5;
        let Ok(down) = Dir3::new(-normal) else {
            continue;
        };
        let filter = SpatialQueryFilter::from_mask(GameLayer::Level);
        let Some(ray) = spatial.cast_ray(origin, down, 2.0, true, &filter) else {
            continue;
        };
        let Ok((hit_node, source, cratered, transform, collider)) = ground.get(ray.entity) else {
            continue;
        };
        let point = origin + *down * ray.distance;

        let current = cratered.map_or(&*source.triangulated, |c| &c.cut.triangulated);
        let material = crate::physics::hit_triangle(collider, transform, origin, down, 2.0)
            .and_then(|triangle| {
                material_per_triangle(source.model_type, current)
                    .get(triangle)
                    .copied()
            });
        let texture = material
            .and_then(|m| source.surfaces.textures.get(m as usize))
            .and_then(|t| t.as_deref());

        let crater = config.crater(point, size, texture);
        if crater.radius <= 0.0 {
            continue;
        }

        // Every ground node under the rim, the hit one first: if that one
        // will not take the crater, none does.
        let reach = Vec3::new(crater.radius, crater.band() + crater.depth, crater.radius);
        let mut cuts = Vec::new();
        let mut candidates: Vec<_> = ground
            .iter()
            .filter(|(entity, ..)| *entity != hit_node)
            .collect();
        candidates.insert(0, ground.get(hit_node).expect("fetched above"));

        for (entity, source, cratered, transform, _) in candidates {
            if entity != hit_node && !aabb_near(source, transform, point, reach) {
                continue;
            }
            let (current, tags, count) = match cratered {
                Some(c) => (
                    &c.cut.triangulated,
                    c.cut.crater_of_group.as_slice(),
                    c.craters.len(),
                ),
                None => (&*source.triangulated, &[][..], 0),
            };
            let cut = cut_crater(current, tags, &transform.affine(), &crater, count);
            match cut {
                Some(cut) => cuts.push((entity, source, cratered, cut)),
                None if entity == hit_node => break,
                None => {}
            }
        }
        if cuts.first().is_none_or(|(entity, ..)| *entity != hit_node) {
            continue;
        }

        let mut nodes = Vec::with_capacity(cuts.len());
        for (entity, source, cratered, cut) in cuts {
            let mut craters = cratered.map(|c| c.craters.clone()).unwrap_or_default();
            craters.push(crater.clone());

            let (collider, surface, cratered) = cut_components(source, craters, cut);
            let mut entity_commands = commands.entity(entity);
            if let Some(collider) = collider {
                entity_commands.insert(collider);
            }
            if let Some(surface) = surface {
                entity_commands.insert(surface);
            }
            entity_commands.insert(cratered);
            nodes.push(entity);
        }

        let (_, anim) = config.look_for(texture);
        if let Some(anim) = anim {
            let local = transform.affine().inverse().transform_point3(point);
            calls.write(CallAnim::at(anim, hit_node, local));
        }

        formed.write(CraterFormed { crater, nodes });
    }
}

/// What a cut node carries: the rebuilt collider, its soils, and the
/// craters with the cut they made.
///
/// Shared by [`form_craters`] and [`recut`], so a crater put back by a save's
/// restore is the same as one a shell made.
pub fn cut_components(
    source: &GroundSource,
    craters: Vec<Crater>,
    cut: CutModel,
) -> (Option<Collider>, Option<SurfaceInfo>, Cratered) {
    let collider = crate::colliders::collider_from(source.model_type, &cut.triangulated);
    let soils = material_per_triangle(source.model_type, &cut.triangulated)
        .into_iter()
        .map(|m| {
            source
                .surfaces
                .soils
                .get(m as usize)
                .copied()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let surface = (!soils.is_empty()).then(|| SurfaceInfo::new(soils));
    (
        collider,
        surface,
        Cratered {
            craters,
            cut: Arc::new(cut),
        },
    )
}

/// Cut a node's craters again, in order, from its original triangles.
///
/// What a save's restore does with the craters it recorded. A crater that no
/// longer cuts — the level's ground read differently since the save — is
/// left out rather than failing the rest. `None` when none of them cut.
pub fn recut(
    source: &GroundSource,
    to_world: &Affine3A,
    craters: &[Crater],
) -> Option<(Vec<Crater>, CutModel)> {
    let mut applied: Vec<Crater> = Vec::new();
    let mut current: Option<CutModel> = None;

    for crater in craters {
        let (triangles, tags) = match &current {
            Some(cut) => (&cut.triangulated, cut.crater_of_group.as_slice()),
            None => (&*source.triangulated, &[][..]),
        };
        if let Some(cut) = cut_crater(triangles, tags, to_world, crater, applied.len()) {
            current = Some(cut);
            applied.push(crater.clone());
        }
    }

    current.map(|cut| (applied, cut))
}

/// Whether a node's triangles come near a point, by their bounds.
fn aabb_near(source: &GroundSource, transform: &GlobalTransform, point: Vec3, reach: Vec3) -> bool {
    let affine = transform.affine();
    let mut min = Vec3::MAX;
    let mut max = Vec3::MIN;
    for p in &source.triangulated.positions {
        let p = affine.transform_point3(*p);
        min = min.min(p);
        max = max.max(p);
    }
    (point - reach).cmple(max).all() && (point + reach).cmpge(min).all()
}

/// The crater plugin.
pub struct CraterPlugin;

impl Plugin for CraterPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CraterFormed>().add_systems(
            FixedUpdate,
            form_craters
                .in_set(SimSet::Damage)
                .after(crate::weapons::firing::resolve_hits),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat square of ground, `half` metres each way, as two triangles.
    fn flat(half: f32, y: f32) -> Triangulated {
        Triangulated {
            positions: vec![
                Vec3::new(-half, y, -half),
                Vec3::new(-half, y, half),
                Vec3::new(half, y, half),
                Vec3::new(half, y, -half),
            ],
            normals: vec![Vec3::Y; 4],
            uvs: vec![Vec2::ZERO, Vec2::Y, Vec2::ONE, Vec2::X],
            colors: vec![[1.0; 4]; 4],
            groups: vec![TriangleGroup {
                material_index: 3,
                indices: vec![0, 1, 2, 0, 2, 3],
                show_backface: false,
            }],
            skipped_polygons: 0,
        }
    }

    fn crater(at: Vec3) -> Crater {
        CraterConfig::default().crater(at, (10, 20), None)
    }

    fn area_xz(t: &Triangulated, group: usize) -> f32 {
        t.groups[group]
            .indices
            .chunks(3)
            .map(|tri| {
                let [a, b, c] = [0, 1, 2].map(|i| t.positions[tri[i] as usize].xz());
                (b - a).perp_dot(c - a).abs() * 0.5
            })
            .sum()
    }

    /// The reading of `[10, 20]` and the caps: 10 m, 2 m held to 1.7 m, and
    /// a 15° wall.
    #[test]
    fn a_weapons_crater_is_capped_by_the_mission() {
        let c = crater(Vec3::ZERO);
        assert_eq!(c.radius, 10.0);
        assert!((c.depth - 1.7).abs() < 1e-6);
        let run = 1.7 / 15f32.to_radians().tan();
        assert!((c.floor_radius - (10.0 - run)).abs() < 1e-4);

        let nuke = CraterConfig::default().crater(Vec3::ZERO, (30, 40), None);
        assert_eq!(nuke.radius, 25.0, "RADIUS caps the nuke");
    }

    #[test]
    fn the_drop_is_full_on_the_floor_and_nothing_at_the_rim() {
        let c = crater(Vec3::ZERO);
        assert!((c.drop_at(Vec2::ZERO) - c.depth).abs() < 1e-6);
        assert!(c.drop_at(Vec2::new(c.radius, 0.0)).abs() < 1e-4);
        assert_eq!(c.drop_at(Vec2::new(c.radius + 1.0, 0.0)), 0.0);
        let mid = (c.radius + c.floor_radius) / 2.0;
        assert!((c.drop_at(Vec2::new(mid, 0.0)) - c.depth / 2.0).abs() < 1e-3);
    }

    /// Cutting keeps the ground's area: what leaves the old group is exactly
    /// what the bowl group gains, seen from above.
    #[test]
    fn the_cut_keeps_the_grounds_area() {
        let ground = flat(50.0, 0.0);
        let c = crater(Vec3::ZERO);
        let cut = cut_crater(&ground, &[], &Affine3A::IDENTITY, &c, 0).expect("flat ground cuts");

        let outside = area_xz(&cut.triangulated, 0);
        let bowl = area_xz(&cut.triangulated, 1);
        assert!(
            (outside + bowl - 100.0 * 100.0).abs() < 0.5,
            "{outside} + {bowl}"
        );

        // The bowl is the rim polygon's area.
        let n = c.sides as f32;
        let polygon = 0.5 * n * c.radius * c.radius * (std::f32::consts::TAU / n).sin();
        assert!(
            (bowl - polygon).abs() < 0.05,
            "bowl {bowl}, polygon {polygon}"
        );

        assert_eq!(cut.crater_of_group, vec![None, Some(0)]);
        assert_eq!(
            cut.triangulated.groups[1].material_index, 3,
            "keeps the soil's material"
        );
    }

    /// The original's shape: a hexagonal floor and six walls, seven faces.
    #[test]
    fn a_crater_on_flat_ground_has_a_floor_and_six_walls() {
        let ground = flat(50.0, 0.0);
        let c = crater(Vec3::ZERO);
        assert_eq!(c.sides, 6, "`POINTS 6`");
        let cut = cut_crater(&ground, &[], &Affine3A::IDENTITY, &c, 0).expect("cuts");
        let t = &cut.triangulated;

        let mut planes: Vec<(Vec3, f32)> = Vec::new();
        for tri in t.groups[1].indices.chunks(3) {
            let [a, b, d] = [0, 1, 2].map(|i| t.positions[tri[i] as usize]);
            let n = (b - a).cross(d - a).normalize();
            let plane = (n, n.dot(a));
            if !planes
                .iter()
                .any(|(m, k)| m.dot(n) > 0.9999 && (k - plane.1).abs() < 1e-3)
            {
                planes.push(plane);
            }
        }
        assert_eq!(planes.len(), 7, "{planes:?}");
    }

    /// The bowl is lowered to the profile, and nothing outside it moves.
    #[test]
    fn the_bowl_follows_the_profile_and_the_rest_stays_put() {
        let ground = flat(50.0, 4.0);
        let c = crater(Vec3::new(5.0, 4.0, -3.0));
        let cut = cut_crater(&ground, &[], &Affine3A::IDENTITY, &c, 0).expect("cuts");
        let t = &cut.triangulated;

        for &i in &t.groups[0].indices {
            assert!((t.positions[i as usize].y - 4.0).abs() < 1e-4);
        }
        let mut deepest = 0.0f32;
        for &i in &t.groups[1].indices {
            let p = t.positions[i as usize];
            let wanted = 4.0 - c.drop_at(p.xz());
            assert!((p.y - wanted).abs() < 1e-3, "{p} wanted y {wanted}");
            deepest = deepest.max(4.0 - p.y);
        }
        assert!((deepest - c.depth).abs() < 1e-4);
    }

    /// Winding survives: every piece still faces up, as the ground did.
    #[test]
    fn every_piece_still_faces_up() {
        let ground = flat(30.0, 0.0);
        let cut =
            cut_crater(&ground, &[], &Affine3A::IDENTITY, &crater(Vec3::ZERO), 0).expect("cuts");
        let t = &cut.triangulated;
        let reference = {
            let [a, b, c] =
                [0, 1, 2].map(|i| ground.positions[ground.groups[0].indices[i] as usize]);
            (b - a).cross(c - a).y.signum()
        };
        for group in &t.groups {
            for tri in group.indices.chunks(3) {
                let [a, b, c] = [0, 1, 2].map(|i| t.positions[tri[i] as usize]);
                let n = (b - a).cross(c - a);
                assert_eq!(n.y.signum(), reference, "a flipped piece {a} {b} {c} n={n}");
            }
        }
    }

    /// Ground that does not reach the rim is not touched.
    #[test]
    fn ground_away_from_the_crater_is_not_cut() {
        let ground = flat(5.0, 0.0);
        assert!(
            cut_crater(
                &ground,
                &[],
                &Affine3A::IDENTITY,
                &crater(Vec3::new(100.0, 0.0, 0.0)),
                0
            )
            .is_none()
        );
    }

    /// A wall under the rim stops the cut; a deck far overhead does not
    /// count as ground.
    #[test]
    fn a_wall_under_the_rim_stops_the_cut_and_a_high_deck_is_ignored() {
        let mut wall = flat(5.0, 0.0);
        wall.positions = vec![
            Vec3::new(-5.0, 0.0, 0.0),
            Vec3::new(-5.0, 4.0, 0.0),
            Vec3::new(5.0, 4.0, 0.0),
            Vec3::new(5.0, 0.0, 0.0),
        ];
        assert!(cut_crater(&wall, &[], &Affine3A::IDENTITY, &crater(Vec3::ZERO), 0).is_none());

        let deck = flat(50.0, 30.0);
        assert!(cut_crater(&deck, &[], &Affine3A::IDENTITY, &crater(Vec3::ZERO), 0).is_none());
    }

    /// A second crater cuts into the first: the overlap deepens.
    #[test]
    fn a_second_crater_cuts_the_first() {
        let ground = flat(50.0, 0.0);
        let a = crater(Vec3::ZERO);
        let first = cut_crater(&ground, &[], &Affine3A::IDENTITY, &a, 0).expect("cuts");
        let b = crater(Vec3::new(6.0, 0.0, 0.0));
        let second = cut_crater(
            &first.triangulated,
            &first.crater_of_group,
            &Affine3A::IDENTITY,
            &b,
            1,
        )
        .expect("cuts again");

        let lowest = second
            .triangulated
            .groups
            .iter()
            .flat_map(|g| g.indices.iter())
            .map(|&i| second.triangulated.positions[i as usize].y)
            .fold(f32::MAX, f32::min);
        assert!(
            lowest < -a.depth - 0.5,
            "the overlap is deeper than one crater: {lowest}"
        );
        assert!(second.crater_of_group.contains(&Some(0)));
        assert!(second.crater_of_group.contains(&Some(1)));
    }

    /// A transformed node is cut in world space and stored in its own.
    #[test]
    fn a_moved_node_is_cut_where_it_is_in_the_world() {
        let ground = flat(50.0, 0.0);
        let to_world = Affine3A::from_translation(Vec3::new(1000.0, 20.0, 0.0));
        let c = crater(Vec3::new(1000.0, 20.0, 0.0));
        let cut = cut_crater(&ground, &[], &to_world, &c, 0).expect("cuts");
        let lowest = cut.triangulated.groups[1]
            .indices
            .iter()
            .map(|&i| cut.triangulated.positions[i as usize].y)
            .fold(f32::MAX, f32::min);
        assert!((lowest + c.depth).abs() < 1e-3, "local floor at {lowest}");
    }

    #[test]
    fn declient_reads_its_crater_block() {
        let json: serde_json::Value = serde_json::from_str(
            r#"[["VERSION", [1.0], "CRATER", ["DEFAULT_TEXTURE", ["craterash"],
                "DEFAULT_ANIM", ["ash_dirt.flt"], "TEXTURE_PARMS", [1.0, 1.0],
                "SLOPE", [15.0], "RADIUS", [25.0], "DEPTH", [1.7], "POINTS", [6],
                "TEXTURE_ANIM", ["lavsta1", ["lav00001", "lav_dirt.flt"],
                                 "mnsroad3", ["craterroad"]]],
                "QUICK_SAND", ["SLOPE", [20.0]]]]"#,
        )
        .expect("valid");
        let archive = bft_assets::ReaderData::from_entries(
            [(DECLIENT_ENTRY.to_string(), json)].into_iter().collect(),
        );
        let config = CraterConfig::from_reader(&archive).expect("has CRATER");

        assert_eq!(config.default_texture.as_deref(), Some("craterash"));
        assert_eq!(config.max_depth, 1.7);
        assert_eq!(config.points, 6);
        assert_eq!(
            config.look_for(Some("LAVSTA1.tif")),
            (Some("lav00001".into()), Some("lav_dirt.flt".into()))
        );
        assert_eq!(
            config.look_for(Some("mnsroad3")),
            (Some("craterroad".into()), Some("ash_dirt.flt".into())),
            "a texture with no anim of its own throws the default dirt"
        );
        assert_eq!(
            config.look_for(Some("grass")),
            (Some("craterash".into()), Some("ash_dirt.flt".into()))
        );
    }
}
