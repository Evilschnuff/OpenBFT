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
    // Smallest view scale, explored shade, unseen shade, the sight's lean.
    shade: vec4<f32>,
    // Azimuths (0: no sight), elevations, explored columns, rows.
    counts: vec4<u32>,
    // Explored corner x, z, cell size.
    grid: vec4<f32>,
    // Open columns' corner x, z, column size.
    columns: vec4<f32>,
    // The previous sight's origin; w its reach.
    previous: vec4<f32>,
    // The previous sight's azimuths (0: none) and elevations.
    previous_counts: vec4<u32>,
    // How far the view has faded from the previous sight to the current,
    // 0–1; the previous sight's lean; 1 in z to draw only what is seen.
    fade: vec4<f32>,
    // The colour the edge of a cut is tinted toward, linear.
    rim: vec4<f32>,
}

// Written only when the sight or the explored ground changes.
struct CutawayData {
    depths: array<f32, MAX_DIRECTIONS>,
    explored: array<u32, MAX_WORDS>,
    // Per column: the lowest and highest open height, in HEIGHT_UNITs, as
    // two signed halves, the lowest in the low half.
    spans: array<u32, 262144>,
    // The previous sight's depths, as `depths`.
    previous_depths: array<f32, MAX_DIRECTIONS>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<storage, read> cutaway: Cutaway;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var<storage, read> data: CutawayData;

// `cutaway::KEEP`, the low byte of `cutaway::when_seen`, and `SPARE`.
const KEEP: u32 = 1u;
const WHEN_SEEN: u32 = 2u;
const SPARE: u32 = 3u;

// The constants of cutaway.rs.
const SEEN_MARGIN: f32 = 1.5;
const SEEN_MARGIN_SHARE: f32 = 0.03;
const SEEN_SOFT: f32 = 3.0;
const SEEN_SOFT_SHARE: f32 = 0.05;
const FACING_AWAY: f32 = -0.3;
const FACING_TOWARD: f32 = 0.2;
const EDGE_FADE: f32 = 0.08;
const LEAN_MAX_ANGLE: f32 = 0.35;
const LEAN_UPRIGHT: f32 = 0.5;
const LEAN_SAMPLES: u32 = 6u;
const LEAN_CLEAR: f32 = 2.0;
const LEAN_CLEAR_SHARE: f32 = 0.1;
// The grain the edge of what is drawn dissolves in, metres of the level.
const GRAIN: f32 = 0.5;
const WALK_COLUMNS: u32 = 96u;
const WALK_LEAD: f32 = 0.1;
const RIM_LENGTH: f32 = 3.0;
// How far toward the rim colour what is left of a cut's edge is tinted, at
// most.
const RIM_TINT: f32 = 0.85;
const HIDE_LENGTH: f32 = 300.0;
const HIDE_DEPTH: f32 = 30.0;
const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;

// What `judge` returns for a fragment that is not drawn.
const CUT: f32 = -1.0;

// Which sight a function reads: the one being shown, or the one the view
// is fading over from (`Cutaway::previous`).
const CURRENT: u32 = 0u;
const PREVIOUS: u32 = 1u;

// A sight's origin, its reach in w.
fn sight_origin(which: u32) -> vec4<f32> {
    return select(cutaway.origin, cutaway.previous, which == PREVIOUS);
}

// A sight's azimuths and elevations.
fn sight_counts(which: u32) -> vec2<u32> {
    return select(cutaway.counts.xy, cutaway.previous_counts.xy, which == PREVIOUS);
}

// A sight's lean.
fn sight_lean(which: u32) -> f32 {
    return select(cutaway.shade.w, cutaway.fade.y, which == PREVIOUS);
}

fn depth_at(which: u32, column: u32, row: u32) -> f32 {
    let index = min(row * sight_counts(which).x + column, MAX_DIRECTIONS - 1u);
    if which == PREVIOUS {
        return data.previous_depths[index];
    }
    return data.depths[index];
}

// The four directions round `offset` and how far between them it lies: the
// columns and rows packed as (column 0, column 1, row 0, row 1), the
// fractions in `between`.
struct Around {
    cells: vec4<u32>,
    between: vec2<f32>,
}

fn around(which: u32, offset: vec3<f32>) -> Around {
    let magnitude = length(offset);
    let counts = sight_counts(which);
    let azimuths = counts.x;
    let elevations = counts.y;
    let column = (atan2(offset.z, offset.x) + PI) / TAU * f32(azimuths) - 0.5;
    let height = offset.y / max(magnitude, 1e-6);
    let even = sign(height) * pow(abs(height), 1.0 / 3.0);
    let row = clamp((even + 1.0) / 2.0 * f32(elevations) - 0.5, 0.0, f32(elevations) - 1.0);
    let column_floor = floor(column);
    let column_0 = u32(column_floor - floor(column_floor / f32(azimuths)) * f32(azimuths)) % azimuths;
    let column_1 = (column_0 + 1u) % azimuths;
    let row_0 = u32(floor(row));
    let row_1 = min(row_0 + 1u, elevations - 1u);
    return Around(vec4(column_0, column_1, row_0, row_1), vec2(column - column_floor, row - floor(row)));
}

// `Sight::depth_towards`: how far the sight reaches toward `offset`,
// blended between the four directions round it.
fn depth_towards(which: u32, offset: vec3<f32>) -> f32 {
    if length(offset) <= 1e-6 {
        return sight_origin(which).w;
    }
    let cells = around(which, offset);
    let c = cells.cells;
    let low = mix(depth_at(which, c.x, c.z), depth_at(which, c.y, c.z), cells.between.x);
    let high = mix(depth_at(which, c.x, c.w), depth_at(which, c.y, c.w), cells.between.x);
    return mix(low, high, cells.between.y);
}

// `Sight::nearest_depth_towards`: the nearest of the four directions round
// `offset`.
fn nearest_depth_towards(which: u32, offset: vec3<f32>) -> f32 {
    if length(offset) <= 1e-6 {
        return sight_origin(which).w;
    }
    let c = around(which, offset).cells;
    return min(
        min(depth_at(which, c.x, c.z), depth_at(which, c.y, c.z)),
        min(depth_at(which, c.x, c.w), depth_at(which, c.y, c.w)),
    );
}

// `Sight::leans_past`.
fn leans_past(which: u32, offset: vec3<f32>, distance: f32) -> f32 {
    let lean = sight_lean(which);
    let depth = nearest_depth_towards(which, offset);
    if lean <= 0.0 || depth <= 1e-6 || depth >= distance {
        return 0.0;
    }
    let angle = min(lean * (1.0 / depth - 1.0 / distance), LEAN_MAX_ANGLE);
    let toward = offset / distance;
    var side = cross(toward, vec3(0.0, 1.0, 0.0));
    if length(side) <= 1e-6 {
        side = vec3(1.0, 0.0, 0.0);
    } else {
        side = normalize(side);
    }
    let up = cross(side, toward);
    let clear = depth + LEAN_CLEAR + depth * LEAN_CLEAR_SHARE;
    let full = clear + LEAN_CLEAR + depth * LEAN_CLEAR_SHARE;
    var best = 0.0;
    for (var sample = 0u; sample < LEAN_SAMPLES; sample++) {
        let around_angle = f32(sample) / f32(LEAN_SAMPLES) * TAU;
        let aside = side * cos(around_angle) + up * (sin(around_angle) * LEAN_UPRIGHT);
        let beside = toward * cos(angle) + aside * sin(angle);
        best = max(best, smoothstep(clear, full, depth_towards(which, beside)));
    }
    return best;
}

// `Sight::seen`, for one of the two sights.
fn seen_by(which: u32, point: vec3<f32>, normal: vec3<f32>) -> f32 {
    let origin = sight_origin(which);
    let offset = point - origin.xyz;
    let distance = length(offset);
    if distance > origin.w || distance <= 1e-6 {
        return 0.0;
    }
    let facing = smoothstep(FACING_AWAY, FACING_TOWARD, dot(normal, -offset / distance));
    let depth = depth_towards(which, offset);
    let start = depth + SEEN_MARGIN + depth * SEEN_MARGIN_SHARE;
    let end = start + SEEN_SOFT + depth * SEEN_SOFT_SHARE;
    var amount = 1.0 - smoothstep(start, end, distance);
    if amount < 1.0 {
        amount = max(amount, leans_past(which, offset, distance));
    }
    return facing * amount;
}

// `Cutaway::seen`: the current sight's, faded in over the previous one's.
fn seen(point: vec3<f32>, normal: vec3<f32>) -> f32 {
    let current = seen_by(CURRENT, point, normal);
    let fade = cutaway.fade.x;
    if fade >= 1.0 || cutaway.previous_counts.x == 0u {
        return current;
    }
    return mix(seen_by(PREVIOUS, point, normal), current, fade);
}

// `Sight::reaches`, the current sight's.
fn reaches(point: vec3<f32>) -> bool {
    let offset = point - cutaway.origin.xyz;
    let distance = length(offset);
    return distance <= cutaway.origin.w && distance <= depth_towards(CURRENT, offset);
}

// `Sight::sees_body`, the current sight's.
fn sees_body(origin: vec3<f32>, lift: f32, margin: f32) -> bool {
    let offset = origin + vec3(0.0, lift, 0.0) - cutaway.origin.xyz;
    let distance = length(offset);
    if distance > cutaway.origin.w {
        return false;
    }
    let depth = depth_towards(CURRENT, offset);
    return distance <= depth + margin || leans_past(CURRENT, offset, distance) >= 0.5;
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
// A column's open span in metres, (bottom, top); bottom over top when it
// has none or lies outside the grid.
fn column_span(cell: vec2<f32>) -> vec2<f32> {
    if cell.x < 0.0 || cell.y < 0.0 || cell.x >= f32(COLUMNS) || cell.y >= f32(COLUMNS) {
        return vec2(1.0, 0.0);
    }
    let span = data.spans[u32(cell.y) * COLUMNS + u32(cell.x)];
    let bottom = f32(bitcast<i32>(span << 16u) >> 16u);
    let top = f32(bitcast<i32>(span) >> 16u);
    return vec2(bottom, top) * HEIGHT_UNIT;
}

// How far along a line through `at` (in columns) the next boundary across
// one axis lies, metres of the line.
fn next_boundary(at: f32, cell: f32, along: f32) -> f32 {
    if abs(along) <= 1e-6 {
        return 1e30;
    }
    let next = select(cell, cell + 1.0, along > 0.0);
    return (next - at) * cutaway.columns.z / along;
}

// `OpenColumns::walk` with `Sight::reaches` as what it accepts: how far
// of the line runs through open space the sight sees, up to `enough`.
fn walk_open_length(start: vec3<f32>, direction: vec3<f32>, length: f32, enough: f32) -> f32 {
    var open = 0.0;
    let size = cutaway.columns.z;
    if size <= 0.0 || length <= WALK_LEAD {
        return open;
    }
    let at = (start.xz - cutaway.columns.xy) / size;
    var cell = floor(at);
    let across = direction.xz;
    let step = sign(across);
    var next = vec2(next_boundary(at.x, cell.x, across.x), next_boundary(at.y, cell.y, across.y));
    let delta = vec2(
        select(size / abs(across.x), 1e30, abs(across.x) <= 1e-6),
        select(size / abs(across.y), 1e30, abs(across.y) <= 1e-6),
    );
    var enter = WALK_LEAD;
    for (var index = 0u; index < WALK_COLUMNS; index++) {
        let leave = min(min(next.x, next.y), length);
        if leave > enter {
            let span = column_span(cell);
            var low = 1.0;
            var high = 0.0;
            if abs(direction.y) <= 1e-6 {
                if span.x <= start.y && start.y <= span.y {
                    low = enter;
                    high = leave;
                }
            } else if span.x <= span.y {
                let to_bottom = (span.x - start.y) / direction.y;
                let to_top = (span.y - start.y) / direction.y;
                low = max(min(to_bottom, to_top), enter);
                high = min(max(to_bottom, to_top), leave);
            }
            if low <= high
                && (reaches(start + direction * ((low + high) * 0.5))
                    || reaches(start + direction * high))
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
    return open;
}

// `Cutaway::hidden_behind`: how much open space the sight sees lies behind
// `point` along the eye's line, 0–1.
fn hidden_behind(point: vec3<f32>) -> f32 {
    let to = point - cutaway.eye.xyz;
    if dot(to, to) <= 1e-12 {
        return 0.0;
    }
    let away = normalize(to);
    let floor_height = cutaway.origin.y - HIDE_DEPTH;
    var span = HIDE_LENGTH;
    if away.y < -0.05 {
        span = min((point.y - floor_height) / -away.y, HIDE_LENGTH);
    }
    if span <= 0.0 {
        return 0.0;
    }
    // Every column behind it, so a low tunnel is not stepped over; the
    // sight's own depths confirm a point a column says is open.
    return min(walk_open_length(point, away, span, RIM_LENGTH) / RIM_LENGTH, 1.0);
}

// `Cutaway::cuts`, for this view and mesh: x how much the fragment is seen,
// 1 drawn as it is down to 0 drawn dark, or CUT; y how bright dark is; z how
// far it lies in the edge of a cut, 0–1, to tint it by.
fn judge(world: vec3<f32>, normal: vec3<f32>, tag: u32, instance: u32) -> vec3<f32> {
    let whole = vec3(1.0, 1.0, 0.0);
    let cut = vec3(CUT, 0.0, 0.0);
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
    // a soft scatter rather than a line, and what is left is tinted.
    if edge <= threshold {
        return cut;
    }
    var rim = 1.0 - edge;
    if cutaway.counts.x == 0u {
        return vec3(1.0, 1.0, rim);
    }
    if (tag & 0xffu) == WHEN_SEEN {
        let body = get_world_from_local(instance)[3].xyz;
        let lift = f32((tag >> 8u) & 0xffu) / 10.0;
        let margin = f32((tag >> 16u) & 0xffu) / 10.0;
        return select(cut, whole, sees_body(body, lift, margin));
    }
    let amount = seen(world, normal);
    if amount < 0.5 {
        // How far it is to be cut, 0–1: seeing only what is seen, by how
        // little it is seen; otherwise by how much of the sight it hides,
        // and never something spared. Cut where the grain is under that, so
        // the edge of a cut thins out over a band, and the rest is tinted.
        var hide = 0.0;
        if cutaway.fade.z != 0.0 {
            hide = clamp(1.0 - amount * 2.0, 0.0, 1.0);
        } else if tag != SPARE {
            hide = hidden_behind(world);
        }
        if hide > threshold {
            return cut;
        }
        rim = max(rim, hide);
    }
    let dark = mix(cutaway.shade.z, cutaway.shade.y, explored_amount(world.xz));
    return vec3(amount, dark, rim);
}

@fragment
fn fragment(
    vertex_output: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    var in = vertex_output;

    let tag = get_tag(in.instance_index);
    let judged = judge(
        in.world_position.xyz,
        normalize(in.world_normal) * select(-1.0, 1.0, is_front),
        tag,
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
    if judged.z > 0.0 {
        // In the edge of a cut, or the level's rim: tinted, so what is cut
        // away shows as cut.
        out.color = vec4(mix(out.color.rgb, cutaway.rim.rgb, judged.z * RIM_TINT), out.color.a);
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
#endif

    return out;
}
