//! The vehicle and warning sounds the data names: skid, landing, collision,
//! sub sonar, low shields and the weapon warnings.
//!
//! - `vehicle.zrd` `<form>_mode/sounds`: `skid` while the tracks slide,
//!   `collide` on a hard knock, `land` coming down from a jump.
//! - `vehicle.zrd` `common_mode/sounds/pinging`: `snd_ping`, a looped
//!   1.7 s ping. // approximation: the data does not say when it plays; it
//!   is read as the sub's sonar, looping while submerged.
//! - `player.zrd` `low_shield_snd`: the `snd_lowshield` voice once as the
//!   shields fall below the threshold, and the `snd_warn3` alarm looping
//!   while they stay there. // approximation: the threshold (10) is read
//!   as a percentage of full shields.
//! - `weapons.zrd`'s warnings: `LOCK_ON_WARNING` as an enemy missile locks
//!   onto the player, `NO_AMMO_WARNING` and the two `*_INACTIVE` clicks when
//!   a pull or a selection is refused.
//!
//! Every name comes from the data; only the "when" is ours, and each rule
//! below says so where it is a reading rather than a statement.

use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bft_sim::cinematic::PlayerControl;
use bft_sim::damage::Health;
use bft_sim::states::AppState;
use bft_sim::vehicle::{
    GroundState, PlayerVehicle, Traction, Vehicle, VehicleCollided, VehicleForm, VehicleTable,
};
use bft_sim::weapons::{Mover, Projectile, Refusal, TriggerRefused, WeaponTable};

use super::{AudioSystems, PlaySound};

/// How long a vehicle has to be off the ground for touching down to be a
/// landing, seconds. Rolling over a bump is not. // approximation
pub const LANDING_AIRTIME: f32 = 0.35;
/// How long after a landing a collision is taken to be the same impact,
/// seconds: the hull box touching down is a collision too.
pub const LANDING_MASKS_COLLISION: f32 = 0.3;
/// The least time between two collision sounds from one vehicle, seconds.
pub const COLLIDE_GAP: f32 = 0.3;
/// How often a skid sound restarts while the slide goes on, seconds: a
/// little under `skid.wav`'s 2.4 s, so a long slide is one continuous scrape.
pub const SKID_REPEAT: f32 = 2.2;
/// The least time between two refusal sounds of one kind, seconds: the sim
/// refuses every tick the trigger is held.
pub const REFUSAL_GAP: f32 = 1.0;
/// The low-shield threshold as a fraction of full shields, from
/// `low_shield_snd`'s 10.
pub fn low_shield_fraction(threshold: f32) -> f32 {
    threshold / 100.0
}

pub struct CuesPlugin;

impl Plugin for CuesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CueState>().add_systems(
            Update,
            (
                skid_sounds,
                impact_sounds,
                ping_sound,
                shield_warnings,
                lock_on_warnings,
                refusal_sounds,
            )
                .in_set(AudioSystems::Bridge)
                .run_if(in_state(AppState::InGame)),
        );
    }
}

/// What the cue systems remember between frames.
#[derive(Resource, Debug, Default)]
pub struct CueState {
    /// Per vehicle: seconds until a skid may sound again.
    skid: HashMap<Entity, f32>,
    /// Per vehicle: seconds in the air, and seconds since the last landing.
    air: HashMap<Entity, (f32, f32)>,
    /// Per vehicle: seconds since the last collision sound.
    collide: HashMap<Entity, f32>,
    /// The anchor the ping loops on, while it does.
    ping: Option<Entity>,
    /// The anchor the low-shield alarm loops on, while it does.
    alarm: Option<Entity>,
    /// The missiles already announced.
    locked: HashSet<Entity>,
    /// Per refusal: seconds until it may sound again.
    refused: HashMap<Refusal, f32>,
}

/// The mode block's sounds for a vehicle in its current form.
fn mode_sounds<'a>(
    table: &'a VehicleTable,
    vehicle: &Vehicle,
) -> Option<&'a bft_sim::vehicle::tuning::ModeSounds> {
    table
        .kind(&vehicle.kind)
        .and_then(|def| def.mode(vehicle.form))
        .map(|mode| &mode.sounds)
}

/// The mode's `skid` as a slide starts, and again while it lasts.
pub fn skid_sounds(
    time: Res<Time>,
    table: Option<Res<VehicleTable>>,
    mut state: ResMut<CueState>,
    mut play: MessageWriter<PlaySound>,
    vehicles: Query<(Entity, &Vehicle, &Traction)>,
) {
    let Some(table) = table else {
        return;
    };
    let dt = time.delta_secs();
    let mut seen = HashSet::new();
    for (entity, vehicle, traction) in &vehicles {
        seen.insert(entity);
        let wait = state.skid.entry(entity).or_insert(0.0);
        *wait = (*wait - dt).max(0.0);
        if !traction.is_skidding() {
            // Let the next slide sound at once.
            *wait = 0.0;
            continue;
        }
        if *wait > 0.0 {
            continue;
        }
        *wait = SKID_REPEAT;
        if let Some(sound) = mode_sounds(&table, vehicle).and_then(|sounds| sounds.skid.as_ref()) {
            play.write(PlaySound::on(sound, entity));
        }
    }
    state.skid.retain(|entity, _| seen.contains(entity));
}

/// Whether touching down after `airtime` seconds in the air is a landing.
pub fn is_landing(was_grounded: bool, grounded: bool, airtime: f32) -> bool {
    !was_grounded && grounded && airtime >= LANDING_AIRTIME
}

/// The mode's `land` on touching down after a jump, and `collide` on a hard
/// knock that is not that landing.
#[allow(clippy::type_complexity)]
pub fn impact_sounds(
    time: Res<Time>,
    table: Option<Res<VehicleTable>>,
    mut state: ResMut<CueState>,
    mut play: MessageWriter<PlaySound>,
    mut collided: MessageReader<VehicleCollided>,
    vehicles: Query<(Entity, &Vehicle, &GroundState)>,
    mut grounded_before: Local<HashMap<Entity, bool>>,
) {
    let Some(table) = table else {
        collided.clear();
        return;
    };
    let dt = time.delta_secs();
    for value in state.collide.values_mut() {
        *value += dt;
    }

    let mut seen = HashSet::new();
    for (entity, vehicle, ground) in &vehicles {
        seen.insert(entity);
        let was = grounded_before
            .insert(entity, ground.grounded)
            .unwrap_or(true);
        let (airtime, since_landing) = state.air.entry(entity).or_insert((0.0, f32::MAX));
        *since_landing += dt;
        if is_landing(was, ground.grounded, *airtime) {
            *since_landing = 0.0;
            if let Some(sound) =
                mode_sounds(&table, vehicle).and_then(|sounds| sounds.land.as_ref())
            {
                play.write(PlaySound::on(sound, entity));
            }
        }
        *airtime = if ground.grounded { 0.0 } else { *airtime + dt };
    }
    state.air.retain(|entity, _| seen.contains(entity));
    grounded_before.retain(|entity, _| seen.contains(entity));

    for event in collided.read() {
        let Ok((entity, vehicle, _)) = vehicles.get(event.entity) else {
            continue;
        };
        let landed = state
            .air
            .get(&entity)
            .is_some_and(|(_, since)| *since < LANDING_MASKS_COLLISION);
        let since = state.collide.entry(entity).or_insert(f32::MAX);
        if landed || *since < COLLIDE_GAP {
            continue;
        }
        *since = 0.0;
        if let Some(sound) = mode_sounds(&table, vehicle).and_then(|sounds| sounds.collide.as_ref())
        {
            play.write(PlaySound::on(sound, entity));
        }
    }
}

/// Start or stop a loop on an anchor under `parent`.
///
/// A looped [`PlaySound`] lasts as long as the entity it is attached to, so
/// a loop the game has to stop hangs off an anchor of its own.
fn keep_loop(
    commands: &mut Commands,
    play: &mut MessageWriter<PlaySound>,
    anchor: &mut Option<Entity>,
    wanted: Option<(Entity, &str)>,
) {
    match (wanted, *anchor) {
        (Some((parent, sound)), None) => {
            let entity = commands
                .spawn((
                    Name::new(format!("{sound} loop")),
                    Transform::IDENTITY,
                    ChildOf(parent),
                ))
                .id();
            play.write(PlaySound::on(sound, entity).looping());
            *anchor = Some(entity);
        }
        (None, Some(entity)) => {
            if let Ok(mut entity) = commands.get_entity(entity) {
                entity.despawn();
            }
            *anchor = None;
        }
        _ => {}
    }
}

/// `pinging` while the player's sub is under water.
pub fn ping_sound(
    mut commands: Commands,
    table: Option<Res<VehicleTable>>,
    mut state: ResMut<CueState>,
    mut play: MessageWriter<PlaySound>,
    players: Query<(Entity, &Vehicle, &GroundState), With<PlayerVehicle>>,
) {
    let wanted = table.as_ref().and_then(|table| {
        let (entity, vehicle, ground) = players.single().ok()?;
        if vehicle.form != VehicleForm::Sub || !ground.is_divable() {
            return None;
        }
        let sound = table.kind(&vehicle.kind)?.sounds.pinging.as_deref()?;
        Some((entity, sound))
    });
    let mut anchor = state.ping;
    keep_loop(&mut commands, &mut play, &mut anchor, wanted);
    state.ping = anchor;
}

/// The low-shield voice as the shields fall past the threshold, and the
/// alarm while they stay below it.
pub fn shield_warnings(
    mut commands: Commands,
    table: Option<Res<VehicleTable>>,
    control: Option<Res<PlayerControl>>,
    mut state: ResMut<CueState>,
    mut play: MessageWriter<PlaySound>,
    players: Query<(Entity, &Health), With<PlayerVehicle>>,
    mut was_low: Local<bool>,
) {
    let Some(table) = table else {
        return;
    };
    let (warning, threshold, alarm) = &table.player.low_shield;
    let player = players.single().ok();
    let cinematic = control.is_some_and(|control| control.is_cinematic());
    let low = !cinematic
        && player.is_some_and(|(_, health)| {
            health.current > 0.0 && health.fraction() < low_shield_fraction(*threshold)
        });

    if low && !*was_low {
        play.write(PlaySound::ui(warning));
    }
    *was_low = low;

    let wanted = player
        .filter(|_| low)
        .map(|(entity, _)| (entity, alarm.as_str()));
    let mut anchor = state.alarm;
    keep_loop(&mut commands, &mut play, &mut anchor, wanted);
    state.alarm = anchor;
}

/// `LOCK_ON_WARNING` as each homing missile locks onto the player.
pub fn lock_on_warnings(
    weapons: Option<Res<WeaponTable>>,
    mut state: ResMut<CueState>,
    mut play: MessageWriter<PlaySound>,
    players: Query<Entity, With<PlayerVehicle>>,
    projectiles: Query<(Entity, &Projectile)>,
) {
    let Some(weapons) = weapons else {
        return;
    };
    let Ok(player) = players.single() else {
        return;
    };
    let mut alive = HashSet::new();
    for (entity, projectile) in &projectiles {
        if projectile.target != Some(player)
            || weapons
                .by_index(projectile.weapon)
                .is_none_or(|def| def.mover != Mover::Homing)
        {
            continue;
        }
        alive.insert(entity);
        if state.locked.insert(entity) && !weapons.warnings.lock_on.is_empty() {
            play.write(PlaySound::on(&weapons.warnings.lock_on, player));
        }
    }
    state.locked.retain(|entity| alive.contains(entity));
}

/// The warning `weapons.zrd` names for a refusal.
pub fn refusal_sound(warnings: &bft_sim::weapons::WeaponWarnings, reason: Refusal) -> &str {
    match reason {
        Refusal::NoAmmo => &warnings.no_ammo,
        Refusal::TriggerInactive => &warnings.trigger_inactive,
        Refusal::WeaponInactive => &warnings.weapon_inactive,
    }
}

/// The out-of-ammo and inactive-trigger warnings, for the player's own
/// refused pulls and selections.
pub fn refusal_sounds(
    time: Res<Time>,
    weapons: Option<Res<WeaponTable>>,
    mut state: ResMut<CueState>,
    mut play: MessageWriter<PlaySound>,
    mut refused: MessageReader<TriggerRefused>,
    players: Query<(), With<PlayerVehicle>>,
) {
    let dt = time.delta_secs();
    for wait in state.refused.values_mut() {
        *wait = (*wait - dt).max(0.0);
    }
    let Some(weapons) = weapons else {
        refused.clear();
        return;
    };
    for event in refused.read() {
        if !players.contains(event.shooter) {
            continue;
        }
        let wait = state.refused.entry(event.reason).or_insert(0.0);
        if *wait > 0.0 {
            continue;
        }
        *wait = REFUSAL_GAP;
        let sound = refusal_sound(&weapons.warnings, event.reason);
        if !sound.is_empty() {
            play.write(PlaySound::ui(sound));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hop over a bump is not a landing; coming down from a jump is.
    #[test]
    fn only_a_real_jump_lands() {
        assert!(is_landing(false, true, 0.5));
        assert!(!is_landing(false, true, 0.1));
        assert!(!is_landing(true, true, 0.5));
        assert!(!is_landing(false, false, 0.5));
    }

    #[test]
    fn each_refusal_says_its_own_warning() {
        let warnings = bft_sim::weapons::WeaponWarnings {
            lock_on: "snd_lo_warn".into(),
            no_ammo: "snd_ammo_out".into(),
            trigger_inactive: "snd_trigger_click".into(),
            weapon_inactive: "snd_weapon_click".into(),
        };
        assert_eq!(refusal_sound(&warnings, Refusal::NoAmmo), "snd_ammo_out");
        assert_eq!(
            refusal_sound(&warnings, Refusal::TriggerInactive),
            "snd_trigger_click"
        );
        assert_eq!(
            refusal_sound(&warnings, Refusal::WeaponInactive),
            "snd_weapon_click"
        );
    }

    #[test]
    fn the_low_shield_threshold_is_a_percentage() {
        assert!((low_shield_fraction(10.0) - 0.1).abs() < 1e-6);
    }
}
