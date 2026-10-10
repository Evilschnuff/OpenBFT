//! One tick of an enemy: perceive, decide, steer, aim, shoot.
//!
//! Every system here runs in [`SimSet::Ai`], which is after `SimSet::Damage`,
//! so a brain never steers a hull that was destroyed earlier in the same tick.
//! The inputs a brain writes are read by [`SimSet::Vehicle`] on the *next*
//! tick, which is one tick of lag and is invisible at 60 Hz.

use avian3d::prelude::*;
use bevy::prelude::*;
use rand::{Rng, RngExt};

use super::table::{AiTable, AttackStrategy, NetDef, NetType, TurretDef};
use super::tuning::SteeringTuning;
use super::{AiBrain, AiState, AiTick, Enemy, TurretAi};
use crate::cinematic::PlayerControl;
use crate::damage::{DamageEvent, Destroyed, Health, Team};
use crate::level::GameLayer;
use crate::physics::SimRng;
use crate::vehicle::{PlayerInput, Turret};
use crate::weapons::{FireRequest, Mover, Muzzle, WeaponLoadout, WeaponTable};

/// Everything a brain can see and shoot: it has health and a side.
///
/// The same shape the weapons' `TargetQuery` has, and for the same reason: a
/// thing with no health cannot be hurt and a thing with no team cannot be an
/// enemy of anything.
pub type PerceivableQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        &'static Team,
        Option<&'static LinearVelocity>,
    ),
    With<Health>,
>;

/// Whether a cinematic is running, during which no enemy sees anything.
///
/// The intros land the player among enemies — m2's minelayer is placed 40 m
/// from the start — and the original's do not open fire on a VTOL that is
/// still unloading. The player cannot shoot back while the scripts hold the
/// input, so nothing else can wake them either.
fn in_cinematic(control: &Option<Res<PlayerControl>>) -> bool {
    control
        .as_ref()
        .is_some_and(|control| control.is_cinematic())
}

/// Count the ticks, so the perception cadence has something to phase against.
pub fn tick_ai(mut tick: ResMut<AiTick>) {
    tick.0 = tick.0.wrapping_add(1);
}

// ---------------------------------------------------------------------------
// Perception
// ---------------------------------------------------------------------------

/// What a perception pass found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sighting {
    pub entity: Entity,
    pub position: Vec3,
    pub velocity: Vec3,
    pub distance: f32,
}

/// The nearest hostile inside `range` with a clear line of sight to `from`.
///
/// Line of sight is one ray against [`GameLayer::Level`] only: a vehicle
/// between the two does not block the shot, because in the original an enemy
/// happily shoots through its own kind and the player's cover is terrain.
/// The ray stops [`LOS_MARGIN`] short of the target so a target standing
/// against a wall is not occluded by the wall behind it, and ends
/// [`EYE_HEIGHT`](super::tuning::EYE_HEIGHT) above the target's origin — the
/// middle of its hull rather than its tracks, which on rolling ground are
/// hidden behind the first rise long before the vehicle is.
pub fn nearest_visible(
    spatial: &SpatialQuery,
    targets: &PerceivableQuery,
    from: Vec3,
    team: Team,
    range: f32,
    require_los: bool,
    ignore: &[Entity],
) -> Option<Sighting> {
    let mut best: Option<Sighting> = None;

    for (entity, transform, other, velocity) in targets.iter() {
        if !other.is_hostile_to(team) {
            continue;
        }

        let position = transform.translation();
        let offset = position - from;
        let distance = offset.length();

        if distance > range || distance <= f32::EPSILON {
            continue;
        }
        if best.is_some_and(|best| best.distance <= distance) {
            continue;
        }
        let seen_at = position + Vec3::Y * super::tuning::EYE_HEIGHT;
        if require_los && !has_line_of_sight(spatial, from, seen_at, ignore) {
            continue;
        }

        best = Some(Sighting {
            entity,
            position,
            velocity: velocity.map(|velocity| velocity.0).unwrap_or(Vec3::ZERO),
            distance,
        });
    }

    best
}

/// How far short of a target the line-of-sight ray stops, in metres.
///
/// // approximation: no reader field corresponds. Half a vehicle width, so a
/// target parked against a wall is visible and one just behind it is not.
pub const LOS_MARGIN: f32 = 1.5;

/// Whether nothing solid stands between two points.
///
/// `ignore` is the looker's own geometry, where it is part of the level.
pub fn has_line_of_sight(spatial: &SpatialQuery, from: Vec3, to: Vec3, ignore: &[Entity]) -> bool {
    let offset = to - from;
    let distance = offset.length();

    if distance <= LOS_MARGIN {
        return true;
    }

    let Ok(direction) = Dir3::new(offset / distance) else {
        return true;
    };

    spatial
        .cast_ray(
            from,
            direction,
            distance - LOS_MARGIN,
            true,
            &SpatialQueryFilter::from_mask(GameLayer::Level)
                .with_excluded_entities(ignore.iter().copied()),
        )
        .is_none()
}

/// Re-acquire every brain's target, on its own phase of the cadence.
///
/// A sleeping brain — [`Dormant`](AiState::Dormant) or
/// [`Roam`](AiState::Roam) — does not look; it measures. Its "target" is the
/// nearest hostile by distance alone, out to whichever is further of its
/// wake radius and [`SteeringTuning::roam_range`], because all [`think`]
/// asks of it is how near the player is. That is also no ray at all for most
/// of a level's units, most of the time.
pub fn perceive(
    tick: Res<AiTick>,
    time: Res<Time<Fixed>>,
    table: Option<Res<AiTable>>,
    control: Option<Res<PlayerControl>>,
    spatial: SpatialQuery,
    targets: PerceivableQuery,
    mut brains: Query<(&mut AiBrain, &GlobalTransform, &Team)>,
) {
    let delta = time.delta_secs();
    let blind = in_cinematic(&control);

    for (mut brain, transform, team) in &mut brains {
        brain.since_seen += delta;

        if brain.state == AiState::Dead {
            continue;
        }
        if !brain.steering.perceives(tick.0, brain.phase) {
            continue;
        }

        let net = net_of(&table, &brain);
        let from = transform.translation() + Vec3::Y * super::tuning::EYE_HEIGHT;

        if !brain.state.awake() {
            let range = brain.wake_rad(net).max(brain.steering.roam_range);
            brain.target = if blind {
                None
            } else {
                nearest_visible(&spatial, &targets, from, *team, range, false, &[])
                    .map(|sighting| sighting.entity)
            };
            // Known about, not seen: it must not fire on the tick it wakes
            // until an awake pass has actually seen the target.
            brain.los_lost = brain.los_lost.max(1);
            continue;
        }

        // The detection range: the net's attack radius where one is stated,
        // else the weapon row's own engagement range. A brain looks exactly
        // as far as it is authored to shoot, which is what stops a woken
        // group chasing a player it could never reach.
        let range = brain.attack_rad(net).max(brain.weapon_range);

        let sighting = if blind {
            None
        } else {
            nearest_visible(&spatial, &targets, from, *team, range, true, &[])
        };

        match sighting {
            Some(sighting) => {
                brain.target = Some(sighting.entity);
                brain.los_lost = 0;
                brain.since_seen = 0.0;
            }
            None => {
                brain.los_lost = brain.los_lost.saturating_add(1);

                // Two conditions, and both are needed: the tick counter is
                // the "it ducked behind a rock" filter (ours), and the
                // seconds are the data's `not_pursuit_dwell`.
                let dwell = brain.not_pursuit_dwell(net);
                if brain.los_lost >= brain.steering.los_lost_ticks && brain.since_seen >= dwell {
                    brain.target = None;
                }
            }
        }
    }
}

/// The net a brain walks, if the table has it.
fn net_of<'a>(table: &'a Option<Res<AiTable>>, brain: &AiBrain) -> Option<&'a NetDef> {
    table
        .as_ref()
        .zip(brain.net)
        .and_then(|(table, index)| table.net(index))
}

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

/// Advance every brain's state.
///
/// Separate from [`steer`] so the decision can be tested without a physics
/// world: this reads positions and writes a state, and nothing else.
pub fn think(
    time: Res<Time<Fixed>>,
    table: Option<Res<AiTable>>,
    positions: Query<&GlobalTransform>,
    mut brains: Query<(&mut AiBrain, &GlobalTransform, &Health)>,
) {
    let delta = time.delta_secs();

    for (mut brain, transform, health) in &mut brains {
        if health.is_dead() {
            if brain.state != AiState::Dead {
                brain.state = AiState::Dead;
                brain.target = None;
            }
            continue;
        }

        brain.timer += delta;

        let net = net_of(&table, &brain);
        let here = transform.translation();
        let target = brain
            .target
            .and_then(|entity| positions.get(entity).ok())
            .map(|transform| transform.translation());

        let next = next_state(&brain, net, here, target);

        // Waking counts as contact, so a woken unit turns on the player for
        // its `not_pursuit_dwell` even if it has not yet seen them, rather
        // than dropping the target on its first look.
        if !brain.state.awake() && next.awake() {
            brain.since_seen = 0.0;
        }

        if next != brain.state {
            brain.state = next;
            brain.timer = 0.0;
        }
    }
}

/// The state a brain should be in, given where everything is.
///
/// Pure, so the transition table is a unit test rather than a world.
pub fn next_state(
    brain: &AiBrain,
    net: Option<&NetDef>,
    here: Vec3,
    target: Option<Vec3>,
) -> AiState {
    if brain.state == AiState::Dead {
        return AiState::Dead;
    }

    let from_home = here.distance(brain.home);

    // Asleep is left only by the player coming inside `activate_rad`. A
    // buddy waking the group is `wake_buddies` and a hit is `wake_on_damage`,
    // which set the state directly rather than being cases here. Meanwhile a
    // player inside `roam_range` gets the unit walking its net.
    if !brain.state.awake() {
        let Some(target) = target else {
            return AiState::Dormant;
        };
        let distance = here.distance(target);

        let roam_range = if brain.state == AiState::Roam {
            brain.steering.roam_range * ROAM_HYSTERESIS
        } else {
            brain.steering.roam_range
        };

        return if distance <= brain.wake_rad(net) {
            AiState::Engage
        } else if distance <= roam_range {
            AiState::Roam
        } else {
            AiState::Dormant
        };
    }

    // The leash. Checked before the target, so a unit dragged past its
    // `return_range` goes home rather than following the player off the map
    // — which is what the key is for.
    if from_home > brain.return_range(net) {
        return AiState::Return;
    }

    // Home again: a `Return` ends when the unit is back inside a third of
    // its leash, not at the exact point, or it would oscillate on arrival.
    //
    // // approximation: the data states the range to leave at and not the
    // one to arrive at. A third is well clear of the boundary that sent it
    // home and still short of the point it started from.
    if brain.state == AiState::Return {
        let arrived = from_home <= brain.return_range(net) * RETURN_HYSTERESIS;
        if !arrived {
            return AiState::Return;
        }
    }

    // A `defilade` or `hide` net ducks into cover between attacks, for its
    // own `hide_times`. Only two nets in the shipped data state a non-zero
    // pair, so this is a rarely walked path and is deliberately simple.
    if brain.state == AiState::Hide {
        let [min, max] = net.map(|net| net.hide_times).unwrap_or([0.0, 0.0]);
        let hide_for = if max > min { max } else { min };

        if brain.timer < hide_for {
            return AiState::Hide;
        }
    }

    match target {
        Some(target) => {
            let distance = here.distance(target);
            let attack_rad = brain.attack_rad(net);

            if distance > attack_rad.max(brain.weapon_range) {
                return patrol_or_sit(net);
            }

            // A hiding net that has been engaging for a full `attack_dwell`
            // goes back into cover.
            let hides = net.is_some_and(|net| {
                matches!(net.net_type, NetType::Defilade | NetType::Hide) && net.hide_times[1] > 0.0
            });
            if hides && brain.state == AiState::Engage && brain.timer >= dwell_of(net) {
                return AiState::Hide;
            }

            AiState::Engage
        }
        None => patrol_or_sit(net),
    }
}

/// How far back inside its leash a returning unit has to get, as a fraction.
pub const RETURN_HYSTERESIS: f32 = 0.33;

/// How far past [`SteeringTuning::roam_range`] a roaming unit has to be left
/// before it parks again, as a factor.
///
/// // approximation: no reader field corresponds. Enough that a player
/// driving along the boundary does not start and stop every unit on it.
pub const ROAM_HYSTERESIS: f32 = 1.1;

/// What a brain with nothing to shoot does.
///
/// A `fixed` net stays on its node (READER-SCHEMA §3.3), so its idle state is
/// `Patrol` with nowhere to go rather than a separate one — [`steer`] reads
/// the net type and holds position.
fn patrol_or_sit(net: Option<&NetDef>) -> AiState {
    let _ = net;

    AiState::Patrol
}

/// The net's `attack_dwell`, or zero.
fn dwell_of(net: Option<&NetDef>) -> f32 {
    net.map(|net| net.attack_dwell).unwrap_or(0.0)
}

/// Wake a whole group when one of its members wakes.
///
/// `activate_buddy` is 1 on 50 of 441 nets and means exactly this: the group
/// shares its activation. It runs after [`think`], so a unit that woke this
/// tick wakes its buddies on the same tick.
///
/// `attack_buddy` — sharing a *target* — is 0 in every shipped net, so it is
/// read into [`NetDef`] and deliberately not implemented: there is nothing to
/// test it against and a behaviour no data exercises is a behaviour that will
/// be wrong.
pub fn wake_buddies(table: Option<Res<AiTable>>, mut brains: Query<&mut AiBrain>) {
    let Some(table) = table else {
        return;
    };

    // Which nets have a member awake this tick.
    let woken: Vec<usize> = brains
        .iter()
        .filter(|brain| brain.state.awake())
        .filter_map(|brain| brain.net)
        .filter(|index| table.net(*index).is_some_and(|net| net.activate_buddy))
        .collect();

    if woken.is_empty() {
        return;
    }

    for mut brain in &mut brains {
        if brain.state.awake() || brain.state == AiState::Dead {
            continue;
        }
        if brain.net.is_some_and(|index| woken.contains(&index)) {
            brain.state = AiState::Patrol;
            brain.timer = 0.0;
        }
    }
}

/// Wake a unit that is shot, and turn an awake one on whoever shot it.
///
/// No reader field states this for vehicles — `ai.zrd`'s `ACTIVATE_ON_HIT`
/// says it for a turret — but a unit that sits through being shelled
/// because the player fired from outside its `activate_rad` reads as a
/// wreck, and the original's do not. Friendly fire wakes nobody: the shooter
/// has to be hostile to the unit.
///
/// The shooter becomes the target without being *seen*, so the unit turns
/// and hunts for its `not_pursuit_dwell` but does not fire until a
/// perception pass has line of sight.
pub fn wake_on_damage(
    mut hits: MessageReader<DamageEvent>,
    teams: Query<&Team>,
    mut brains: Query<(&mut AiBrain, &Team)>,
) {
    for hit in hits.read() {
        let Some(source) = hit.source else {
            continue;
        };
        let Ok((mut brain, team)) = brains.get_mut(hit.target) else {
            continue;
        };
        if brain.state == AiState::Dead {
            continue;
        }
        if !teams
            .get(source)
            .is_ok_and(|other| other.is_hostile_to(*team))
        {
            continue;
        }

        brain.target = Some(source);
        brain.since_seen = 0.0;
        if !brain.state.awake() {
            brain.state = AiState::Engage;
            brain.timer = 0.0;
        }
    }
}

// ---------------------------------------------------------------------------
// Steering
// ---------------------------------------------------------------------------

/// Turn each brain's state into throttle and steering.
#[allow(clippy::type_complexity)]
pub fn steer(
    table: Option<Res<AiTable>>,
    spatial: SpatialQuery,
    positions: Query<&GlobalTransform>,
    mut brains: Query<(&mut AiBrain, &mut PlayerInput, &Transform, &LinearVelocity), With<Enemy>>,
) {
    for (mut brain, mut input, transform, velocity) in &mut brains {
        if !brain.state.moves() {
            input.throttle = 0.0;
            input.steer = 0.0;
            continue;
        }

        let net = net_of(&table, &brain);
        let here = transform.translation;
        let target = brain
            .target
            .and_then(|entity| positions.get(entity).ok())
            .map(|transform| transform.translation());

        let Some(goal) = desired_point(&mut brain, net, here, target, velocity.0) else {
            input.throttle = 0.0;
            input.steer = 0.0;
            continue;
        };

        let (throttle, steering) = drive_toward(&brain.steering, transform, goal);
        let steering = avoid_walls(&spatial, &brain.steering, transform, steering);

        input.throttle = throttle;
        input.steer = steering;
    }
}

/// Where this brain wants to be, in world space.
///
/// `None` means "hold position", which is what a `sit` strategy, a `fixed`
/// net and a patrol with no graph all want.
pub fn desired_point(
    brain: &mut AiBrain,
    net: Option<&NetDef>,
    here: Vec3,
    target: Option<Vec3>,
    velocity: Vec3,
) -> Option<Vec3> {
    match brain.state {
        AiState::Return => Some(brain.home),
        AiState::Engage => {
            let target = target?;
            engage_point(brain, net, here, target, velocity)
        }
        AiState::Patrol | AiState::Roam => patrol_point(brain, net, here),
        _ => None,
    }
}

/// Where a brain drives while engaging, per the net's `attack_strategy`.
fn engage_point(
    brain: &mut AiBrain,
    net: Option<&NetDef>,
    here: Vec3,
    target: Vec3,
    velocity: Vec3,
) -> Option<Vec3> {
    let [near, far] = brain.pursuit(net);
    let to_target = target - here;
    let distance = to_target.length();

    match brain.strategy {
        // Keep walking the graph and shoot from it, which is what the
        // commonest strategy says: 163 of 457 nets.
        Some(AttackStrategy::FollowNet) | None => patrol_point(brain, net, here),

        // Straight at it, stopping at the near standoff so it does not ram.
        Some(AttackStrategy::HeadOn) => {
            if near > 0.0 && distance <= near {
                None
            } else {
                Some(target)
            }
        }

        // Across its front, reversing every `strafe_period`. The leg is
        // perpendicular to the line of sight at the standoff distance.
        Some(AttackStrategy::BackForth) => {
            let period = brain.steering.strafe_period.max(0.1);
            // `forward` doubles as the strafe direction: the field already
            // exists for the patrol walk and a brain is never doing both.
            brain.forward = brain.timer % (period * 2.0) < period;

            let flat = Vec3::new(to_target.x, 0.0, to_target.z).normalize_or_zero();
            let across = Vec3::new(-flat.z, 0.0, flat.x) * if brain.forward { 1.0 } else { -1.0 };
            let standoff = standoff_point(here, target, near, far);

            Some(standoff + across * (far.max(near).max(1.0) * 0.5))
        }

        // Orbit at the standoff: the tangent, offset to close or open the
        // range as needed.
        Some(AttackStrategy::Circle) => {
            let flat = Vec3::new(to_target.x, 0.0, to_target.z);
            let range = flat.length().max(0.001);
            let inward = flat / range;
            let tangent = Vec3::new(-inward.z, 0.0, inward.x);

            let wanted = if far > 0.0 { (near + far) * 0.5 } else { range };
            // Positive when too far out, so the orbit spirals in.
            let radial = (range - wanted).clamp(-range, range);

            Some(
                here + (tangent * wanted.max(1.0) + inward * radial).normalize_or_zero()
                    * ORBIT_STEP,
            )
        }

        // Hold position and let the turret do the work.
        Some(AttackStrategy::Sit) => {
            let _ = velocity;
            None
        }
    }
}

/// How far ahead on the orbit a circling unit aims, in metres.
///
/// // approximation: no reader field corresponds. Far enough that the
/// steering has a heading to chase rather than a point it is already on.
pub const ORBIT_STEP: f32 = 25.0;

/// The point at the standoff distance along the line to the target.
fn standoff_point(here: Vec3, target: Vec3, near: f32, far: f32) -> Vec3 {
    let offset = target - here;
    let distance = offset.length();

    if distance <= f32::EPSILON {
        return here;
    }

    let wanted = if far > near && far > 0.0 {
        (near + far) * 0.5
    } else {
        near.max(1.0)
    };

    target - offset / distance * wanted.min(distance)
}

/// The next waypoint on the net's graph, advancing when one is reached.
///
/// A `fixed` net stays on its node, which is what the type means, so its
/// patrol point is the node it started from and it never advances.
fn patrol_point(brain: &mut AiBrain, net: Option<&NetDef>, here: Vec3) -> Option<Vec3> {
    let net = net?;

    if net.nodes.is_empty() {
        return None;
    }
    if net.net_type == NetType::Fixed {
        // Back to node 0, which `aiv.zrd` places the unit on anyway.
        return Some(net.nodes[0].pos);
    }

    let current = match brain.waypoint {
        Some(index) if index < net.nodes.len() => index,
        // Joining the graph: the nearest node, which is the one `aiv.zrd`
        // put the unit on for a unit that has not moved.
        _ => {
            let nearest = net.nearest_node(here)?;
            brain.waypoint = Some(nearest);
            nearest
        }
    };

    let node = net.nodes[current];

    if here.distance(node.pos) <= brain.steering.arrive_radius {
        brain.waypoint = Some(advance(brain, net, current));
        return brain.waypoint.map(|next| net.nodes[next].pos);
    }

    Some(node.pos)
}

/// The next node to walk to from `current`.
///
/// The links are **not symmetric** in the data — m1's `net_01` has
/// `node_00 → [1]` and `node_01 → [2, 0]` — so "the neighbour that is not
/// where I came from" is not always available. The walk takes the first such
/// neighbour, and reverses direction at a dead end, which turns a chain into
/// a patrol back and forth rather than a unit stuck at the last node.
fn advance(brain: &mut AiBrain, net: &NetDef, current: usize) -> usize {
    let previous = if brain.forward {
        current.checked_sub(1)
    } else {
        Some(current + 1)
    };

    let mut neighbours = net.neighbours(current).peekable();
    if neighbours.peek().is_none() {
        // An isolated node: stay on it.
        return current;
    }

    let onward = net
        .neighbours(current)
        .find(|next| Some(*next) != previous)
        .or_else(|| net.neighbours(current).next());

    match onward {
        Some(next) => {
            brain.forward = next > current;
            next
        }
        None => {
            brain.forward = !brain.forward;
            current
        }
    }
}

/// Throttle and steering that take a hull toward a point.
///
/// The steering is proportional to the signed angle between the hull's
/// forward and the direction to the point, saturating at
/// [`SteeringTuning::steer_gain`] radians of error. Past
/// [`SteeringTuning::reverse_angle_deg`] a goal within
/// [`SteeringTuning::reverse_max_distance`] is backed up to — the throttle
/// goes negative and the steering flips, because backing out is quicker than
/// driving a circle — and one further away is turned to at full lock and the
/// throttle floor, a tight forward turn.
pub fn drive_toward(tuning: &SteeringTuning, hull: &Transform, goal: Vec3) -> (f32, f32) {
    let offset = goal - hull.translation;
    let flat = Vec3::new(offset.x, 0.0, offset.z);

    if flat.length_squared() <= f32::EPSILON {
        return (0.0, 0.0);
    }

    // The hull's forward is −Z, as everywhere else in this crate.
    let forward = hull.rotation * Vec3::NEG_Z;
    let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or(Vec3::NEG_Z);
    let wanted = flat.normalize();

    // Signed angle about +Y, positive for a left turn in a right-handed
    // Y-up frame.
    let angle = forward.cross(wanted).y.atan2(forward.dot(wanted));

    let behind = angle.abs() > tuning.reverse_angle();
    let reversing = behind && flat.length() <= tuning.reverse_max_distance;
    let gain = tuning.steer_gain.max(f32::EPSILON);

    // **Negated.** `PlayerInput::steer` is not the yaw rate: the vehicle's
    // `ground_forces` asks for `wanted_yaw = -steer * turn_rate`, so a
    // *positive* steer is a turn to the **right**. Feeding the left-positive
    // angle straight in steers every enemy away from its target, which
    // presents as a widening spiral rather than as a failure to move — it
    // cost an afternoon, so it is spelt out here.
    let steer = (-angle * gain / std::f32::consts::PI).clamp(-1.0, 1.0);

    if reversing {
        // Backing up: full reverse, and the steering flips because the hull
        // pivots the other way about its rear.
        (-1.0, -steer)
    } else {
        // Slow down through a hard turn, so a full-lock corner is taken
        // rather than understeered through. A goal too far behind to back
        // up to lands here too, at the throttle floor: rolling, a tracked
        // hull turns at `turns[1]`, several times its standstill `turns[0]`.
        let throttle =
            (1.0 - angle.abs() / std::f32::consts::FRAC_PI_2).clamp(tuning.min_engage_speed, 1.0);

        (throttle, steer)
    }
}

/// Bend the steering away from anything the three whiskers hit.
///
/// Three forward rays at hull height — one straight ahead, one to each side
/// at [`SteeringTuning::whisker_angle_deg`] — against level geometry only.
/// The side hits push the steering away from themselves; a centre hit with no
/// side hit picks the side with more room, so a wall met head on is not a
/// stalemate.
pub fn avoid_walls(
    spatial: &SpatialQuery,
    tuning: &SteeringTuning,
    hull: &Transform,
    steering: f32,
) -> f32 {
    if tuning.whisker_length <= 0.0 {
        return steering;
    }

    let origin = hull.translation + Vec3::Y * tuning.whisker_height;
    let angle = tuning.whisker_angle();

    let probe = |offset: f32| -> Option<f32> {
        let direction = hull.rotation * Quat::from_rotation_y(offset) * Vec3::NEG_Z;
        let direction = Dir3::new(Vec3::new(direction.x, 0.0, direction.z)).ok()?;

        let hit = spatial.cast_ray(
            origin,
            direction,
            tuning.whisker_length,
            true,
            &SpatialQueryFilter::from_mask(GameLayer::Level),
        )?;

        // 1 at the nose, 0 at the end of the whisker: a wall two metres away
        // pushes much harder than one at the limit of sight.
        Some(1.0 - (hit.distance / tuning.whisker_length).clamp(0.0, 1.0))
    };

    // A `+angle` rotation about +Y swings the ray to the **left**.
    let left = probe(angle).unwrap_or(0.0);
    let right = probe(-angle).unwrap_or(0.0);
    let centre = probe(0.0).unwrap_or(0.0);

    if left <= 0.0 && right <= 0.0 && centre <= 0.0 {
        return steering;
    }

    // Each side pushes away from itself, in `steer`'s own right-positive
    // sense (see `drive_toward`): a wall on the left wants a positive steer.
    // A centre hit with nothing to either side breaks the tie — toward the
    // right when both are equally clear, so two vehicles meeting head on
    // pass rather than mirroring each other for ever.
    let mut push = left - right;
    if centre > 0.0 && (left - right).abs() < f32::EPSILON {
        push += centre;
    }

    (steering + push).clamp(-1.0, 1.0)
}

// ---------------------------------------------------------------------------
// Aiming and firing
// ---------------------------------------------------------------------------

/// Point every engaging brain's turret at its target and pull the trigger.
///
/// The turret is *not* driven directly: the brain writes
/// [`PlayerInput::aim_point`] and the vehicle's `aim_turret` does the slewing
/// and, for an arcing weapon, the ballistic solution. So an AI mortar lobs
/// its shell exactly the way the player's does, and neither has its own copy
/// of the maths.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn aim(
    mut rng: ResMut<SimRng>,
    weapons: Option<Res<WeaponTable>>,
    control: Option<Res<PlayerControl>>,
    mut requests: MessageWriter<FireRequest>,
    positions: Query<(&GlobalTransform, Option<&LinearVelocity>)>,
    mut brains: Query<(
        Entity,
        &AiBrain,
        &mut PlayerInput,
        &Transform,
        &Turret,
        Option<&WeaponLoadout>,
        Option<&Muzzle>,
    )>,
) {
    let blind = in_cinematic(&control);

    for (entity, brain, mut input, hull, turret, loadout, muzzle) in &mut brains {
        if !brain.state.shoots() || blind {
            input.aim_point = None;
            input.fire = false;
            continue;
        }

        let Some(target) = brain.target else {
            input.aim_point = None;
            input.fire = false;
            continue;
        };
        let Ok((transform, velocity)) = positions.get(target) else {
            input.fire = false;
            continue;
        };

        let muzzle_pos =
            hull.translation + hull.rotation * muzzle.map(|m| m.0).unwrap_or(Vec3::ZERO);

        let weapon = loadout
            .and_then(|loadout| loadout.selected_ammo())
            .zip(weapons.as_ref())
            .and_then(|(ammo, table)| table.by_index(ammo.weapon));

        // A mine is laid, not aimed: see `lay_mine`.
        if weapon.is_some_and(|def| def.mover == Mover::Mine) {
            input.aim_point = None;
            input.fire = false;
            if brain.los_lost == 0 {
                requests.write(lay_mine(entity, hull));
            }
            continue;
        }

        // The projectile's own speed decides the lead, so a slow mortar
        // leads much further than a pulse gun.
        let speed = weapon.and_then(|def| def.velocity).unwrap_or(0.0);

        let lead = lead_point(
            muzzle_pos,
            transform.translation(),
            velocity.map(|velocity| velocity.0).unwrap_or(Vec3::ZERO),
            speed,
            brain.steering.lead_factor,
        );

        let aimed = scatter(&mut rng.0, muzzle_pos, lead, brain.steering.aim_error());
        input.aim_point = Some(aimed);

        // Fire when the barrel is actually near the aim, not merely when a
        // target exists: the turret slews at a finite rate and a brain that
        // fired while still traversing would spray the landscape.
        let barrel = crate::weapons::firing::aim_direction(hull.rotation, *turret);
        let wanted = (aimed - muzzle_pos).normalize_or_zero();
        // And only at a target it can currently see: a unit woken by a hit or
        // by proximity knows where the player is, and tracks them, but does
        // not shell the hill between them.
        let on_target = wanted != Vec3::ZERO
            && brain.los_lost == 0
            && barrel.angle_between(wanted) <= brain.steering.fire_cone();

        input.fire = on_target;

        if on_target {
            // The request goes out here rather than being left to the
            // weapons' `request_fire`, which reads the hull's own turret angles:
            // this way the shot follows the *led* point on the tick the aim
            // lands on it, and the rate limit is still the weapons'.
            requests.write(FireRequest {
                shooter: entity,
                aim_dir: wanted,
                tether_aim: None,
                from: None,
                secondary: false,
            });
        }
    }
}

/// A mine dropped in front of the hull, for a unit whose weapon is one.
///
/// The minelayer's only weapon is P-HEM, and aimed like a gun it was thrown
/// at the player at its 80 m/s and went off on them like a shell. A layer
/// puts its mines down in front of itself instead, released just ahead of
/// the nose and pointed down, and it is the player who has to drive over
/// them — a mine only trips on a hostile, so the layer drives over its own.
/// The weapons' cooldown paces the drops at the row's interval, as it does
/// every other shot.
///
/// // approximation: no reader field says where a mine leaves an enemy; in
/// front is how the original's reads in play. The minelayer's model runs to
/// 4.3 m ahead of its origin, so 6 m is clear of the nose.
pub fn lay_mine(shooter: Entity, hull: &Transform) -> FireRequest {
    FireRequest {
        shooter,
        aim_dir: Vec3::NEG_Y,
        tether_aim: None,
        from: Some(
            hull.translation + hull.rotation * Vec3::new(0.0, MINE_DROP_HEIGHT, -MINE_DROP_AHEAD),
        ),
        secondary: false,
    }
}

/// How far ahead of the hull origin a laid mine is released, in metres.
pub const MINE_DROP_AHEAD: f32 = 6.0;

/// How far above the hull origin a laid mine is released, in metres.
pub const MINE_DROP_HEIGHT: f32 = 1.0;

/// Where to aim to hit a target moving at constant velocity.
///
/// The first-order interception: the target's position plus its velocity over
/// the projectile's flight time, scaled by `factor` so a brain can be made to
/// under-lead deliberately. With a zero speed — a hitscan beam — there is no
/// flight time and the answer is the target itself.
pub fn lead_point(from: Vec3, target: Vec3, velocity: Vec3, speed: f32, factor: f32) -> Vec3 {
    if speed <= 0.0 {
        return target;
    }

    // One iteration of the fixed point: the flight time to where the target
    // is, then to where it will be. A second pass buys nothing at the ranges
    // and speeds in this data (a 325 m shot at 100 m/s against a 12 m/s
    // vehicle moves the answer by centimetres).
    let flight = from.distance(target) / speed;
    let predicted = target + velocity * flight * factor;
    let flight = from.distance(predicted) / speed;

    target + velocity * flight * factor
}

/// Push a point off the true aim by a random angle.
///
/// The error is applied as a rotation about a random axis perpendicular to
/// the line of fire, so it is the same in every direction and scales with
/// range the way a real dispersion does — a two-degree error is 3.5 m at
/// 100 m and 11 m at 325 m.
///
/// `error` is the *maximum*, not a standard deviation: a Gaussian would
/// occasionally produce a wild shot, and the sim's RNG is a uniform stream
/// (the physics' `SimRng`).
pub fn scatter(rng: &mut impl Rng, from: Vec3, at: Vec3, error: f32) -> Vec3 {
    if error <= 0.0 {
        return at;
    }

    let offset = at - from;
    let distance = offset.length();
    if distance <= f32::EPSILON {
        return at;
    }

    let direction = offset / distance;
    // Any vector not parallel to the aim gives a perpendicular basis.
    let reference = if direction.y.abs() < 0.9 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let axis = direction.cross(reference).normalize_or(Vec3::Y);

    let angle = rng.random_range(-error..=error);
    let roll = rng.random_range(0.0..std::f32::consts::TAU);

    let axis = Quat::from_axis_angle(direction, roll) * axis;
    let scattered = Quat::from_axis_angle(axis, angle) * direction;

    from + scattered * distance
}

// ---------------------------------------------------------------------------
// Turrets
// ---------------------------------------------------------------------------

/// Perceive, track and fire for every `ai.zrd` emplacement.
///
/// A turret has no locomotion, so this is the whole of its tick. It writes
/// the yaw and pitch onto its own part entities — the nodes its `PARTS` names
/// — rather than going through `PlayerInput`, because a turret is not a
/// vehicle and has no hull to drive.
#[allow(clippy::too_many_arguments)]
pub fn drive_turrets(
    tick: Res<AiTick>,
    time: Res<Time<Fixed>>,
    table: Option<Res<AiTable>>,
    control: Option<Res<PlayerControl>>,
    tuning: Res<SteeringTuning>,
    mut rng: ResMut<SimRng>,
    weapons: Option<Res<WeaponTable>>,
    spatial: SpatialQuery,
    targets: PerceivableQuery,
    mut requests: MessageWriter<FireRequest>,
    mut turrets: Query<(
        Entity,
        &mut TurretAi,
        &GlobalTransform,
        &Team,
        Option<&crate::damage::Health>,
    )>,
    mut parts: Query<&mut Transform>,
    positions: Query<(&GlobalTransform, Option<&LinearVelocity>)>,
) {
    let Some(table) = table else {
        return;
    };
    let delta = time.delta_secs();
    let blind = in_cinematic(&control);

    for (entity, mut turret, transform, team, health) in &mut turrets {
        let Some(def) = table.turrets.get(turret.def) else {
            continue;
        };
        // A destroyed turret is a wreck: `destroy_turret` has swapped its
        // model, and nothing is left to aim or fire. A brain left running
        // would shoot from under the rubble.
        if blind || turret.deactivated || health.is_some_and(|health| health.is_dead()) {
            turret.target = None;
            continue;
        }

        turret.fire_timer = (turret.fire_timer - delta).max(0.0);
        turret.dwell_left = (turret.dwell_left - delta).max(0.0);

        let here = transform.translation();
        // It looks and shoots from its barrel, not from its origin at the
        // foot of the emplacement.
        let eye = turret
            .muzzles
            .first()
            .and_then(|muzzle| positions.get(*muzzle).ok())
            .map_or(here, |(muzzle, _)| muzzle.translation());

        if tuning.perceives(tick.0, turret.phase) {
            turret.target = nearest_visible(
                &spatial,
                &targets,
                eye,
                *team,
                def.detection_range,
                def.needs_los(),
                &turret.own_nodes,
            )
            .map(|sighting| sighting.entity);
        }

        // `ALWAYS_LOOK_AT` tracks even with nothing acquired, which is what
        // makes m6's guard turrets follow the player past their range.
        let tracked = turret.target.or_else(|| {
            def.always_look_at
                .then(|| {
                    nearest_visible(&spatial, &targets, here, *team, f32::MAX, false, &[])
                        .map(|sighting| sighting.entity)
                })
                .flatten()
        });

        let Some(tracked) = tracked else {
            continue;
        };
        let Ok((target, velocity)) = positions.get(tracked) else {
            turret.target = None;
            continue;
        };

        let speed = weapons
            .as_ref()
            .and_then(|table| table.get(&def.weapon))
            .and_then(|def| def.velocity)
            .unwrap_or(0.0);

        let aimed = lead_point(
            eye,
            target.translation(),
            velocity.map(|velocity| velocity.0).unwrap_or(Vec3::ZERO),
            speed,
            tuning.lead_factor,
        );
        // The barrel tracks the lead point; only the shot is scattered.
        // Scattering the point the parts slew toward made them chase a new
        // random point every tick, which on screen is a shiver.
        let on_target = track_parts(&mut turret, &mut parts, transform, aimed, &tuning, delta);

        // Only an acquired target is shot at: `ALWAYS_LOOK_AT` moves the
        // barrel, it does not pull the trigger.
        if turret.target != Some(tracked) || !on_target {
            continue;
        }

        // From the muzzle whose turn it is, which alternates on a two-barrel
        // emplacement.
        let muzzle = turret
            .muzzles
            .get(turret.next_muzzle)
            .and_then(|muzzle| positions.get(*muzzle).ok())
            .map_or(eye, |(muzzle, _)| muzzle.translation());
        let shot = scatter(&mut rng.0, muzzle, aimed, tuning.aim_error());
        fire_turret(entity, &mut turret, def, muzzle, shot, delta, &mut requests);
    }
}

/// Slew a turret's parts toward a world point; report whether it is on it.
fn track_parts(
    turret: &mut TurretAi,
    parts: &mut Query<&mut Transform>,
    base: &GlobalTransform,
    at: Vec3,
    tuning: &SteeringTuning,
    delta: f32,
) -> bool {
    let (_, rotation, translation) = base.to_scale_rotation_translation();
    let local = rotation.inverse() * (at - translation);

    // The same convention as a vehicle turret: forward is −Z, yaw about +Y.
    let wanted_yaw = (-local.x).atan2(-local.z);
    let wanted_pitch = local.y.atan2(local.xz().length());

    let yaw = crate::vehicle::step_angle(turret.aim.x, wanted_yaw, tuning.turret_yaw_rate * delta);
    let pitch =
        crate::vehicle::step_angle(turret.aim.y, wanted_pitch, tuning.turret_pitch_rate * delta);

    turret.aim = Vec2::new(yaw, pitch);

    // A `BASE_MOVES` turret rotates the whole emplacement rather than a
    // child part, which is how the helicopter's guns track: there is nothing
    // under them to turn.
    if let Some(node) = turret.yaw_node
        && let Ok(mut transform) = parts.get_mut(node)
    {
        transform.rotation = Quat::from_rotation_y(yaw);
    }
    if let Some(node) = turret.pitch_node
        && let Ok(mut transform) = parts.get_mut(node)
    {
        // The pitch part is a child of the yaw part, so its own rotation is
        // pitch alone — unless there is no yaw part, in which case it
        // carries both (the two-element `PARTS` form).
        transform.rotation = if turret.yaw_node.is_some() {
            Quat::from_rotation_x(pitch)
        } else {
            Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0)
        };
    }

    (yaw - wanted_yaw).abs() <= tuning.fire_cone()
        && (pitch - wanted_pitch).abs() <= tuning.fire_cone()
}

/// Pull a turret's trigger, honouring `FIRE_LIMITS` and `FIRE_DWELL`.
///
/// `FIRE_LIMITS` is `[min, max]` seconds of one burst (READER-SCHEMA §3.1) and
/// `FIRE_DWELL` the pause after it. A turret with neither fires continuously
/// at its interval, which is what the 30 entries with no limits do.
fn fire_turret(
    entity: Entity,
    turret: &mut TurretAi,
    def: &TurretDef,
    from: Vec3,
    at: Vec3,
    delta: f32,
    requests: &mut MessageWriter<FireRequest>,
) {
    if turret.dwell_left > 0.0 {
        return;
    }

    // Start a burst. The length is the max of the stated pair: the min is
    // read as the shortest the emplacement commits to and the max as the
    // longest it will hold the trigger, and firing for the longer of the two
    // is what makes a 0.8–2.5 s turret feel like the original's.
    if turret.burst_left <= 0.0 {
        turret.burst_left = def
            .fire_limits
            .map(|[min, max]| max.max(min))
            .unwrap_or(f32::INFINITY);
    }

    turret.burst_left -= delta;

    if turret.fire_timer <= 0.0 {
        let direction = (at - from).normalize_or_zero();
        if direction != Vec3::ZERO {
            requests.write(FireRequest {
                shooter: entity,
                aim_dir: direction,
                tether_aim: None,
                from: Some(from),
                secondary: false,
            });
            // The rate limit is the weapons' cooldown as well; this is the
            // turret's own burst pacing on top, so an emplacement with a
            // `FIRE_DWELL` is not firing on the tick it is meant to pause.
            turret.fire_timer = def.fire_interval.max(0.0);
            turret.next_muzzle = (turret.next_muzzle + 1) % turret.muzzles.len().max(1);
        }
    }

    if turret.burst_left <= 0.0 {
        turret.dwell_left = def.fire_dwell.unwrap_or(0.0);
    }
}

/// Where a turret's shot comes from: its next `firepoint`, else its own
/// origin.
///
/// Attached as a [`Muzzle`] when the parts resolve, so the weapons'
/// `fire_weapons` needs no turret case. Exposed because the spawner and the
/// alternating two-muzzle entry both need the same rule.
pub fn muzzle_of(turret: &TurretAi, parts: &Query<&GlobalTransform>, base: Vec3) -> Vec3 {
    turret
        .muzzles
        .get(turret.next_muzzle)
        .and_then(|entity| parts.get(*entity).ok())
        .map(|transform| transform.translation())
        .unwrap_or(base)
}

// ---------------------------------------------------------------------------
// Death
// ---------------------------------------------------------------------------

/// Stop the brain of anything that was destroyed.
///
/// The *drops* are the damage module's `spawn_drops`, which already reads
/// [`Destroyed`] and [`Drops`](crate::damage::Drops); the death animations
/// — the kind's, a turret's `DESTROY_ANIM`, the weapon's `KILL_ANIMATION` —
/// are [`call_destruction_animations`](crate::anim::call_destruction_animations).
/// This parks the brain, so a dead enemy stops steering and shooting on the
/// tick it dies, and takes its hull out of the physics: the wreck is the death
/// anim's to draw, and it must not block the player or catch shots.
///
/// The body is disabled along with the collider, or a hull with no collider
/// would fall through the terrain with its wreck on it.
pub fn stop_dead_brains(
    mut commands: Commands,
    mut destroyed: MessageReader<Destroyed>,
    mut brains: Query<(&mut AiBrain, &mut PlayerInput)>,
) {
    for event in destroyed.read() {
        if let Ok((mut brain, mut input)) = brains.get_mut(event.entity) {
            brain.state = AiState::Dead;
            brain.target = None;
            *input = PlayerInput::default();
            commands
                .entity(event.entity)
                .insert((ColliderDisabled, RigidBodyDisabled));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::table::tests_support::net_def;
    use crate::vehicle::tuning::VehicleAiDefaults;

    fn brain() -> AiBrain {
        AiBrain {
            net: Some(1),
            state: AiState::Patrol,
            target: None,
            timer: 0.0,
            waypoint: None,
            forward: true,
            home: Vec3::ZERO,
            strategy: Some(AttackStrategy::HeadOn),
            defaults: VehicleAiDefaults::default(),
            los_lost: 0,
            since_seen: 0.0,
            phase: 0,
            steering: SteeringTuning::default(),
            weapon_range: 325.0,
        }
    }

    /// The data's own ranges decide the state, not invented ones.
    #[test]
    fn a_dormant_unit_wakes_only_inside_its_nets_activate_rad() {
        let mut brain = brain();
        brain.state = AiState::Dormant;
        let net = NetDef {
            activate_rad: 100.0,
            ..net_def()
        };

        let far = next_state(
            &brain,
            Some(&net),
            Vec3::ZERO,
            Some(Vec3::new(150.0, 0.0, 0.0)),
        );
        assert_eq!(
            far,
            AiState::Roam,
            "150 m is outside a 100 m radius: it walks, it does not hunt"
        );

        let very_far = next_state(
            &brain,
            Some(&net),
            Vec3::ZERO,
            Some(Vec3::new(brain.steering.roam_range + 50.0, 0.0, 0.0)),
        );
        assert_eq!(
            very_far,
            AiState::Dormant,
            "beyond roam_range it stays parked"
        );

        let near = next_state(
            &brain,
            Some(&net),
            Vec3::ZERO,
            Some(Vec3::new(50.0, 0.0, 0.0)),
        );
        assert_eq!(near, AiState::Engage);

        let blind = next_state(&brain, Some(&net), Vec3::ZERO, None);
        assert_eq!(blind, AiState::Dormant, "nothing seen, nothing to wake for");
    }

    /// A roaming unit is still asleep: it wakes at the same radius a parked
    /// one does, and parks again only once well clear of `roam_range`.
    #[test]
    fn a_roaming_unit_wakes_at_activate_rad_and_parks_past_the_hysteresis() {
        let mut brain = brain();
        brain.state = AiState::Roam;
        let net = NetDef {
            activate_rad: 100.0,
            ..net_def()
        };
        let roam = brain.steering.roam_range;

        let at = |distance: f32| {
            next_state(
                &brain,
                Some(&net),
                Vec3::ZERO,
                Some(Vec3::new(distance, 0.0, 0.0)),
            )
        };

        assert_eq!(at(90.0), AiState::Engage);
        assert_eq!(at(roam * 1.05), AiState::Roam, "inside the hysteresis");
        assert_eq!(at(roam * ROAM_HYSTERESIS + 1.0), AiState::Dormant);
        assert!(!AiState::Roam.awake() && AiState::Roam.moves() && !AiState::Roam.shoots());
    }

    /// The dev knob scales the data's radius rather than replacing it.
    #[test]
    fn the_activation_scale_widens_the_wake_radius() {
        let mut brain = brain();
        brain.state = AiState::Dormant;
        brain.steering.roam_range = 0.0;
        let net = NetDef {
            activate_rad: 100.0,
            ..net_def()
        };
        let target = Some(Vec3::new(150.0, 0.0, 0.0));

        assert_eq!(
            next_state(&brain, Some(&net), Vec3::ZERO, target),
            AiState::Dormant
        );

        brain.steering.activation_scale = 2.0;
        assert_eq!(brain.wake_rad(Some(&net)), 200.0);
        assert_eq!(
            next_state(&brain, Some(&net), Vec3::ZERO, target),
            AiState::Engage
        );
    }

    #[test]
    fn a_unit_dragged_past_its_return_range_goes_home_target_or_not() {
        let mut brain = brain();
        brain.state = AiState::Engage;
        let net = NetDef {
            return_range: 100.0,
            attack_rad: 200.0,
            ..net_def()
        };

        let here = Vec3::new(150.0, 0.0, 0.0);
        let state = next_state(&brain, Some(&net), here, Some(here + Vec3::X * 10.0));

        assert_eq!(state, AiState::Return, "the leash beats the target");
    }

    /// 10 nets state `return_range: 0`, which is no leash rather than a
    /// zero-metre one.
    #[test]
    fn a_zero_return_range_is_no_leash() {
        let mut brain = brain();
        brain.state = AiState::Engage;
        brain.defaults = VehicleAiDefaults::default();
        let net = NetDef {
            return_range: 0.0,
            attack_rad: 200.0,
            ..net_def()
        };

        assert_eq!(brain.return_range(Some(&net)), f32::INFINITY);

        let here = Vec3::new(5000.0, 0.0, 0.0);
        let state = next_state(&brain, Some(&net), here, Some(here + Vec3::X));
        assert_eq!(state, AiState::Engage);
    }

    #[test]
    fn a_returning_unit_stops_short_of_its_home_rather_than_oscillating() {
        let mut brain = brain();
        brain.state = AiState::Return;
        let net = NetDef {
            return_range: 100.0,
            ..net_def()
        };

        // Just inside the leash, but not yet back: still returning.
        let state = next_state(&brain, Some(&net), Vec3::new(90.0, 0.0, 0.0), None);
        assert_eq!(state, AiState::Return);

        // Well inside: done.
        let state = next_state(&brain, Some(&net), Vec3::new(20.0, 0.0, 0.0), None);
        assert_eq!(state, AiState::Patrol);
    }

    #[test]
    fn losing_the_target_drops_out_of_engage() {
        let mut brain = brain();
        brain.state = AiState::Engage;
        let net = net_def();

        assert_eq!(
            next_state(&brain, Some(&net), Vec3::ZERO, None),
            AiState::Patrol
        );
    }

    #[test]
    fn a_dead_brain_stays_dead() {
        let mut brain = brain();
        brain.state = AiState::Dead;

        assert_eq!(
            next_state(&brain, None, Vec3::ZERO, Some(Vec3::X)),
            AiState::Dead
        );
    }

    /// The whole reason the AI writes `PlayerInput` rather than forces.
    #[test]
    fn steering_is_signed_toward_the_goal_and_saturates() {
        let tuning = SteeringTuning::default();
        let hull = Transform::IDENTITY;

        // Dead ahead is −Z.
        let (throttle, steer) = drive_toward(&tuning, &hull, Vec3::new(0.0, 0.0, -100.0));
        assert!(throttle > 0.9, "straight ahead is full throttle");
        assert!(steer.abs() < 1e-5, "and no steering");

        // The sign convention, pinned rather than merely asserted to differ:
        // `PlayerInput::steer` is **right-positive**, because the vehicle's
        // `ground_forces` computes `wanted_yaw = -steer * turn_rate`. A goal
        // to the hull's right therefore wants a positive steer. Getting this
        // backwards makes every enemy spiral away from its target instead of
        // failing outright, which is why it is a test and not a comment.
        //
        // Forward is −Z, so the hull's right is +X.
        let (_, to_the_right) = drive_toward(&tuning, &hull, Vec3::new(100.0, 0.0, -100.0));
        let (_, to_the_left) = drive_toward(&tuning, &hull, Vec3::new(-100.0, 0.0, -100.0));

        assert!(to_the_right > 0.0, "a goal on the right steers right");
        assert!(to_the_left < 0.0, "and one on the left steers left");

        // Hard over saturates rather than growing without bound.
        let (_, hard) = drive_toward(&tuning, &hull, Vec3::new(100.0, 0.0, -1.0));
        assert!(hard.abs() <= 1.0);
    }

    #[test]
    fn a_goal_just_behind_the_hull_reverses() {
        let tuning = SteeringTuning::default();
        let hull = Transform::IDENTITY;

        let (throttle, _) = drive_toward(&tuning, &hull, Vec3::new(0.0, 0.0, 10.0));

        assert!(throttle < 0.0, "backing out beats driving a circle");
    }

    /// Backing toward a goal keeps it behind, so a far one is turned to
    /// instead, or the unit reverses the whole way there.
    #[test]
    fn a_goal_far_behind_the_hull_is_turned_to_going_forwards() {
        let tuning = SteeringTuning::default();
        let hull = Transform::IDENTITY;

        let (throttle, steer) = drive_toward(&tuning, &hull, Vec3::new(5.0, 0.0, 100.0));

        assert_eq!(
            throttle, tuning.min_engage_speed,
            "it does not drive off backwards, and turns slowly rolling"
        );
        assert!(
            steer > 0.9,
            "it turns hard, toward the goal on its right: {steer}"
        );
    }

    #[test]
    fn a_goal_underfoot_asks_for_nothing() {
        let tuning = SteeringTuning::default();
        let hull = Transform::from_xyz(5.0, 2.0, 5.0);

        // Same x and z: the flat offset is zero even though y differs.
        let (throttle, steer) = drive_toward(&tuning, &hull, Vec3::new(5.0, 99.0, 5.0));

        assert_eq!((throttle, steer), (0.0, 0.0));
    }

    /// A lead shot hits a target crossing at a constant velocity, with no
    /// error.
    #[test]
    fn the_lead_point_is_where_the_target_will_be() {
        let from = Vec3::ZERO;
        let target = Vec3::new(0.0, 0.0, -100.0);
        let velocity = Vec3::new(10.0, 0.0, 0.0);
        let speed = 100.0;

        let lead = lead_point(from, target, velocity, speed, 1.0);

        // The shot takes `distance / speed` to arrive, and the target has
        // moved `velocity * flight` by then. Solving exactly: the aim point
        // is where the two meet, so the flight time to the aim point times
        // the velocity is the offset from the target.
        let flight = from.distance(lead) / speed;
        let actually_there = target + velocity * flight;

        assert!(
            lead.distance(actually_there) < 0.05,
            "aimed at {lead}, target will be at {actually_there}"
        );
    }

    /// A layer puts its mine down in front of itself, not at the player.
    #[test]
    fn a_mine_is_laid_ahead_of_the_hull_pointing_down() {
        // Facing +X: forward (−Z) rotated a quarter turn clockwise.
        let hull = Transform::from_xyz(10.0, 0.0, 10.0)
            .with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2));
        let forward = hull.rotation * Vec3::NEG_Z;

        let request = lay_mine(Entity::PLACEHOLDER, &hull);
        let from = request.from.expect("a mine leaves from a stated point");

        assert_eq!(request.aim_dir, Vec3::NEG_Y);
        assert!(
            (from - hull.translation).dot(forward) > 5.0,
            "{from} is not ahead of a hull facing {forward}"
        );
    }

    #[test]
    fn a_hitscan_weapon_does_not_lead() {
        let target = Vec3::new(0.0, 0.0, -100.0);

        let lead = lead_point(Vec3::ZERO, target, Vec3::X * 50.0, 0.0, 1.0);

        assert_eq!(lead, target, "a beam arrives now");
    }

    #[test]
    fn a_stationary_target_is_not_led() {
        let target = Vec3::new(0.0, 0.0, -100.0);

        let lead = lead_point(Vec3::ZERO, target, Vec3::ZERO, 100.0, 1.0);

        assert_eq!(lead, target);
    }

    /// A zero error has to be exactly zero, or the "hits with no error"
    /// tests are testing the RNG.
    #[test]
    fn zero_aim_error_does_not_move_the_point() {
        use rand::SeedableRng;

        let mut rng = rand_chacha::ChaCha8Rng::from_seed([7; 32]);
        let at = Vec3::new(10.0, 0.0, -100.0);

        assert_eq!(scatter(&mut rng, Vec3::ZERO, at, 0.0), at);
    }

    #[test]
    fn aim_error_stays_inside_its_cone_and_keeps_the_range() {
        use rand::SeedableRng;

        let mut rng = rand_chacha::ChaCha8Rng::from_seed([3; 32]);
        let from = Vec3::ZERO;
        let at = Vec3::new(0.0, 0.0, -300.0);
        let error = 2.0_f32.to_radians();

        for _ in 0..200 {
            let scattered = scatter(&mut rng, from, at, error);

            let angle = (at - from).angle_between(scattered - from);
            assert!(
                angle <= error + 1e-4,
                "{angle} rad is outside a {error} rad cone"
            );
            assert!(
                (scattered.distance(from) - 300.0).abs() < 0.1,
                "the error is angular, not radial"
            );
        }
    }

    /// The links are asymmetric in the real data, so the walk must not assume
    /// it can always step back the way it came.
    #[test]
    fn the_patrol_walk_advances_along_a_chain_and_turns_round_at_the_end() {
        use crate::ai::table::NetNode;

        let net = NetDef {
            nodes: (0..4)
                .map(|i| NetNode {
                    pos: Vec3::new(i as f32 * 100.0, 0.0, 0.0),
                    links: [
                        if i > 0 { i - 1 } else { -1 },
                        if i < 3 { i + 1 } else { -1 },
                        -1,
                    ],
                    flag: 12,
                })
                .collect(),
            ..net_def()
        };

        let mut brain = brain();

        // Joining at node 0, it walks toward 1.
        let goal = patrol_point(&mut brain, Some(&net), Vec3::ZERO);
        assert_eq!(goal, Some(net.nodes[1].pos), "arrived at 0, heading for 1");

        // Walk the chain to the end and back, and check it never sticks.
        let mut visited = Vec::new();
        for _ in 0..12 {
            let at = brain
                .waypoint
                .map(|index| net.nodes[index].pos)
                .unwrap_or(Vec3::ZERO);
            visited.push(brain.waypoint);
            patrol_point(&mut brain, Some(&net), at);
        }

        assert!(
            visited.iter().flatten().any(|index| *index == 3),
            "it reaches the far end: {visited:?}"
        );
        assert!(
            visited.iter().flatten().any(|index| *index == 0),
            "and comes back: {visited:?}"
        );
    }

    /// A `fixed` net stays on its node, which is what the type means.
    #[test]
    fn a_fixed_net_holds_its_first_node() {
        use crate::ai::table::NetNode;

        let net = NetDef {
            net_type: NetType::Fixed,
            nodes: vec![
                NetNode {
                    pos: Vec3::new(10.0, 0.0, 0.0),
                    links: [1, -1, -1],
                    flag: 12,
                },
                NetNode {
                    pos: Vec3::new(500.0, 0.0, 0.0),
                    links: [0, -1, -1],
                    flag: 12,
                },
            ],
            ..net_def()
        };

        let mut brain = brain();
        let goal = patrol_point(&mut brain, Some(&net), Vec3::new(10.0, 0.0, 0.0));

        assert_eq!(goal, Some(net.nodes[0].pos), "it does not wander to node 1");
    }

    #[test]
    fn a_sit_strategy_holds_position_and_head_on_closes() {
        let mut brain = brain();
        brain.state = AiState::Engage;
        brain.strategy = Some(AttackStrategy::Sit);
        let net = net_def();

        let target = Vec3::new(0.0, 0.0, -100.0);
        assert_eq!(
            desired_point(&mut brain, Some(&net), Vec3::ZERO, Some(target), Vec3::ZERO),
            None,
            "sit does not drive"
        );

        brain.strategy = Some(AttackStrategy::HeadOn);
        assert_eq!(
            desired_point(&mut brain, Some(&net), Vec3::ZERO, Some(target), Vec3::ZERO),
            Some(target),
            "head_on drives at it"
        );
    }

    /// The `pursuit` standoff is the data's, and head-on has to respect it or
    /// an enemy rams the player instead of shooting.
    #[test]
    fn head_on_stops_at_the_nets_pursuit_standoff() {
        let mut brain = brain();
        brain.state = AiState::Engage;
        brain.strategy = Some(AttackStrategy::HeadOn);
        let net = NetDef {
            pursuit: [30.0, 45.0],
            ..net_def()
        };

        // Inside the near standoff: hold.
        let close = Vec3::new(0.0, 0.0, -20.0);
        assert_eq!(
            desired_point(&mut brain, Some(&net), Vec3::ZERO, Some(close), Vec3::ZERO),
            None
        );

        // Outside it: close.
        let far = Vec3::new(0.0, 0.0, -80.0);
        assert_eq!(
            desired_point(&mut brain, Some(&net), Vec3::ZERO, Some(far), Vec3::ZERO),
            Some(far)
        );
    }

    #[test]
    fn a_circling_unit_moves_across_the_target_rather_than_at_it() {
        let mut brain = brain();
        brain.state = AiState::Engage;
        brain.strategy = Some(AttackStrategy::Circle);
        let net = NetDef {
            pursuit: [40.0, 60.0],
            ..net_def()
        };

        let here = Vec3::new(0.0, 0.0, 50.0);
        let target = Vec3::ZERO;
        let goal = desired_point(&mut brain, Some(&net), here, Some(target), Vec3::ZERO)
            .expect("a circling unit has somewhere to be");

        let to_target = (target - here).normalize();
        let to_goal = (goal - here).normalize();

        assert!(
            to_goal.dot(to_target).abs() < 0.9,
            "the orbit is mostly tangential, not a charge"
        );
    }

    /// `back_forth` has to actually reverse, or it is a very slow `head_on`.
    #[test]
    fn back_and_forth_reverses_every_strafe_period() {
        let mut brain = brain();
        brain.state = AiState::Engage;
        brain.strategy = Some(AttackStrategy::BackForth);
        let net = NetDef {
            pursuit: [30.0, 50.0],
            ..net_def()
        };

        let here = Vec3::new(0.0, 0.0, 50.0);
        let target = Vec3::ZERO;

        brain.timer = 0.0;
        let first = desired_point(&mut brain, Some(&net), here, Some(target), Vec3::ZERO)
            .expect("somewhere to strafe");

        brain.timer = brain.steering.strafe_period + 0.1;
        let second = desired_point(&mut brain, Some(&net), here, Some(target), Vec3::ZERO)
            .expect("somewhere to strafe");

        let across = (first - here).normalize_or_zero();
        let back = (second - here).normalize_or_zero();

        assert!(
            across.dot(back) < 0.5,
            "the second leg goes the other way: {across} then {back}"
        );
    }
}
