// The cutaway in front of the StandardMaterial's fragment shader: see
// cutaway.rs, whose `Cutaway::cuts` this mirrors. Past the cut, this is
// Bevy 0.19's pbr.wgsl without the decal, meshlet and OIT paths, which the
// game does not use.

#import bevy_pbr::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    mesh_view_bindings::view,
    mesh_functions::{get_tag, get_world_from_local},
}

#ifdef PREPASS_PIPELINE
#import bevy_pbr::{
    prepass_io::{VertexOutput, FragmentOutput},
    pbr_deferred_functions::deferred_output,
}
#else
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT,
}
#endif

#ifdef VISIBILITY_RANGE_DITHER
#import bevy_pbr::pbr_functions::visibility_range_dither;
#endif

// `cutaway::MAX_DIRECTIONS`, `MAX_CELLS / 32` and `COLUMNS`.
const MAX_DIRECTIONS: u32 = 8192u;
const MAX_WORDS: u32 = 8192u;
const COLUMNS: u32 = 512u;
const HEIGHT_UNIT: f32 = 0.25;

struct Cutaway {
    // The eye; w is 1 when the cutaway is on.
    eye: vec4<f32>,
    // The sight's origin; w its reach.
    origin: vec4<f32>,
    // Smallest view scale, explored shade, unseen shade.
    shade: vec4<f32>,
    // Azimuths (0: no sight), elevations, explored columns, rows.
    counts: vec4<u32>,
    // Explored corner x, z, cell size.
    grid: vec4<f32>,
    // Open columns' corner x, z, column size.
    columns: vec4<f32>,
}

// Written only when the sight or the explored ground changes.
struct CutawayData {
    depths: array<f32, MAX_DIRECTIONS>,
    explored: array<u32, MAX_WORDS>,
    // Per column: the lowest and highest open height, in HEIGHT_UNITs, as
    // two signed halves, the lowest in the low half.
    spans: array<u32, 262144>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<storage, read> cutaway: Cutaway;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<storage, read> data: CutawayData;

// `cutaway::KEEP`, and the low byte of `cutaway::when_seen`.
const KEEP: u32 = 1u;
const WHEN_SEEN: u32 = 2u;

// The constants of cutaway.rs.
const SEEN_MARGIN: f32 = 1.5;
const SEEN_MARGIN_SHARE: f32 = 0.03;
const SEEN_SOFT: f32 = 3.0;
const SEEN_SOFT_SHARE: f32 = 0.05;
const FACING_AWAY: f32 = -0.3;
const FACING_TOWARD: f32 = 0.2;
const EDGE_FADE: f32 = 0.08;
const REACH_SLACK: f32 = 1.0;
// The grain the edge of what is drawn dissolves in, metres of the level.
const GRAIN: f32 = 0.5;
const HIDE_STEPS: u32 = 8u;
const HIDE_LENGTH: f32 = 300.0;
const HIDE_DEPTH: f32 = 30.0;
const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

// What `judge` returns for a fragment that is not drawn.
const CUT: f32 = -1.0;

fn depth_at(column: u32, row: u32) -> f32 {
    return data.depths[min(row * cutaway.counts.x + column, MAX_DIRECTIONS - 1u)];
}

// `Sight::depth_towards`: how far the sight reaches toward `offset`,
// blended between the four directions round it.
fn depth_towards(offset: vec3<f32>) -> f32 {
    let magnitude = length(offset);
    let azimuths = cutaway.counts.x;
    let elevations = cutaway.counts.y;
    if magnitude <= 1e-6 {
        return cutaway.origin.w;
    }
    let column = (atan2(offset.z, offset.x) + PI) / TAU * f32(azimuths) - 0.5;
    let height = offset.y / magnitude;
    let even = sign(height) * pow(abs(height), 1.0 / 3.0);
    let row = clamp((even + 1.0) / 2.0 * f32(elevations) - 0.5, 0.0, f32(elevations) - 1.0);
    let column_floor = floor(column);
    let column_0 = u32(column_floor - floor(column_floor / f32(azimuths)) * f32(azimuths)) % azimuths;
    let column_1 = (column_0 + 1u) % azimuths;
    let row_0 = u32(floor(row));
    let row_1 = min(row_0 + 1u, elevations - 1u);
    let across = column - column_floor;
    let low = mix(depth_at(column_0, row_0), depth_at(column_1, row_0), across);
    let high = mix(depth_at(column_0, row_1), depth_at(column_1, row_1), across);
    return mix(low, high, row - floor(row));
}

// `Sight::seen`.
fn seen(point: vec3<f32>, normal: vec3<f32>) -> f32 {
    let offset = point - cutaway.origin.xyz;
    let distance = length(offset);
    if distance > cutaway.origin.w || distance <= 1e-6 {
        return 0.0;
    }
    let facing = smoothstep(FACING_AWAY, FACING_TOWARD, dot(normal, -offset / distance));
    let depth = depth_towards(offset);
    let start = depth + SEEN_MARGIN + depth * SEEN_MARGIN_SHARE;
    let end = start + SEEN_SOFT + depth * SEEN_SOFT_SHARE;
    return facing * (1.0 - smoothstep(start, end, distance));
}

// `Sight::reaches`.
fn reaches(point: vec3<f32>) -> bool {
    let offset = point - cutaway.origin.xyz;
    let distance = length(offset);
    return distance <= cutaway.origin.w && distance <= depth_towards(offset) + REACH_SLACK;
}

// `Sight::sees_body`.
fn sees_body(origin: vec3<f32>, lift: f32, margin: f32) -> bool {
    let offset = origin + vec3(0.0, lift, 0.0) - cutaway.origin.xyz;
    let distance = length(offset);
    return distance <= cutaway.origin.w && distance <= depth_towards(offset) + margin;
}

// `Explored::amount`.
fn explored_amount(point: vec2<f32>) -> f32 {
    let cell = cutaway.grid.z;
    if cell <= 0.0 {
        return 0.0;
    }
    let at = (point - cutaway.grid.xy) / cell - 0.5;
    let corner = floor(at);
    let between = smoothstep(vec2(0.0), vec2(1.0), at - corner);
    let middle = cutaway.grid.xy + (corner + 0.5) * cell;
    let low = mix(
        f32(is_explored(middle)),
        f32(is_explored(middle + vec2(cell, 0.0))),
        between.x,
    );
    let high = mix(
        f32(is_explored(middle + vec2(0.0, cell))),
        f32(is_explored(middle + vec2(cell, cell))),
        between.x,
    );
    return mix(low, high, between.y);
}

// A number in 0..1 for the patch of the level `point` is in, fixed to the
// world so the grain does not swim as the head moves.
fn grain(point: vec3<f32>) -> f32 {
    let cell = vec3<u32>(vec3<i32>(floor(point / GRAIN)));
    var hash = cell.x * 73856093u ^ cell.y * 19349663u ^ cell.z * 83492791u;
    hash = (hash ^ (hash >> 13u)) * 1274126177u;
    hash = hash ^ (hash >> 16u);
    return f32(hash & 0xffffu) / 65535.0;
}

// `Explored::is_explored`.
fn is_explored(point: vec2<f32>) -> bool {
    let cell = cutaway.grid.z;
    if cell <= 0.0 {
        return false;
    }
    let at = floor((point - cutaway.grid.xy) / cell);
    if at.x < 0.0 || at.y < 0.0 || u32(at.x) >= cutaway.counts.z || u32(at.y) >= cutaway.counts.w {
        return false;
    }
    let index = u32(at.y) * cutaway.counts.z + u32(at.x);
    return ((data.explored[min(index / 32u, MAX_WORDS - 1u)] >> (index % 32u)) & 1u) == 1u;
}

// `OpenColumns::is_open`.
fn is_open_column(point: vec3<f32>) -> bool {
    let cell = cutaway.columns.z;
    if cell <= 0.0 {
        return false;
    }
    let at = floor((point.xz - cutaway.columns.xy) / cell);
    if at.x < 0.0 || at.y < 0.0 || at.x >= f32(COLUMNS) || at.y >= f32(COLUMNS) {
        return false;
    }
    let span = data.spans[u32(at.y) * COLUMNS + u32(at.x)];
    let bottom = f32(bitcast<i32>(span << 16u) >> 16u);
    let top = f32(bitcast<i32>(span) >> 16u);
    let height = point.y / HEIGHT_UNIT;
    return bottom <= height && height <= top;
}

// `Cutaway::hides_sight`.
fn hides_sight(point: vec3<f32>) -> bool {
    let to = point - cutaway.eye.xyz;
    if dot(to, to) <= 1e-12 {
        return false;
    }
    let away = normalize(to);
    let floor_height = cutaway.origin.y - HIDE_DEPTH;
    var span = HIDE_LENGTH;
    if away.y < -0.05 {
        span = min((point.y - floor_height) / -away.y, HIDE_LENGTH);
    }
    if span <= 0.0 {
        return false;
    }
    for (var index = 1u; index <= HIDE_STEPS; index++) {
        let along = f32(index) / f32(HIDE_STEPS);
        let behind = point + away * (span * along * along);
        // The column first, a single read; the sight's own depths only for
        // a point it says is open.
        if is_open_column(behind) && reaches(behind) {
            return true;
        }
    }
    return false;
}

// `Cutaway::cuts`, for this view and mesh: x how much the fragment is seen,
// 1 drawn as it is down to 0 drawn dark, or CUT; y how bright dark is.
fn judge(world: vec3<f32>, normal: vec3<f32>, tag: u32, instance: u32) -> vec2<f32> {
    let whole = vec2(1.0, 1.0);
    if cutaway.eye.w == 0.0 || tag == KEEP {
        return whole;
    }
    if length(view.world_from_view[0].xyz) < cutaway.shade.x {
        return whole;
    }
    let origin = cutaway.origin.xyz;
    let reach = cutaway.origin.w;
    let edge = 1.0 - smoothstep(reach * (1.0 - EDGE_FADE), reach, length(world.xz - origin.xz));
    let threshold = grain(world);
    // The edge dissolves: drawn where the grain is under how far in it is,
    // a soft scatter rather than a line.
    if edge <= threshold {
        return vec2(CUT, 0.0);
    }
    if cutaway.counts.x == 0u {
        return whole;
    }
    if (tag & 0xffu) == WHEN_SEEN {
        let body = get_world_from_local(instance)[3].xyz;
        let lift = f32((tag >> 8u) & 0xffu) / 10.0;
        let margin = f32((tag >> 16u) & 0xffu) / 10.0;
        return select(vec2(CUT, 0.0), whole, sees_body(body, lift, margin));
    }
    let amount = seen(world, normal);
    if amount < 0.5 && hides_sight(world) {
        return vec2(CUT, 0.0);
    }
    let dark = mix(cutaway.shade.z, cutaway.shade.y, explored_amount(world.xz));
    return vec2(amount, dark);
}

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var in = vertex_output;

    let judged = judge(
        in.world_position.xyz,
        normalize(in.world_normal) * select(-1.0, 1.0, is_front),
        get_tag(in.instance_index),
        in.instance_index,
    );
    if judged.x < 0.0 {
        discard;
    }

#ifdef VISIBILITY_RANGE_DITHER
    visibility_range_dither(in.position, in.visibility_range_dither);
#endif

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

#ifdef PREPASS_PIPELINE
    let out = deferred_output(in, pbr_input);
#else
    var out: FragmentOutput;
    if (pbr_input.material.flags & STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    if judged.x < 1.0 {
        // As far as it is not seen: greyed and dimmed, less where it is
        // remembered.
        let grey = dot(out.color.rgb, vec3(0.2126, 0.7152, 0.0722));
        let dark = mix(vec3(grey), out.color.rgb, 0.4) * judged.y;
        out.color = vec4(mix(dark, out.color.rgb, judged.x), out.color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#endif

    return out;
}
