//! Pickup kinds, their placement, and what picking one up does.
//!
//! # Two files, and a third that is neither
//!
//! `pickup.zrd`'s `PICKUP_DATA` names the **kinds** — a sound and a HUD icon
//! each (READER-SCHEMA §2.4). The per-mission `puppies{,_easy,_hard}.zrd`
//! places them: `[kind, amount, pos, rot, respawn_s]` (§3.5). Neither states
//! what a kind *does*; that is derived from the name
//! ([`PickupEffect::derive`]), which is safe because the 40 names are a
//! closed set with three shapes: `<WEAPON>_AMMO`, `<WEAPON>_WEAPON`, and six
//! specials.
//!
//! # The models are named by an interp script, not by either file
//!
//! Pickup models are the 40 template subtrees `pu000`…`pu039` that every
//! level carries. Which kind is which template is fixed by
//! `support\pickup.gw`'s load order, and **that order is not `PICKUP_DATA`'s
//! order**: the guns block runs `lomiss, gmiss, nuke, gnuke` while
//! `PICKUP_DATA` lists missile, nuke, guided missile, guided nuke. So
//! [`TEMPLATE_ORDER`] is the script's list, transcribed, rather than derived
//! from the reader — deriving it puts the nuke's model on the guided missile.
//!
//! # What is here and what is the anim VM's
//!
//! This module places pickups and applies their effect. The **spin and bob**
//! of a pickup in the world is an anim def (`pu000`…`pu039` in each level's
//! `anim.zbd`), and respawn visibility is the same def resetting; the anim VM
//! drives those. This module owns the trigger volume, the effect and the
//! respawn timer.

use avian3d::prelude::*;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::ReaderValue;

use crate::damage::{Destroyed, Drops, Health, Team};
use crate::schedule::SimSet;
use crate::vehicle::tuning::Difficulty;
use crate::vehicle::{Vehicle, VehicleForm};
use crate::weapons::{AmmoState, WeaponLoadout, WeaponTable};

/// The `pickup.zrd` entry of the global reader archive.
pub const PICKUP_ENTRY: &str = "pickup.zrd";

/// The key the 40 kinds live under.
pub const PICKUP_DATA: &str = "PICKUP_DATA";

/// Which `puppies*.zrd` entry a difficulty reads.
///
/// Difficulty picks a whole file, as it does for `vehicle.zrd`
/// (READER-SCHEMA §1). m1's three are identical; m2's differ (53/56/56).
///
/// **Only the six campaign missions have the variants.** m7–m13 — the
/// multiplayer maps — ship `puppies.zrd` alone, so a caller has to fall back;
/// [`pickup_placements`] does. Difficulty has no meaning in deathmatch.
pub fn puppies_entry(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Easy => "puppies_easy.zrd",
        Difficulty::Normal => "puppies.zrd",
        Difficulty::Hard => "puppies_hard.zrd",
    }
}

/// Kind to template subtree, in `support\pickup.gw`'s load order.
///
/// Transcribed from the interp script rather than derived from
/// `PICKUP_DATA`; see the module docs for the two entries where the orders
/// disagree. The index is the `NNN` of `puNNN`.
pub const TEMPLATE_ORDER: [&str; 40] = [
    // Ammunition, `pu000`–`pu016`.
    "ERFPG_AMMO",
    "HEMORTAR_AMMO",
    "QMORTAR_AMMO",
    "FREON_AMMO",
    "NAPALM_AMMO",
    "P_HEMINE_AMMO",
    "P_QMINE_AMMO",
    "R_HEMINE_AMMO",
    "R_QMINE_AMMO",
    "LOCKON_LASER_AMMO",
    "LASER_SABRE_AMMO",
    "SONIC_CANNON_AMMO",
    "ARC_SABRE_AMMO",
    "MISSILE_AMMO",
    "GUIDED_MISSILE_AMMO",
    "NUKE_AMMO",
    "GUIDED_NUKE_AMMO",
    // The guns, `pu017`–`pu033`.
    "ERFPG_WEAPON",
    "HEMORTAR_WEAPON",
    "QMORTAR_WEAPON",
    "FREON_WEAPON",
    "NAPALM_WEAPON",
    "P_HEMINE_WEAPON",
    "P_QMINE_WEAPON",
    "R_HEMINE_WEAPON",
    "R_QMINE_WEAPON",
    "LOCKON_LASER_WEAPON",
    "LASER_SABRE_WEAPON",
    "SONIC_CANNON_WEAPON",
    "ARC_SABRE_WEAPON",
    "MISSILE_WEAPON",
    "GUIDED_MISSILE_WEAPON",
    "NUKE_WEAPON",
    "GUIDED_NUKE_WEAPON",
    // The specials, `pu034`–`pu039`.
    "NANITE",
    "NANITE100",
    "NANO-CANISTER",
    "PUP_AMPHIB",
    "PUP_HOVER",
    "PUP_SUB",
];

/// The template node name for a kind, or `None` for a kind not in the list.
pub fn template_for(kind: &str) -> Option<String> {
    let index = TEMPLATE_ORDER.iter().position(|name| *name == kind)?;

    Some(format!("pu{index:03}"))
}

/// What [`build_pickup_table`] or [`pickup_placements`] could not do.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PickupError {
    /// No `pickup.zrd`, so this is not the global archive.
    #[error("the reader archive has no `{PICKUP_ENTRY}` entry")]
    NoPickupEntry,
    /// No `PICKUP_DATA`, so it is a different file than this understands.
    #[error("`{PICKUP_ENTRY}` has no `{PICKUP_DATA}` block")]
    NoPickupData,
    /// The mission archive has no `puppies*.zrd` for this difficulty. Every
    /// shipped mission has all three.
    #[error("the mission archive has no `{0}` entry")]
    NoPuppiesEntry(&'static str),
}

/// Every pickup kind, resolved from `pickup.zrd`.
#[derive(Resource, Debug, Clone, Default)]
pub struct PickupTable {
    /// Keyed by the `PICKUP_DATA` name (`"ERFPG_AMMO"`).
    pub kinds: HashMap<String, PickupDef>,
}

impl PickupTable {
    /// One kind's definition.
    pub fn get(&self, kind: &str) -> Option<&PickupDef> {
        self.kinds.get(kind)
    }
}

/// One `PICKUP_DATA` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct PickupDef {
    /// The reader key.
    pub kind: String,
    /// `SOUND`: played on pickup.
    pub sound: Option<String>,
    /// `IMAGE`: the HUD icon in `image.zbd` (`gun_01`…`gun_13`). Only the
    /// 17 `_WEAPON` kinds have one.
    pub image: Option<String>,
    /// What picking it up does, derived from the name.
    pub effect: PickupEffect,
    /// The `puNNN` template subtree, from [`TEMPLATE_ORDER`].
    pub template: Option<String>,
}

/// What a pickup gives you.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PickupEffect {
    /// Rounds for a weapon already owned; the amount is the placement's.
    Ammo { weapon_slot: u8, strong: bool },
    /// The weapon itself, plus a starting magazine.
    Weapon { weapon_slot: u8, strong: bool },
    /// Health. The amount is the placement's, so the same kind gives 15 in
    /// one spot and 100 in another.
    Health,
    /// A form chip: amphib, hover or sub.
    FormUnlock(VehicleForm),
}

impl PickupEffect {
    /// Which effect a kind's name implies.
    ///
    /// The 40 names are a closed set and neither reader file says what a kind
    /// does, so the name is the only source. `None` for a name outside the
    /// set, which is a data change worth hearing about rather than a guess.
    pub fn derive(kind: &str) -> Option<PickupEffect> {
        match kind {
            "PUP_AMPHIB" => return Some(PickupEffect::FormUnlock(VehicleForm::Amphib)),
            "PUP_HOVER" => return Some(PickupEffect::FormUnlock(VehicleForm::Hover)),
            "PUP_SUB" => return Some(PickupEffect::FormUnlock(VehicleForm::Sub)),
            "NANITE" | "NANITE100" | "NANO-CANISTER" => return Some(PickupEffect::Health),
            _ => {}
        }

        let (stem, is_weapon) = match kind.strip_suffix("_WEAPON") {
            Some(stem) => (stem, true),
            None => (kind.strip_suffix("_AMMO")?, false),
        };

        let (weapon_slot, strong) = WEAPON_KINDS
            .iter()
            .find(|(name, _, _)| *name == stem)
            .map(|(_, slot, strong)| (*slot, *strong))?;

        Some(if is_weapon {
            PickupEffect::Weapon {
                weapon_slot,
                strong,
            }
        } else {
            PickupEffect::Ammo {
                weapon_slot,
                strong,
            }
        })
    }
}

/// Pickup name stem to the HUD slot and variant it fills.
///
/// The slot numbers are `weapons.zrd`'s, cross-checked against the manual's
/// 9 × 2 layout and the `IMAGE` icons that weapons sharing a slot share
/// (`gun_02` for both mortars, `gun_03` for all four mines).
const WEAPON_KINDS: [(&str, u8, bool); 17] = [
    ("ERFPG", 1, true),
    ("HEMORTAR", 2, false),
    ("QMORTAR", 2, true),
    ("FREON", 3, false),
    ("NAPALM", 3, true),
    ("P_HEMINE", 4, false),
    ("P_QMINE", 4, true),
    ("R_HEMINE", 5, false),
    ("R_QMINE", 5, true),
    ("LOCKON_LASER", 6, false),
    ("LASER_SABRE", 6, true),
    ("SONIC_CANNON", 7, false),
    ("ARC_SABRE", 7, true),
    ("MISSILE", 8, false),
    ("GUIDED_MISSILE", 8, true),
    ("NUKE", 9, false),
    ("GUIDED_NUKE", 9, true),
];

/// One `puppies.zrd` record.
#[derive(Debug, Clone, PartialEq)]
pub struct PickupPlacement {
    /// A `PICKUP_DATA` key.
    pub kind: String,
    /// Rounds, or health points for the nanites.
    pub amount: u32,
    /// World position, metres.
    pub pos: Vec3,
    /// Seconds until it comes back, or `None` in campaign (the record's
    /// fifth field is 0 there).
    pub respawn: Option<f32>,
}

/// A pickup sitting in the world.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Pickup {
    pub kind: String,
    pub amount: u32,
    pub respawn: Option<f32>,
    /// The instance node name, `pu<NNN><ii>` — what `objectives.zrd`
    /// references (`["pu03700"]`).
    pub instance: String,
}

/// A pickup that has been taken and is waiting to come back.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct RespawnTimer {
    /// Seconds left.
    pub remaining: f32,
}

/// Somebody picked something up.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct PickedUp {
    pub by: Entity,
    pub kind: String,
    pub amount: u32,
    pub instance: String,
}

/// The instance node name for the `index`th placement of a kind.
///
/// `pu<NNN><ii>`: the template number then a two-digit instance counter, so
/// the 0th `PUP_AMPHIB` is `pu03700`. `None` for a kind with no template.
pub fn instance_name(kind: &str, index: usize) -> Option<String> {
    Some(format!("{}{index:02}", template_for(kind)?))
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Build the kind table from the global reader archive.
pub fn build_pickup_table(reader: &bft_assets::ReaderData) -> Result<PickupTable, PickupError> {
    let entry = reader
        .read(PICKUP_ENTRY)
        .ok_or(PickupError::NoPickupEntry)?;
    let data = entry.get(PICKUP_DATA).ok_or(PickupError::NoPickupData)?;

    let mut kinds = HashMap::default();

    for (name, value) in data.pairs() {
        let Some(effect) = PickupEffect::derive(name) else {
            warn!("{PICKUP_ENTRY}: pickup kind `{name}` is not one this knows; skipping it");
            continue;
        };

        kinds.insert(
            name.to_string(),
            PickupDef {
                kind: name.to_string(),
                sound: value
                    .get("SOUND")
                    .and_then(|v| v.string())
                    .map(str::to_string),
                image: value
                    .get("IMAGE")
                    .and_then(|v| v.string())
                    .map(str::to_string),
                effect,
                template: template_for(name),
            },
        );
    }

    Ok(PickupTable { kinds })
}

/// Read a mission's pickup placements.
///
/// `reader` is the **per-mission** archive, not the global one.
///
/// Falls back to `puppies.zrd` when the difficulty's own variant is absent,
/// which is every multiplayer map (m7–m13 ship one file). The fallback is
/// **not** silent-by-design generosity: it is the shape of the data, and a
/// mission with neither file is still an error.
pub fn pickup_placements(
    reader: &bft_assets::ReaderData,
    difficulty: Difficulty,
) -> Result<Vec<PickupPlacement>, PickupError> {
    let entry = puppies_entry(difficulty);
    let records = reader
        .read(entry)
        .or_else(|| reader.read(puppies_entry(Difficulty::Normal)))
        .ok_or(PickupError::NoPuppiesEntry(entry))?;

    Ok(records
        .items()
        .iter()
        .filter_map(|record| parse_placement(*record))
        .collect())
}

/// One `[kind, amount, [x,y,z], [rx,ry,rz], respawn_s]` record.
///
/// The rotation is zero in every shipped record and is not read.
fn parse_placement(record: ReaderValue<'_>) -> Option<PickupPlacement> {
    let kind = record.nth(0)?.string()?.to_string();
    let amount = record.nth(1)?.int()?.max(0) as u32;
    let [x, y, z] = record.nth(2)?.float_array::<3>()?;
    let respawn = record.nth(4)?.float()?;

    Some(PickupPlacement {
        kind,
        amount,
        pos: Vec3::new(x, y, z),
        // 0 is "never comes back", which is every campaign record.
        respawn: (respawn > 0.0).then_some(respawn),
    })
}

/// Give every placement its `pu<NNN><ii>` instance name.
///
/// The counter is per kind and in file order, which is what makes
/// `objectives.zrd`'s `["pu03700"]` resolvable: it is the first amphib chip
/// in the file.
pub fn name_placements(placements: &[PickupPlacement]) -> Vec<(PickupPlacement, String)> {
    let mut counters: HashMap<&str, usize> = HashMap::default();

    placements
        .iter()
        .map(|placement| {
            let counter = counters.entry(placement.kind.as_str()).or_default();
            let index = *counter;
            *counter += 1;

            let name = instance_name(&placement.kind, index)
                .unwrap_or_else(|| format!("{}{index:02}", placement.kind));

            (placement.clone(), name)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// How close a vehicle has to get to take a pickup, in metres.
///
/// // approximation: each `puNNN` template has a `bvol` child — its pickup
/// trigger volume — whose extent this stands in for until the real
/// subtrees' colliders are used. The vehicle is 5.7 x 7.4 m, so a
/// radius near its half-width is what "drove over it" means.
pub const PICKUP_RADIUS: f32 = 3.0;

/// Spawn one pickup entity per placement.
///
/// The model is `bft-game`'s (`pickup_view`): this spawns the trigger and the state, names it
/// `pu<NNN><ii>` so objectives can reference it, and leaves the `puNNN`
/// subtree to be attached later.
pub fn spawn_pickups(commands: &mut Commands, placements: &[PickupPlacement]) -> Vec<Entity> {
    name_placements(placements)
        .into_iter()
        .map(|(placement, instance)| {
            commands
                .spawn(pickup_bundle(
                    Pickup {
                        kind: placement.kind.clone(),
                        amount: placement.amount,
                        respawn: placement.respawn,
                        instance,
                    },
                    placement.pos,
                ))
                .id()
        })
        .collect()
}

/// Everything a pickup entity carries: the state and its trigger.
///
/// Shared by [`spawn_pickups`] and a save's restore, so a restored pickup is
/// the same kind of entity as a placed one.
pub fn pickup_bundle(pickup: Pickup, at: Vec3) -> impl Bundle {
    (
        Name::new(pickup.instance.clone()),
        pickup,
        Transform::from_translation(at),
        Collider::sphere(PICKUP_RADIUS),
        Sensor,
        crate::physics::layers::pickup(),
    )
}

/// Take any pickup a player-side vehicle is standing on.
///
/// Distance rather than avian's contact events: 40-odd pickups against one
/// player is a handful of comparisons a tick, it is deterministic without
/// depending on solver ordering (NFR-3 asks for a replayable sim), and a
/// sensor pair that never generates a contact because both bodies are asleep
/// cannot silently swallow a pickup.
#[allow(clippy::type_complexity)]
pub fn collect_pickups(
    mut commands: Commands,
    mut picked: MessageWriter<PickedUp>,
    table: Option<Res<PickupTable>>,
    weapons: Option<Res<WeaponTable>>,
    pickups: Query<(Entity, &Pickup, &GlobalTransform, Option<&Children>), Without<RespawnTimer>>,
    mut vehicles: Query<
        (
            Entity,
            &GlobalTransform,
            &Team,
            Option<&mut WeaponLoadout>,
            Option<&mut Health>,
            Option<&mut Vehicle>,
        ),
        With<crate::vehicle::Vehicle>,
    >,
) {
    let Some(table) = table else {
        return;
    };

    for (entity, pickup, at, models) in &pickups {
        let Some(def) = table.get(&pickup.kind) else {
            continue;
        };

        for (vehicle_entity, vehicle_at, team, loadout, health, vehicle) in &mut vehicles {
            // Only the player's side collects; enemies drive over their own
            // drops without picking them up.
            if *team != Team::PLAYER {
                continue;
            }
            if vehicle_at.translation().distance(at.translation()) > PICKUP_RADIUS {
                continue;
            }

            let applied = apply_pickup(
                def,
                pickup.amount,
                weapons.as_deref(),
                loadout,
                health,
                vehicle,
            );
            if !applied {
                // Full health, or a weapon slot the data has no weapon for:
                // leave it standing so it can be taken later.
                continue;
            }

            picked.write(PickedUp {
                by: vehicle_entity,
                kind: pickup.kind.clone(),
                amount: pickup.amount,
                instance: pickup.instance.clone(),
            });

            // The model is `bft-game`'s clone under the pickup. It goes on
            // to play its way into the vehicle, so it moves off
            // the pickup here — in the same command buffer as, and ahead
            // of, the despawn below, which would otherwise take it along.
            if let Some(template) = template_for(&pickup.kind) {
                let taken = commands
                    .spawn((
                        Name::new(format!("taken {}", pickup.instance)),
                        TakenPickup {
                            remaining: TAKEN_PICKUP_TIME,
                            into: vehicle_entity,
                            template,
                        },
                        Transform::from_translation(at.translation()),
                    ))
                    .id();
                for model in models.into_iter().flatten() {
                    commands.entity(*model).insert(ChildOf(taken));
                }
            }

            match pickup.respawn {
                // Deathmatch: start the clock. `RespawnTimer` is itself the
                // "taken" marker — `collect_pickups` filters on
                // `Without<RespawnTimer>` — because `Visibility` is a render
                // component and this crate builds without a renderer (NFR-4).
                // `bft-game` hides the model by watching for the component.
                Some(seconds) => {
                    commands
                        .entity(entity)
                        .insert(RespawnTimer { remaining: seconds });
                }
                // Campaign: it is gone for good.
                None => {
                    commands.entity(entity).despawn();
                }
            }
            break;
        }
    }
}

/// Add rounds to a weapon's state, mounting it when `mount` is set.
///
/// A state is created, unmounted, for a weapon the loadout has no row for.
/// `false` when nothing changed: the magazine is full and the gun was
/// already mounted, or the table has no such weapon.
fn add_rounds(
    weapons: Option<&WeaponTable>,
    loadout: Option<Mut<'_, WeaponLoadout>>,
    weapon_slot: u8,
    strong: bool,
    amount: u32,
    mount: bool,
) -> bool {
    let (Some(weapons), Some(mut loadout)) = (weapons, loadout) else {
        return false;
    };
    let Some(def) = weapons.slot(weapon_slot, strong) else {
        return false;
    };
    let Some(index) = weapons.index(&def.id) else {
        return false;
    };
    let limit = def.ammo_limit;
    let interval = def.shot_interval();
    let Some(slot) = loadout.slot_mut(weapon_slot) else {
        return false;
    };

    let variant = if strong {
        &mut slot.strong
    } else {
        &mut slot.weak
    };
    let state = variant.get_or_insert(AmmoState {
        weapon: index,
        ammo: 0,
        interval,
        mounted: false,
    });

    let mounts = mount && !state.mounted;
    if state.ammo >= limit && !mounts {
        return false;
    }
    state.add(amount, limit);
    state.mounted |= mount;
    true
}

/// Apply one pickup's effect, and say whether it did anything.
///
/// `false` means "nothing to give" — full health, or a weapon slot with no
/// weapon in the table — which leaves the pickup standing.
fn apply_pickup(
    def: &PickupDef,
    amount: u32,
    weapons: Option<&WeaponTable>,
    loadout: Option<Mut<'_, WeaponLoadout>>,
    health: Option<Mut<'_, Health>>,
    vehicle: Option<Mut<'_, Vehicle>>,
) -> bool {
    match def.effect {
        PickupEffect::Health => {
            let Some(mut health) = health else {
                return false;
            };
            if health.current >= health.max {
                return false;
            }
            health.heal(amount as f32);
            true
        }
        PickupEffect::FormUnlock(form) => {
            let Some(mut vehicle) = vehicle else {
                return false;
            };
            if vehicle.unlocked.has(form) {
                return false;
            }
            vehicle.unlocked.unlock(form);
            true
        }
        PickupEffect::Ammo {
            weapon_slot,
            strong,
        } => {
            // Rounds are taken whether or not the gun is mounted: they wait
            // in the loadout until the weapon's own pickup mounts it.
            add_rounds(weapons, loadout, weapon_slot, strong, amount, false)
        }
        PickupEffect::Weapon {
            weapon_slot,
            strong,
        } => {
            // The gun, and a starting magazine. Already mounted, it only
            // tops the magazine up.
            add_rounds(weapons, loadout, weapon_slot, strong, amount, true)
        }
    }
}

/// How far above a drop point [`ground_below`] starts looking: enough to
/// start above a hull the shot hit the underside of.
const DROP_PROBE_ABOVE: f32 = 4.0;

/// How far down [`ground_below`] looks, past any plausible hover height.
const DROP_PROBE_DEPTH: f32 = 200.0;

/// The level surface under `at`, if there is one within reach.
fn ground_below(spatial: &SpatialQuery, at: Vec3) -> Option<Vec3> {
    let from = at + Vec3::Y * DROP_PROBE_ABOVE;
    let filter = SpatialQueryFilter::from_mask([crate::level::GameLayer::Level]);

    spatial
        .cast_ray(from, Dir3::NEG_Y, DROP_PROBE_DEPTH, true, &filter)
        .map(|hit| from + Vec3::NEG_Y * hit.distance)
}

/// Bring hidden pickups back when their timer runs out.
pub fn respawn_pickups(
    time: Res<Time<Fixed>>,
    mut commands: Commands,
    mut waiting: Query<(Entity, &mut RespawnTimer)>,
) {
    let delta = time.delta_secs();

    for (entity, mut timer) in &mut waiting {
        timer.remaining -= delta;
        if timer.remaining <= 0.0 {
            commands.entity(entity).remove::<RespawnTimer>();
        }
    }
}

/// Spawn a destroyed entity's drops on the ground where it died.
///
/// `vehicle.zrd`'s `pickups` and `nanite` keys, which the vehicle table parses into
/// [`Drops`]. They are campaign drops, so they never respawn.
///
/// `Destroyed::at` is where the killing blow landed, which is often up on
/// the hull or in the air over a hover unit, and a pickup has no body to
/// fall with. So the drop goes to the level surface under that point, where
/// the player can drive over it.
pub fn spawn_drops(
    mut commands: Commands,
    mut destroyed: MessageReader<Destroyed>,
    drops: Query<&Drops>,
    spatial: SpatialQuery,
) {
    for event in destroyed.read() {
        let Ok(Drops(defs)) = drops.get(event.entity) else {
            continue;
        };
        let pos = ground_below(&spatial, event.at).unwrap_or(event.at);

        let placements: Vec<PickupPlacement> = defs
            .iter()
            .map(|drop| PickupPlacement {
                kind: drop.kind.clone(),
                amount: drop.amount,
                pos,
                respawn: None,
            })
            .collect();

        spawn_pickups(&mut commands, &placements);
    }
}

/// A taken pickup's model on its way into the vehicle that took it.
///
/// The pickup itself is gone the tick it is taken (or waiting to respawn),
/// as the rules want; [`collect_pickups`] moves its model onto this entity,
/// which plays the template's `pickup` sequence: up two metres,
/// then stretched toward the vehicle and drawn into it. Despawned, model and
/// all, when the sequence has run.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct TakenPickup {
    /// Seconds until it is removed.
    pub remaining: f32,
    /// The vehicle it is going into.
    pub into: Entity,
    /// The `puNNN` template, which is also the definition's `anim_name`.
    pub template: String,
}

/// The node of a vehicle a taken pickup is drawn into: `bft_00`'s `target`,
/// a transform-only node over the hull's middle.
pub const PICKUP_TARGET_NODE: &str = "target";

/// Where a vehicle's middle is when it has no [`PICKUP_TARGET_NODE`], in
/// hull space.
///
/// // approximation: the middle of the hull box (`HULL_FLOOR_Y` to
/// `HULL_TOP_Y`).
pub const PICKUP_TARGET_FALLBACK: Vec3 = Vec3::new(0.0, 1.9, 0.0);

/// How long a taken pickup lingers when its level has no definition to time
/// it by, in seconds. The shipped `pickup` sequence runs 0.525 s.
pub const TAKEN_PICKUP_TIME: f32 = 0.525;

/// Play each newly taken pickup's `pickup` sequence on its model.
///
/// Every `puNNN` definition in the game is the same (`pickup.zrd`'s
/// `ANIMATION_DEFINITIONS`, compiled once per template): scale to 1, rise at
/// 16 m/s for an eighth of a second, then two `ObjectConnector`s between the
/// **input nodes** — the pickup and the vehicle — that first stretch the
/// model from where it floated to the vehicle and then draw its tail in
/// after it. The connectors' first end is where the rise leaves the model,
/// fixed at the moment of taking, so the stretch starts from the top of the
/// rise rather than chasing the model as it moves.
pub fn animate_taken_pickups(
    mut calls: MessageWriter<crate::anim::CallAnim>,
    program: Option<Res<crate::anim::AnimProgram>>,
    mut taken: Query<(Entity, &mut TakenPickup, &Transform), Added<TakenPickup>>,
    children: Query<&Children>,
    names: Query<&crate::level::NodeName>,
) {
    for (entity, mut pickup, at) in &mut taken {
        let def = program
            .as_ref()
            .and_then(|program| program.0.find(&pickup.template));
        let rise = def.map(rise_of).unwrap_or_default();
        if let Some(time) = def
            .map(|def| def.sequences.iter().map(sequence_time).fold(0.0, f32::max))
            .filter(|time| *time > 0.0)
        {
            pickup.remaining = time;
        }

        let target = find_named(pickup.into, PICKUP_TARGET_NODE, &children, &names);
        let into = crate::anim::ConnectorEnd {
            node: Some(target.unwrap_or(pickup.into)),
            offset: if target.is_some() {
                Vec3::ZERO
            } else {
                PICKUP_TARGET_FALLBACK
            },
            frozen: false,
        };
        let from = crate::anim::ConnectorEnd {
            node: None,
            offset: at.translation + rise,
            frozen: false,
        };
        let mut call = crate::anim::CallAnim::on(pickup.template.clone(), entity);
        call.input_ends = Some((from, into));
        calls.write(call);
    }
}

/// Count down and remove the taken pickups.
pub fn retire_taken_pickups(
    time: Res<Time<Fixed>>,
    mut commands: Commands,
    mut taken: Query<(Entity, &mut TakenPickup)>,
) {
    for (entity, mut pickup) in &mut taken {
        pickup.remaining -= time.delta_secs();
        if pickup.remaining <= 0.0 {
            commands.entity(entity).despawn();
        }
    }
}

/// How far a definition's `ObjectMotion`s carry its root, summed.
///
/// `translation` is a velocity and its change, so each motion covers
/// `initial·t + ½·delta·t²` over its `run_time`. The random spread is left
/// out: it is the part no caller can know in advance (±1 m/s on a pickup's
/// 16).
pub fn rise_of(def: &bft_assets::anim::AnimDefDesc) -> Vec3 {
    let mut total = Vec3::ZERO;
    for event in def.sequences.iter().flat_map(|seq| &seq.events) {
        let bft_assets::anim::EventDataDesc::Presentation {
            kind: "ObjectMotion",
            value,
        } = &event.data
        else {
            continue;
        };
        if value.get("node").and_then(|node| node.as_str()) != Some(def.name.as_str()) {
            continue;
        }
        let Some(translation) = value.get("translation").filter(|t| !t.is_null()) else {
            continue;
        };
        let time = value
            .get("run_time")
            .and_then(|time| time.as_f64())
            .unwrap_or(0.0) as f32;
        let vector = |key: &str| {
            translation
                .get(key)
                .map(|v| {
                    let axis = |a: &str| v.get(a).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                    Vec3::new(axis("x"), axis("y"), axis("z"))
                })
                .unwrap_or_default()
        };
        total += vector("initial") * time + 0.5 * vector("delta") * time * time;
    }
    total
}

/// How long a sequence of back-to-back timed events runs, in seconds.
///
/// Every event of the `pickup` sequence has `start: null`, and a timed event
/// blocks the next (docs/data/anims.md), so the sequence is their run times
/// summed.
fn sequence_time(seq: &bft_assets::anim::SeqDesc) -> f32 {
    seq.events
        .iter()
        .filter_map(|event| match &event.data {
            bft_assets::anim::EventDataDesc::Presentation { value, .. } => value
                .get("run_time")
                .and_then(|time| time.as_f64())
                .map(|time| time as f32),
            _ => None,
        })
        .sum()
}

/// The first node of this name under `root`, breadth-first.
fn find_named(
    root: Entity,
    name: &str,
    children: &Query<&Children>,
    names: &Query<&crate::level::NodeName>,
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

/// The pickups plugin: collection, respawn and enemy drops.
pub struct PickupsPlugin;

impl Plugin for PickupsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<PickedUp>().add_systems(
            FixedUpdate,
            (
                (collect_pickups, ApplyDeferred, animate_taken_pickups).chain(),
                respawn_pickups,
                retire_taken_pickups,
                spawn_drops,
            )
                .in_set(SimSet::Damage),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("valid test JSON")
    }

    /// The list is the whole template set: 40 kinds, 40 subtrees, no
    /// duplicates. A repeated name would silently give two kinds one model.
    #[test]
    fn the_template_order_is_forty_distinct_kinds() {
        let unique: std::collections::BTreeSet<&str> = TEMPLATE_ORDER.iter().copied().collect();

        assert_eq!(TEMPLATE_ORDER.len(), 40);
        assert_eq!(unique.len(), 40, "a repeated kind would share a model");
    }

    /// The two entries where `pickup.gw`'s order and `PICKUP_DATA`'s
    /// disagree. Getting these wrong puts the nuke's model on the guided
    /// missile.
    #[test]
    fn the_missile_and_nuke_templates_follow_the_interp_script() {
        assert_eq!(template_for("MISSILE_AMMO").as_deref(), Some("pu013"));
        assert_eq!(
            template_for("GUIDED_MISSILE_AMMO").as_deref(),
            Some("pu014")
        );
        assert_eq!(template_for("NUKE_AMMO").as_deref(), Some("pu015"));
        assert_eq!(template_for("GUIDED_NUKE_AMMO").as_deref(), Some("pu016"));

        assert_eq!(template_for("MISSILE_WEAPON").as_deref(), Some("pu030"));
        assert_eq!(
            template_for("GUIDED_MISSILE_WEAPON").as_deref(),
            Some("pu031")
        );
        assert_eq!(template_for("NUKE_WEAPON").as_deref(), Some("pu032"));
        assert_eq!(template_for("GUIDED_NUKE_WEAPON").as_deref(), Some("pu033"));
    }

    /// `objectives.zrd` references `pu03700`, which has to resolve to the
    /// first amphib chip.
    #[test]
    fn the_amphib_chip_instance_is_the_name_objectives_reference() {
        assert_eq!(template_for("PUP_AMPHIB").as_deref(), Some("pu037"));
        assert_eq!(instance_name("PUP_AMPHIB", 0).as_deref(), Some("pu03700"));
        assert_eq!(instance_name("PUP_SUB", 11).as_deref(), Some("pu03911"));
    }

    #[test]
    fn every_kind_in_the_template_order_has_an_effect() {
        for kind in TEMPLATE_ORDER {
            assert!(
                PickupEffect::derive(kind).is_some(),
                "no effect derived for `{kind}`"
            );
        }
    }

    #[test]
    fn a_name_outside_the_set_derives_nothing_rather_than_guessing() {
        assert_eq!(PickupEffect::derive("SOMETHING_ELSE"), None);
        assert_eq!(PickupEffect::derive("MADEUP_AMMO"), None);
    }

    /// Weak and strong of the same slot are different kinds with different
    /// models, which is what keeps their ammo separate.
    #[test]
    fn the_two_variants_of_a_slot_are_different_kinds() {
        assert_eq!(
            PickupEffect::derive("HEMORTAR_AMMO"),
            Some(PickupEffect::Ammo {
                weapon_slot: 2,
                strong: false
            })
        );
        assert_eq!(
            PickupEffect::derive("QMORTAR_AMMO"),
            Some(PickupEffect::Ammo {
                weapon_slot: 2,
                strong: true
            })
        );
        assert_eq!(
            PickupEffect::derive("QMORTAR_WEAPON"),
            Some(PickupEffect::Weapon {
                weapon_slot: 2,
                strong: true
            })
        );
    }

    #[test]
    fn the_chips_unlock_forms_and_the_nanites_heal() {
        assert_eq!(
            PickupEffect::derive("PUP_HOVER"),
            Some(PickupEffect::FormUnlock(VehicleForm::Hover))
        );
        assert_eq!(
            PickupEffect::derive("NANO-CANISTER"),
            Some(PickupEffect::Health)
        );
        assert_eq!(
            PickupEffect::derive("NANITE100"),
            Some(PickupEffect::Health)
        );
    }

    #[test]
    fn the_kind_table_carries_the_sound_the_icon_and_the_template() {
        let json = parse(
            r#"[["PICKUP_DATA", [
                 "ERFPG_AMMO", ["SOUND", ["snd_pkup_erfpga"]],
                 "ERFPG_WEAPON", ["SOUND", ["snd_pkup_erfpgw"], "IMAGE", ["gun_01"]],
                 "PUP_SUB", ["SOUND", ["snd_pkup_sub"]]]]]"#,
        );
        let archive = bft_assets::ReaderData::from_entries(
            [(PICKUP_ENTRY.to_string(), json)].into_iter().collect(),
        );

        let table = build_pickup_table(&archive).expect("it builds");

        assert_eq!(table.kinds.len(), 3);
        let ammo = table.get("ERFPG_AMMO").expect("present");
        assert_eq!(ammo.sound.as_deref(), Some("snd_pkup_erfpga"));
        assert_eq!(ammo.image, None, "only the guns carry a HUD icon");
        assert_eq!(ammo.template.as_deref(), Some("pu000"));

        let gun = table.get("ERFPG_WEAPON").expect("present");
        assert_eq!(gun.image.as_deref(), Some("gun_01"));
        assert_eq!(gun.template.as_deref(), Some("pu017"));
        assert_eq!(
            gun.effect,
            PickupEffect::Weapon {
                weapon_slot: 1,
                strong: true
            },
            "the ERFPG is slot 1's strong variant"
        );

        assert_eq!(
            table.get("PUP_SUB").expect("present").template.as_deref(),
            Some("pu039")
        );
    }

    #[test]
    fn a_missing_pickup_entry_is_an_error() {
        assert_eq!(
            build_pickup_table(&bft_assets::ReaderData::default()).unwrap_err(),
            PickupError::NoPickupEntry
        );
    }

    /// m1's real first two records, plus a multiplayer-style respawn.
    #[test]
    fn placements_read_the_kind_amount_position_and_respawn() {
        let json = parse(
            r#"[[["HEMORTAR_AMMO", 1, [2255.9, 2.0, 1349.3], [0.0, 0.0, 0.0], 0.0],
                ["ERFPG_AMMO", 30, [2248.2, 2.0, 1343.6], [0.0, 0.0, 0.0], 0.0],
                ["NANITE", 15, [1371.7, 14.0, 677.6], [0.0, 0.0, 0.0], 60.0]]]"#,
        );
        let archive = bft_assets::ReaderData::from_entries(
            [("puppies.zrd".to_string(), json)].into_iter().collect(),
        );

        let placements = pickup_placements(&archive, Difficulty::Normal).expect("it reads");

        assert_eq!(placements.len(), 3);
        assert_eq!(placements[0].kind, "HEMORTAR_AMMO");
        assert_eq!(placements[0].amount, 1);
        assert_eq!(placements[0].pos, Vec3::new(2255.9, 2.0, 1349.3));
        assert_eq!(
            placements[0].respawn, None,
            "a campaign record's 0 means never, not instantly"
        );
        assert_eq!(placements[1].amount, 30);
        assert_eq!(placements[2].respawn, Some(60.0));
    }

    #[test]
    fn difficulty_picks_a_whole_file() {
        assert_eq!(puppies_entry(Difficulty::Easy), "puppies_easy.zrd");
        assert_eq!(puppies_entry(Difficulty::Normal), "puppies.zrd");
        assert_eq!(puppies_entry(Difficulty::Hard), "puppies_hard.zrd");
    }

    /// A multiplayer map ships `puppies.zrd` alone (m7-m13), so asking for a
    /// difficulty variant has to fall back rather than fail.
    #[test]
    fn a_missing_difficulty_variant_falls_back_to_the_plain_file() {
        // Two records, not one: `ReaderValue::root` unwraps a single-element
        // list of lists, so a one-record fixture would read as the record.
        let json = parse(
            r#"[[["NANITE", 15, [1.0, 2.0, 3.0], [0.0, 0.0, 0.0], 60.0],
                 ["NANITE100", 100, [4.0, 5.0, 6.0], [0.0, 0.0, 0.0], 60.0]]]"#,
        );
        let archive = bft_assets::ReaderData::from_entries(
            [("puppies.zrd".to_string(), json)].into_iter().collect(),
        );

        let placements = pickup_placements(&archive, Difficulty::Hard).expect("it falls back");

        assert_eq!(placements.len(), 2);
        assert_eq!(placements[0].kind, "NANITE");
    }

    /// A mission with no `puppies` file at all is still an error.
    #[test]
    fn a_mission_with_no_pickups_at_all_is_an_error() {
        let archive = bft_assets::ReaderData::default();

        assert_eq!(
            pickup_placements(&archive, Difficulty::Hard).unwrap_err(),
            PickupError::NoPuppiesEntry("puppies_hard.zrd")
        );
    }

    /// The counter is per kind, so two amphib chips are `pu03700` and
    /// `pu03701` even with other kinds between them.
    #[test]
    fn instance_names_count_per_kind_in_file_order() {
        let placement = |kind: &str| PickupPlacement {
            kind: kind.to_string(),
            amount: 1,
            pos: Vec3::ZERO,
            respawn: None,
        };

        let named = name_placements(&[
            placement("PUP_AMPHIB"),
            placement("ERFPG_AMMO"),
            placement("PUP_AMPHIB"),
        ]);

        let names: Vec<&str> = named.iter().map(|(_, name)| name.as_str()).collect();
        assert_eq!(names, vec!["pu03700", "pu00000", "pu03701"]);
    }
}
