//! How the player's model moves on its hull.
//!
//! [`player_model`](crate::player_model) puts `bft_00` on the hull as one
//! rigid piece, so on its own the whole BFT would ride the suspension: it floated half a metre off the ground on its
//! springs, and when the hull pitched on them the tracks lifted with it. This
//! module splits the model into what the ground holds and what the ride
//! moves, and animates the gun:
//!
//! - **The tracks sit on the ground.** A pivot above `body` carries the
//!   model down and round onto the ground plane the suspension rays found
//!   ([`GroundContact`]), so the tracks — and the shadow — stay on the
//!   surface however the hull moves on its springs.
//! - **The upper body leans.** A second pivot above `chassis` (chassis,
//!   turret, gun, wings) pitches back under acceleration, dips under braking
//!   and rolls out of a turn, on a damped spring so it settles with a little
//!   overshoot.
//! - **Weapons swap through the turret doors.** The doors open, the old
//!   weapon sinks into the turret, the new one rises and the doors close.
//! - **A shot kicks the weapon back**, and the pulse guns' `*SCROLL` strips
//!   run while they fire.
//!
//! # What is data and what is ours
//!
//! The nodes are all data: `body`, `chassis`, the `doorleft`/`doorright`
//! panels hinged at the turret's outer edges, the fifteen weapon models under
//! `gun` and the pulse guns' `RFPG_RSCROLL`-style strips. **None of the
//! motion is.** No anim definition, reader key or interp line in the game
//! names a door, a weapon swap, a lean or a recoil — the engine did these
//! itself, and every constant below saying `// approximation` is our
//! reading of how the original looks, to be tuned against footage.
//!
//! Visual only: the sim never reads these pivots except through the
//! firepoints, which move with the gun as they should.

use avian3d::prelude::LinearVelocity;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_sim::level::NodeName;
use bft_sim::states::AppState;
use bft_sim::vehicle::{GroundState, PlayerVehicle, Vehicle, VehicleForm, VehicleTable};
use bft_sim::weapons::{WeaponFired, WeaponLoadout, WeaponTable};

use crate::audio::PlaySound;
use crate::materials::TextureScroll;
use crate::player_model::{PlayerModel, is_side_model, is_weapon_model};

/// The node the ground pivot goes above: tracks, chassis and shadow.
const BODY_NODE: &str = "body";
/// The node the lean pivot goes above: everything that rides the tracks.
const CHASSIS_NODE: &str = "chassis";
/// The two turret-top panels the weapons pass through.
const DOOR_NODES: [&str; 2] = ["doorleft", "doorright"];
/// The suffix of the pulse guns' scrolling strips (`RFPG_RSCROLL`).
const SCROLL_SUFFIX: &str = "SCROLL";

/// How far the doors swing open, radians.
///
/// // approximation: past vertical, so the weapon clears them.
const DOOR_OPEN_ANGLE: f32 = 1.75;
/// How long the doors take to open or close, seconds. // approximation
const DOOR_TIME: f32 = 0.15;
/// How far a weapon sinks to be stowed, metres, along the gun's down axis.
///
/// // approximation: the tallest centre weapon (HEM) reaches 0.34 m above
/// the gun joint and the gun sits 1.0 m above the turret's floor, so 1.2 m
/// takes every weapon out of sight inside the turret.
const STOW_DEPTH: f32 = 1.2;
/// How long a weapon takes to sink or rise, seconds. // approximation
const STOW_TIME: f32 = 0.25;

/// How far a shot kicks its weapon back, metres. // approximation
const RECOIL_DISTANCE: f32 = 0.2;
/// How quickly a kicked weapon returns, per second (exponential).
/// // approximation
const RECOIL_RETURN: f32 = 14.0;
/// How fast the pulse guns' strips scroll while they fire, UV per second.
///
/// // approximation: the scroll nodes have no interp binding, unlike
/// `arc_scroll` (`Object3DSetScroll on 0.0 12.0`), so the engine drives
/// them; half the arc sabre's rate, along the barrel.
const FIRING_SCROLL: Vec2 = Vec2::new(0.0, 6.0);
/// How long the strips keep scrolling after the last shot, seconds.
/// // approximation
const SCROLL_HOLD: f32 = 0.3;

/// Pitch per m/s² of forward acceleration, radians. // approximation
const LEAN_PITCH_PER_ACCEL: f32 = 0.004;
/// Roll per m/s² of sideways acceleration, radians. // approximation
const LEAN_ROLL_PER_ACCEL: f32 = 0.005;
/// The most the upper body leans either way, radians (about 5°).
/// // approximation
const LEAN_MAX: f32 = 0.09;
/// The lean spring's natural frequency, Hz. // approximation
const LEAN_FREQUENCY: f32 = 1.6;
/// The lean spring's damping ratio: under 1, so it overshoots a little.
/// // approximation
const LEAN_DAMPING: f32 = 0.45;
/// How fast the acceleration the lean reads is smoothed, per second.
const ACCEL_SMOOTHING: f32 = 10.0;

/// How fast the tracks follow the ground, per second (exponential).
const HUG_RATE: f32 = 18.0;
/// The most the tracks tilt away from the hull, radians (30°): beyond it the
/// rays are reading a ledge, not a slope.
const HUG_MAX_TILT: f32 = 0.52;
/// The furthest the tracks drop below the hull origin, metres. Past the
/// suspension's reach, which is where the rays stop finding ground.
const HUG_MAX_DROP: f32 = 1.5;
/// The furthest they rise above it.
const HUG_MAX_RISE: f32 = 0.5;

/// The player's model in motion: ground hug, lean, weapon swap and recoil.
pub struct PlayerRigPlugin;

impl Plugin for PlayerRigPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                build_rig,
                (hug_ground, lean_body, swap_weapons, fire_weapons_visuals),
            )
                .chain()
                .run_if(in_state(AppState::InGame)),
        );
    }
}

/// The parts of the player's model this module moves.
#[derive(Component, Debug, Clone)]
pub struct PlayerRig {
    /// Above `body`: carries the tracks onto the ground.
    pub ground_pivot: Entity,
    /// Above `chassis`: the lean.
    pub lean_pivot: Option<Entity>,
    /// Where the lean turns about, in `body` space: the chassis's origin.
    pub lean_origin: Vec3,
    /// The turret doors, with their rest pose and which way each opens.
    pub doors: Vec<Part>,
    /// The weapons under the doors, by node name.
    pub centre: Vec<(String, Part)>,
    /// Every weapon model, for the recoil, by node name.
    pub weapons: Vec<(String, Part)>,
    /// The scrolling strips, with the weapon node each belongs to.
    pub scrolls: Vec<(String, Entity)>,
}

/// A node this module moves, and where it rests.
#[derive(Debug, Clone, Copy)]
pub struct Part {
    pub entity: Entity,
    pub rest: Transform,
    /// For a door, which way it swings: the sign that lifts its inner edge.
    pub sign: f32,
}

/// What the rig is doing.
#[derive(Component, Debug, Clone, Default)]
pub struct RigState {
    /// The ground pivot's pose as last written, for the smoothing.
    pub hug: Transform,
    /// Upper-body pitch and roll, radians, and their rates.
    pub lean: Vec2,
    pub lean_rate: Vec2,
    /// Hull-space acceleration, smoothed.
    pub accel: Vec3,
    /// The hull's velocity last frame, world space.
    pub last_velocity: Option<Vec3>,
    /// The centre weapon out of the turret, or on its way in or out.
    pub shown: Option<String>,
    /// Whether [`Self::shown`] has been decided once: the first choice is
    /// made without an animation, so a level does not open on a swap.
    pub settled: bool,
    /// How open the doors are, 0–1.
    pub door: f32,
    /// How far [`Self::shown`] has sunk, 0 (out) – 1 (stowed).
    pub stow: f32,
    /// Each weapon's recoil, metres, by entity.
    pub kick: HashMap<Entity, f32>,
    /// Seconds each weapon's strips keep scrolling, by weapon node name.
    pub scrolling: HashMap<String, f32>,
}

/// Insert the two pivots and find the parts, once the model is on the hull.
#[allow(clippy::type_complexity)]
pub fn build_rig(
    mut commands: Commands,
    players: Query<(Entity, &PlayerModel), (With<PlayerVehicle>, Without<PlayerRig>)>,
    children: Query<&Children>,
    names: Query<&NodeName>,
    parents: Query<&ChildOf>,
    transforms: Query<&Transform>,
) {
    for (hull, model) in &players {
        let find = |name: &str| find_named(model.root, name, &children, &names);
        let Some(body) = find(BODY_NODE) else {
            // Not ready, or no such node: try again next frame rather than
            // building a rig that moves nothing. Every level has `body`.
            continue;
        };
        let Ok(body_parent) = parents.get(body).map(ChildOf::parent) else {
            continue;
        };

        let ground_pivot = commands
            .spawn((
                Name::new("ground pivot"),
                Transform::IDENTITY,
                Visibility::Inherited,
                ChildOf(body_parent),
            ))
            .id();
        commands.entity(body).insert(ChildOf(ground_pivot));

        let (lean_pivot, lean_origin) = match find(CHASSIS_NODE) {
            Some(chassis) => {
                let origin = transforms
                    .get(chassis)
                    .map(|transform| transform.translation)
                    .unwrap_or_default();
                let parent = parents.get(chassis).map(ChildOf::parent).unwrap_or(body);
                let pivot = commands
                    .spawn((
                        Name::new("lean pivot"),
                        Transform::IDENTITY,
                        Visibility::Inherited,
                        ChildOf(parent),
                    ))
                    .id();
                commands.entity(chassis).insert(ChildOf(pivot));
                (Some(pivot), origin)
            }
            None => (None, Vec3::ZERO),
        };

        let part = |entity: Entity, sign: f32| Part {
            entity,
            rest: transforms.get(entity).copied().unwrap_or_default(),
            sign,
        };

        let doors = DOOR_NODES
            .iter()
            .filter_map(|name| find(name))
            .map(|door| {
                // Hinged at the outer edge, which is the node's origin; the
                // panel reaches inward from it. Lifting the inner edge is a
                // turn about the door's own z, positive for the door on the
                // left of the centre line.
                let x = transforms
                    .get(door)
                    .map(|transform| transform.translation.x)
                    .unwrap_or(0.0);
                part(door, if x < 0.0 { 1.0 } else { -1.0 })
            })
            .collect();

        let weapons: Vec<(String, Part)> = model
            .weapons
            .iter()
            .map(|(name, entity)| (name.clone(), part(*entity, 0.0)))
            .collect();
        let centre = weapons
            .iter()
            .filter(|(name, _)| !is_side_model(name))
            .cloned()
            .collect();

        let mut scrolls = Vec::new();
        for (name, entity) in &model.weapons {
            let Ok(kids) = children.get(*entity) else {
                continue;
            };
            for kid in kids.iter() {
                if names
                    .get(kid)
                    .is_ok_and(|kid_name| kid_name.0.ends_with(SCROLL_SUFFIX))
                {
                    // Standing still until the gun fires. The component has
                    // to be there before the parts are claimed, so they get
                    // materials of their own (`claim_animated_parts`).
                    commands.entity(kid).insert(TextureScroll::new(Vec2::ZERO));
                    scrolls.push((name.clone(), kid));
                }
            }
        }

        commands.entity(hull).insert((
            PlayerRig {
                ground_pivot,
                lean_pivot,
                lean_origin,
                doors,
                centre,
                weapons,
                scrolls,
            },
            RigState::default(),
        ));
    }
}

/// The first node of this name under `root`, breadth-first.
fn find_named(
    root: Entity,
    name: &str,
    children: &Query<&Children>,
    names: &Query<&NodeName>,
) -> Option<Entity> {
    let mut queue = std::collections::VecDeque::from([root]);
    while let Some(entity) = queue.pop_front() {
        if names.get(entity).is_ok_and(|found| found.0 == name) {
            return Some(entity);
        }
        if let Ok(kids) = children.get(entity) {
            queue.extend(kids.iter());
        }
    }
    None
}

/// Where the ground pivot wants to be for this contact: the model's origin
/// on the ground plane, its up along the ground's.
///
/// `None` when the tracks should not be on the ground: airborne, floating,
/// or a form that does not stand on it.
pub fn ground_pose(ground: &GroundState, form: VehicleForm) -> Option<Transform> {
    if !matches!(form, VehicleForm::Track | VehicleForm::Amphib) || ground.is_in_water() {
        return None;
    }
    let contact = ground.contact?;
    let tilt = contact.normal.angle_between(Vec3::Y);
    if !tilt.is_finite() || tilt > HUG_MAX_TILT {
        return None;
    }
    Some(Transform {
        translation: Vec3::Y * contact.height.clamp(-HUG_MAX_DROP, HUG_MAX_RISE),
        rotation: Quat::from_rotation_arc(Vec3::Y, contact.normal),
        scale: Vec3::ONE,
    })
}

/// Put the tracks on the ground the suspension rays found.
pub fn hug_ground(
    time: Res<Time>,
    mut players: Query<(&PlayerRig, &mut RigState, &GroundState, &Vehicle)>,
    mut transforms: Query<&mut Transform>,
) {
    let blend = 1.0 - (-HUG_RATE * time.delta_secs()).exp();
    for (rig, mut state, ground, vehicle) in &mut players {
        // Off the ground the model hangs where `player_model` put it, and eases
        // back there rather than snapping.
        let wanted = ground_pose(ground, vehicle.form).unwrap_or_default();
        let hug = Transform {
            translation: state.hug.translation.lerp(wanted.translation, blend),
            rotation: state.hug.rotation.slerp(wanted.rotation, blend),
            scale: Vec3::ONE,
        };
        state.hug = hug;
        if let Ok(mut transform) = transforms.get_mut(rig.ground_pivot)
            && *transform != hug
        {
            *transform = hug;
        }
    }
}

/// The lean a hull-space acceleration asks for: (pitch, roll), radians.
///
/// Pitch is positive nose-up, which is what accelerating forward (along −z)
/// does to a body on springs; roll is positive right-side-up, and a body
/// rolls away from the way it is being pushed, so a left turn — pushed left,
/// toward −x — rolls it onto its right side.
pub fn lean_target(accel: Vec3) -> Vec2 {
    let forward = -accel.z;
    Vec2::new(
        (forward * LEAN_PITCH_PER_ACCEL).clamp(-LEAN_MAX, LEAN_MAX),
        (accel.x * LEAN_ROLL_PER_ACCEL).clamp(-LEAN_MAX, LEAN_MAX),
    )
}

/// One step of the lean spring: a damped oscillator toward `target`.
pub fn step_lean(lean: Vec2, rate: Vec2, target: Vec2, dt: f32) -> (Vec2, Vec2) {
    let omega = std::f32::consts::TAU * LEAN_FREQUENCY;
    // Semi-implicit Euler, split into small steps so a slow frame cannot
    // make the spring explode.
    let steps = (dt / (1.0 / 240.0)).ceil().max(1.0);
    let h = dt / steps;
    let (mut lean, mut rate) = (lean, rate);
    for _ in 0..steps as usize {
        let accel = (target - lean) * omega * omega - rate * 2.0 * LEAN_DAMPING * omega;
        rate += accel * h;
        lean += rate * h;
    }
    (
        lean.clamp(Vec2::splat(-LEAN_MAX * 1.5), Vec2::splat(LEAN_MAX * 1.5)),
        rate,
    )
}

/// Lean the upper body with the hull's acceleration.
pub fn lean_body(
    time: Res<Time>,
    mut players: Query<(&PlayerRig, &mut RigState, &GlobalTransform, &LinearVelocity)>,
    mut transforms: Query<&mut Transform>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (rig, mut state, hull, velocity) in &mut players {
        let Some(pivot) = rig.lean_pivot else {
            continue;
        };

        // The velocity only changes on a fixed step, so a frame with no step
        // reads no acceleration and the next a spike; the smoothing spreads
        // them back out.
        let raw = state
            .last_velocity
            .map(|last| (velocity.0 - last) / dt)
            .unwrap_or_default();
        state.last_velocity = Some(velocity.0);
        let local = hull.rotation().inverse() * raw;
        let blend = 1.0 - (-ACCEL_SMOOTHING * dt).exp();
        state.accel = state.accel.lerp(local, blend);

        let (lean, rate) = step_lean(state.lean, state.lean_rate, lean_target(state.accel), dt);
        state.lean = lean;
        state.lean_rate = rate;

        let rotation = Quat::from_rotation_x(lean.x) * Quat::from_rotation_z(lean.y);
        // About the chassis's own origin, not the pivot's: the pivot sits at
        // `body`'s origin, on the ground.
        let pose = Transform {
            translation: rig.lean_origin - rotation * rig.lean_origin,
            rotation,
            scale: Vec3::ONE,
        };
        if let Ok(mut transform) = transforms.get_mut(pivot)
            && *transform != pose
        {
            *transform = pose;
        }
    }
}

/// Which centre weapon model the loadout wants out of the turret.
///
/// `None` with the pulse gun selected: its models are the side cannons,
/// which [`show_selected_weapon`](crate::player_model::show_selected_weapon)
/// shows directly.
pub fn wanted_centre(
    rig: &PlayerRig,
    loadout: &WeaponLoadout,
    table: &WeaponTable,
) -> Option<String> {
    let weapon = loadout
        .trigger(false)
        .and_then(|(slot, strong)| table.slot(slot, strong))?;
    rig.centre
        .iter()
        .find(|(name, _)| is_weapon_model(name, &weapon.hud_name))
        .map(|(name, _)| name.clone())
}

/// One step of the swap: open, stow the old weapon, raise the new, close.
///
/// Returns the new `(door, stow, shown)`. Continuous rather than a sequence
/// of phases, so a second selection mid-swap turns the motion round instead
/// of queueing a second swap behind the first.
pub fn step_swap(
    door: f32,
    stow: f32,
    shown: Option<String>,
    wanted: &Option<String>,
    dt: f32,
) -> (f32, f32, Option<String>) {
    let door_step = dt / DOOR_TIME;
    let stow_step = dt / STOW_TIME;
    let (mut door, mut stow, mut shown) = (door, stow, shown);

    if shown != *wanted {
        if shown.is_none() {
            // Nothing to put away: the new weapon starts stowed.
            shown = wanted.clone();
            stow = 1.0;
        } else if door < 1.0 {
            door = (door + door_step).min(1.0);
        } else {
            stow = (stow + stow_step).min(1.0);
            if stow >= 1.0 {
                shown = wanted.clone();
            }
        }
    } else if shown.is_some() && stow > 0.0 {
        if door < 1.0 {
            door = (door + door_step).min(1.0);
        } else {
            stow = (stow - stow_step).max(0.0);
        }
    } else {
        door = (door - door_step).max(0.0);
    }
    (door, stow, shown)
}

/// Swap the centre weapon through the turret doors.
#[allow(clippy::too_many_arguments)]
pub fn swap_weapons(
    time: Res<Time>,
    table: Option<Res<WeaponTable>>,
    vehicles: Option<Res<VehicleTable>>,
    mut play: MessageWriter<PlaySound>,
    mut players: Query<(Entity, &PlayerRig, &mut RigState, &WeaponLoadout, &Vehicle)>,
    mut transforms: Query<&mut Transform>,
    mut visibilities: Query<&mut Visibility>,
) {
    let Some(table) = table else {
        return;
    };
    for (hull, rig, mut state, loadout, vehicle) in &mut players {
        let wanted = wanted_centre(rig, loadout, &table);
        if !state.settled {
            state.shown = wanted.clone();
            state.settled = true;
        }
        let (door, stow, shown) = step_swap(
            state.door,
            state.stow,
            state.shown.clone(),
            &wanted,
            time.delta_secs(),
        );
        // `vehicle.zrd`'s `weapon_down` as a weapon starts to sink, and
        // `weapon_up` as the next starts to rise; `weapon_select` already
        // played on the selection itself (`audio::bridges`).
        let sounds = vehicles
            .as_ref()
            .and_then(|vehicles| vehicles.kind(&vehicle.kind))
            .map(|def| &def.sounds);
        let sinking = state.stow == 0.0 && stow > 0.0 && shown == state.shown;
        let rising = shown != state.shown && shown.is_some();
        let sound = match (sinking, rising) {
            (true, _) => sounds.and_then(|sounds| sounds.weapon_down.as_ref()),
            (_, true) => sounds.and_then(|sounds| sounds.weapon_up.as_ref()),
            _ => None,
        };
        if let Some(sound) = sound {
            play.write(PlaySound::on(sound, hull));
        }

        state.door = door;
        state.stow = stow;
        state.shown = shown;

        for door_part in &rig.doors {
            let angle = smooth(state.door) * DOOR_OPEN_ANGLE * door_part.sign;
            let pose = Transform {
                rotation: door_part.rest.rotation * Quat::from_rotation_z(angle),
                ..door_part.rest
            };
            write(&mut transforms, door_part.entity, pose);
        }

        for (name, part) in &rig.centre {
            let out = state.shown.as_deref() == Some(name.as_str());
            let visibility = if out {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if let Ok(mut current) = visibilities.get_mut(part.entity)
                && *current != visibility
            {
                *current = visibility;
            }
            let sink = if out { smooth(state.stow) } else { 0.0 };
            let kick = state.kick.get(&part.entity).copied().unwrap_or(0.0);
            let pose = Transform {
                translation: part.rest.translation
                    + part.rest.rotation * Vec3::new(0.0, -sink * STOW_DEPTH, kick),
                ..part.rest
            };
            write(&mut transforms, part.entity, pose);
        }
    }
}

/// Ease in and out, so a door or a weapon does not start or stop dead.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Write a transform only when it changed, so change detection stays quiet.
fn write(transforms: &mut Query<&mut Transform>, entity: Entity, pose: Transform) {
    if let Ok(mut transform) = transforms.get_mut(entity)
        && *transform != pose
    {
        *transform = pose;
    }
}

/// Kick the weapon that fired, and run its strips.
#[allow(clippy::type_complexity)]
pub fn fire_weapons_visuals(
    time: Res<Time>,
    table: Option<Res<WeaponTable>>,
    mut fired: MessageReader<WeaponFired>,
    mut players: Query<(Entity, &PlayerRig, &mut RigState)>,
    mut transforms: Query<&mut Transform>,
    children: Query<&Children>,
    mut scrolls: Query<&mut TextureScroll>,
) {
    let Some(table) = table else {
        fired.clear();
        return;
    };
    let dt = time.delta_secs();

    for shot in fired.read() {
        let Ok((_, rig, mut state)) = players.get_mut(shot.shooter) else {
            continue;
        };
        let Some(def) = table.by_index(shot.weapon) else {
            continue;
        };
        // The pulse guns' pair fire in turn, and only the barrel that fired
        // kicks: `_R` is barrel 0, `fpnt_r`'s.
        let side = shot
            .barrel
            .map(|barrel| if barrel == 0 { "_R" } else { "_L" });
        for (name, part) in &rig.weapons {
            if !is_weapon_model(name, &def.hud_name) {
                continue;
            }
            if let Some(side) = side
                && !name.ends_with(side)
            {
                continue;
            }
            state.kick.insert(part.entity, RECOIL_DISTANCE);
            state.scrolling.insert(name.clone(), SCROLL_HOLD);
        }
    }

    let decay = (-RECOIL_RETURN * dt).exp();
    for (_, rig, mut state) in &mut players {
        state.kick.retain(|_, kick| {
            *kick *= decay;
            *kick > 1e-3
        });
        for time_left in state.scrolling.values_mut() {
            *time_left -= dt;
        }
        state.scrolling.retain(|_, time_left| *time_left > 0.0);

        // The centre weapons' kick is written with their stow in
        // `swap_weapons`; the side cannons are not stowed and take theirs
        // here.
        for (name, part) in &rig.weapons {
            if !is_side_model(name) {
                continue;
            }
            let kick = state.kick.get(&part.entity).copied().unwrap_or(0.0);
            let pose = Transform {
                translation: part.rest.translation + part.rest.rotation * Vec3::new(0.0, 0.0, kick),
                ..part.rest
            };
            write(&mut transforms, part.entity, pose);
        }

        for (weapon, strip) in &rig.scrolls {
            let rate = if state.scrolling.contains_key(weapon) {
                FIRING_SCROLL
            } else {
                Vec2::ZERO
            };
            // The strip's parts carry the scroll, one copy each
            // (`claim_animated_parts`).
            let mut stack = vec![*strip];
            while let Some(entity) = stack.pop() {
                if let Ok(mut scroll) = scrolls.get_mut(entity)
                    && scroll.rate != rate
                {
                    scroll.rate = rate;
                }
                if let Ok(kids) = children.get(entity) {
                    stack.extend(kids.iter());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_sim::vehicle::GroundContact;

    fn on_ground(normal: Vec3, height: f32) -> GroundState {
        GroundState {
            grounded: true,
            contact: Some(GroundContact { normal, height }),
            ..GroundState::airborne()
        }
    }

    /// Springs holding the hull half a metre up put the tracks half a metre
    /// down, and a slope under the hull tilts them onto it.
    #[test]
    fn the_tracks_go_down_and_round_onto_the_ground() {
        let flat = ground_pose(&on_ground(Vec3::Y, -0.5), VehicleForm::Track).expect("on ground");
        assert!((flat.translation.y + 0.5).abs() < 1e-5);
        assert!(flat.rotation.angle_between(Quat::IDENTITY) < 1e-5);

        let slope = Vec3::new(0.2, 1.0, 0.0).normalize();
        let tilted = ground_pose(&on_ground(slope, -0.4), VehicleForm::Track).expect("on ground");
        assert!((tilted.rotation * Vec3::Y).distance(slope) < 1e-4);
    }

    /// Hovering, airborne, or on a ledge the rays misread, the model stays
    /// on the hull.
    #[test]
    fn the_tracks_leave_the_ground_alone_when_they_should() {
        assert!(ground_pose(&on_ground(Vec3::Y, -0.5), VehicleForm::Hover).is_none());
        assert!(ground_pose(&GroundState::airborne(), VehicleForm::Track).is_none());
        let cliff = Vec3::new(1.0, 0.3, 0.0).normalize();
        assert!(ground_pose(&on_ground(cliff, -0.5), VehicleForm::Track).is_none());
    }

    /// Accelerating lifts the nose, braking dips it, and a left turn rolls
    /// the body onto its right side.
    #[test]
    fn the_body_leans_against_the_push() {
        assert!(lean_target(Vec3::new(0.0, 0.0, -10.0)).x > 0.0);
        assert!(lean_target(Vec3::new(0.0, 0.0, 10.0)).x < 0.0);
        assert!(lean_target(Vec3::new(-10.0, 0.0, 0.0)).y < 0.0);
        assert_eq!(lean_target(Vec3::new(0.0, 0.0, -1000.0)).x, LEAN_MAX);
    }

    /// The spring settles on its target, overshooting on the way.
    #[test]
    fn the_lean_settles_with_a_little_overshoot() {
        let target = Vec2::new(0.05, 0.0);
        let (mut lean, mut rate) = (Vec2::ZERO, Vec2::ZERO);
        let mut peak: f32 = 0.0;
        for _ in 0..600 {
            (lean, rate) = step_lean(lean, rate, target, 1.0 / 60.0);
            peak = peak.max(lean.x);
        }
        assert!((lean.x - 0.05).abs() < 1e-3, "settled at {}", lean.x);
        assert!(peak > 0.05, "no overshoot: peak {peak}");
    }

    /// A swap opens the doors, stows the old weapon, raises the new one and
    /// closes the doors, in that order.
    #[test]
    fn a_swap_opens_stows_raises_and_closes() {
        let wanted = Some("HEM".to_string());
        let (mut door, mut stow, mut shown) = (0.0, 0.0, Some("LASER".to_string()));
        let mut order = Vec::new();
        for _ in 0..200 {
            (door, stow, shown) = step_swap(door, stow, shown, &wanted, 1.0 / 60.0);
            let phase = match (shown.as_deref(), door, stow) {
                (Some("LASER"), d, 0.0) if d < 1.0 => "open",
                (Some("LASER"), _, _) => "stow",
                (Some("HEM"), _, s) if s > 0.0 => "raise",
                (Some("HEM"), d, _) if d > 0.0 => "close",
                _ => "done",
            };
            if order.last() != Some(&phase) {
                order.push(phase);
            }
        }
        assert_eq!(order, ["open", "stow", "raise", "close", "done"]);
    }

    /// Changing one's mind mid-swap sends the rising weapon back down
    /// rather than finishing and swapping again.
    #[test]
    fn a_second_selection_turns_the_swap_round() {
        let (mut door, mut stow, mut shown) = (1.0, 0.6, Some("HEM".to_string()));
        let wanted = Some("LASER".to_string());
        (door, stow, shown) = step_swap(door, stow, shown, &wanted, 1.0 / 60.0);
        assert_eq!(shown.as_deref(), Some("HEM"));
        assert!(stow > 0.6, "the rising weapon should sink again");
        assert_eq!(door, 1.0);
    }
}
