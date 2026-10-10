//! Put the real `bft_00` subtree on the player's vehicle.
//!
//! Without it the player drives an invisible box.
//!
//! # Re-parented, not cloned
//!
//! `bft_00` is **not** a template root: in all 13 levels it is a child of the
//! World node (docs/data/nodes.md), so the level spawner already spawns it with its
//! meshes built — as scenery, parked at
//! a placeholder position. Only m1's `bft` twin is parentless.
//!
//! So the model is re-parented onto the hull rather than cloned from a
//! prototype. Cloning would have left the original standing in the level: a
//! second BFT, visible, at the placeholder.
//!
//! # The colliders have to come off
//!
//! Nearly every node of the subtree collides — 48 of m1's 49, 71 of m5's 72 —
//! so the level spawner gives each one a `RigidBody::Static` and a trimesh collider.
//! That is right for scenery and wrong the moment the subtree hangs off a
//! dynamic hull: a static body does not follow its parent, and avian resolves
//! it against the hull's box. The model lies flat, floats, then tips and
//! spins.
//!
//! So the physics comes off every node as it is re-parented. The vehicle's
//! collider is the hull box `spawn_player_vehicle` built from `vehicle.zrd`'s
//! `collision` ring; the model is a visual from here on.
//!
//! # The two joints
//!
//! The subtree separates yaw from pitch. `…/chassis/turret` and
//! `…/turret/gun` are both transform-only nodes with translation-only
//! transforms, one under the other — the shape of a joint — so the turret
//! ring yaws and the barrel alone pitches. This module marks them
//! [`TurretPart`] and [`GunPart`] and the vehicle's `aim_turret` writes to
//! each; the stand-in `TurretPart` it spawned is despawned here, so only one
//! entity claims the aim.
//!
//! # What is hidden
//!
//! From m5 the subtree carries every form's geometry at once — a tank chassis
//! and a sub chassis, three sets of wings, pontoons beside the tracks — and
//! the engine picks by visibility rather than by swapping models. The track
//! set is shown and the rest hidden. Switching on a form change belongs to
//! the anim VM, because the `t2a_anims` transitions that drive it are anim
//! defs.

use avian3d::prelude::{Collider, ColliderDensity, RigidBody};
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::level::LevelData;
use bft_sim::anim::PlayerModelRoot;
use bft_sim::level::{LevelStatic, NodeActive, NodeIndex, node_transform};
use bft_sim::states::AppState;
use bft_sim::vehicle::{AimJoints, AimPivot, GunPart, PlayerVehicle, TurretPart};
use bft_sim::weapons::{Firepoint, Muzzle, TwinBarrels, WeaponLoadout, WeaponTable};

use crate::level_view::LoadedLevel;
use crate::materials::CycledTexture;

/// The template root the player's model is cloned from.
///
/// Every level has one. m1 alone also has a structurally identical `bft`, which
/// is not used: `bft_00` is the name `playerName` binds (docs/data/interp.md)
/// and the only one all 13 levels agree on.
pub const PLAYER_TEMPLATE: &str = "bft_00";

/// The yaw joint: the turret ring.
const TURRET_NODE: &str = "turret";
/// The pitch joint: the barrel, under the ring.
const GUN_NODE: &str = "gun";
/// The centre firepoint, which is where a shot leaves the barrel.
const MUZZLE_NODE: &str = "fpnt_c";
/// The firepoints at the tips of the two side cannons, right then left.
const TWIN_NODES: [&str; 2] = ["fpnt_r", "fpnt_l"];
/// The suffix a twin-barrelled weapon's two models carry: `RFPG_R`.
const TWIN_SUFFIX: &str = "_R";

/// The nodes belonging to a form the player does not start in.
///
/// Hidden at spawn so the tank does not drive around wearing its pontoons and
/// all three sets of wings at once. Absent from the early levels' subtrees,
/// which carry only the tank set; a name that is not there is skipped.
const NON_TRACK_NODES: [&str; 8] = [
    "sub_chassis",
    "s_turret",
    "wings_hover",
    "wings_sub",
    "hover_shadow",
    "amphib_shadow",
    "rpont",
    "lpont",
];

/// The firepoints under the gun, which are markers rather than weapons.
const FIREPOINT_PREFIX: &str = "fpnt";

/// The regeneration shield, which should not be on until a pickup turns it on.
const SHIELD_NODE: &str = "new_skin";

/// The track nodes whose texture cycle [`advance_track_cycle`] drives.
const TRACK_NODES: [&str; 2] = ["rtracks", "ltracks"];

/// How far the tracks' texture scrolls per metre travelled, in frames.
///
/// // approximation: the data says only that the cycle is 4 frames at speed
/// `0.0`, which means the game advances it (docs/data/interp.md). It says nothing
/// about the rate. A metre and a half a frame reads as tracks that keep up
/// with the ground at walking pace and blur at speed.
const TRACK_FRAMES_PER_METRE: f32 = 1.0 / 1.5;

/// The player's model half of `GamePlugin`.
pub struct PlayerModelPlugin;

impl Plugin for PlayerModelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            OnEnter(AppState::InGame),
            // After the vehicle exists — and after a sync point, which is
            // the part that is easy to miss. `.after()` orders two systems
            // but does not flush the first one's `Commands` between them, so
            // without `ApplyDeferred` the hull and its stand-in turret are
            // still queued and this system queries an empty world.
            (ApplyDeferred, attach_to_player)
                .chain()
                .after(crate::play::spawn_player),
        )
        .add_systems(
            Update,
            (advance_track_cycle, show_selected_weapon).run_if(in_state(AppState::InGame)),
        );
    }
}

/// Find the freshly spawned player and give it its model.
///
/// A system rather than a call inside `spawn_player` because the stand-in
/// turret is a child entity, and a child spawned through `Commands` in the
/// same system is not queryable until those commands are applied.
pub fn attach_to_player(
    mut commands: Commands,
    loaded: Option<Res<LoadedLevel>>,
    levels: Res<Assets<LevelData>>,
    axes: Option<Res<AxisConvention>>,
    nodes: Option<Res<NodeIndex>>,
    players: Query<(Entity, Option<&Children>), With<PlayerVehicle>>,
    stand_ins: Query<Entity, With<TurretPart>>,
) {
    let Ok((hull, children)) = players.single() else {
        // No player is not this system's business to complain about:
        // `spawn_player` already said why it could not spawn one.
        debug!("no player vehicle to attach a model to");
        return;
    };

    let Some(level) = loaded.as_ref().and_then(|loaded| levels.get(&loaded.data)) else {
        warn!("no level data; the player keeps the debug box for a model");
        return;
    };

    let Some(nodes) = nodes else {
        warn!("no node index; the player keeps the debug box for a model");
        return;
    };

    let axes = axes
        .map(|axes| *axes)
        .unwrap_or(AxisConvention::GAMEZ_DEFAULT);

    let stand_in = children
        .into_iter()
        .flatten()
        .copied()
        .find(|entity| stand_ins.contains(*entity));

    let model = attach_player_model(&mut commands, level, &axes, &nodes, hull, stand_in);

    // The muzzle the model gives us replaces the spawn's approximation,
    // a guessed −3.5 m forward of the hull.
    if let Some(muzzle) = model.and_then(|model| model.muzzle) {
        commands.entity(hull).insert(Muzzle(muzzle));
    }
}

/// The cloned `bft_00` subtree on a player vehicle, and the parts of it the
/// game drives.
#[derive(Component, Debug, Clone)]
pub struct PlayerModel {
    /// The clone's root, a child of the hull.
    pub root: Entity,
    /// `…/chassis/turret`: the yaw joint, marked [`TurretPart`].
    pub turret: Option<Entity>,
    /// `…/turret/gun`: the pitch joint, marked [`GunPart`].
    pub gun: Option<Entity>,
    /// Where `fpnt_c` sits in hull space with the gun centred, which is
    /// what the weapons' [`Muzzle`] wants.
    pub muzzle: Option<Vec3>,
    /// The `rtracks`/`ltracks` nodes, whose texture cycle turns with the
    /// vehicle's speed.
    pub tracks: Vec<Entity>,
    /// The gun's weapon models by node name (`RFPG_R`, `HEM`, `LASER`, …),
    /// of which [`show_selected_weapon`] draws only the selected one.
    pub weapons: Vec<(String, Entity)>,
}

/// Clone `bft_00` onto a spawned player vehicle.
///
/// Returns `None` when the level has no such template, which is not a fault
/// worth failing on: the vehicle still drives, it is just invisible.
pub fn attach_player_model(
    commands: &mut Commands,
    level: &LevelData,
    axes: &AxisConvention,
    nodes: &NodeIndex,
    hull: Entity,
    stand_in_turret: Option<Entity>,
) -> Option<PlayerModel> {
    let root_id = find_in_level(level, PLAYER_TEMPLATE)?;
    let root = nodes.get(root_id)?;

    // Re-parented, not cloned. `bft_00` is already a live entity with its
    // meshes built: the level spawner spawned it with the rest of the World subtree.
    // Cloning would leave the original sitting at its placeholder position,
    // which is a second BFT parked in the level.
    //
    // Its own transform goes with it — that is the placeholder (in m1, 240 m
    // away at −70°; identity in the other twelve) — and is replaced by the
    // identity, so the hull's placement is the model's.
    //
    // `PlayerModelRoot` keeps the level scripts off it from here on. From m2
    // the start animations place `bft_00` in the VTOL's cargo bay and hide it
    // for the fly-in, which on the re-parented model hid the player for good.
    commands.entity(root).insert((
        ChildOf(hull),
        Transform::IDENTITY,
        NodeActive(true),
        PlayerModelRoot { hull },
    ));

    // Strip the physics the level spawner gave the subtree. Nearly every node of it
    // collides (48 of m1's 49, 71 of m5's 72), so as scenery each one got a
    // `RigidBody::Static` and a trimesh collider. Re-parenting those under a
    // dynamic hull is what made the model lie flat, float, then tip and spin:
    // static bodies do not follow a parent, and avian resolves the overlap
    // between them and the hull's own box.
    //
    // The vehicle's collider is the hull box from `vehicle.zrd`'s `collision`
    // ring, which `spawn_player_vehicle` already gave it. The model is a
    // visual from here on.
    for id in subtree(level, root_id) {
        let Some(entity) = nodes.get(id) else {
            continue;
        };
        commands
            .entity(entity)
            .remove::<(RigidBody, Collider, ColliderDensity, LevelStatic)>();
    }

    let turret = descendant(level, nodes, root_id, TURRET_NODE);
    let gun = descendant(level, nodes, root_id, GUN_NODE);

    // The aim goes on the model's own joints, so the vehicle's empty stand-in
    // has nothing left to do. Leaving it would put two `TurretPart`s under
    // one hull and `aim_turret` would write to both.
    if let (Some(_), Some(stand_in)) = (turret, stand_in_turret) {
        commands.entity(stand_in).despawn();
    }

    if let Some(turret) = turret {
        commands.entity(turret).insert(TurretPart);

        // Record the joints on the hull. `aim_turret` reads them off
        // `AimJoints` rather than searching, because the turret is four
        // levels down (`bft_00/body/chassis/turret`) and a scan of the
        // hull's direct children finds nothing.
        commands.entity(hull).insert(AimJoints {
            yaw: turret,
            pitch: gun,
        });
    }
    if let Some(gun) = gun {
        commands.entity(gun).insert(GunPart);
    }

    for name in NON_TRACK_NODES.iter().chain([&SHIELD_NODE]) {
        if let Some(entity) = descendant(level, nodes, root_id, name) {
            commands.entity(entity).insert(Visibility::Hidden);
        }
    }

    let tracks: Vec<Entity> = TRACK_NODES
        .iter()
        .filter_map(|name| descendant(level, nodes, root_id, name))
        .collect();

    // The muzzle, read off the data rather than guessed. `Muzzle` is in hull
    // space and `fpnt_c` is four joints down (`body/chassis/turret/gun`), so
    // the chain of transforms is composed rather than the leaf's translation
    // taken on its own. The root's own is skipped: that is the placeholder
    // this function just replaced.
    //
    // This is the aim *at rest*: the turret's yaw and the barrel's pitch
    // move the real firepoint every tick.
    let muzzle = muzzle_in_hull_space(level, axes, root_id);

    // That rest pose is only the start. `follow_firepoints` moves `Muzzle`
    // onto the live `fpnt_c` every tick, and `AimPivot` onto the gun, so the
    // shot leaves the barrel wherever it points and the turret aims from
    // where it pivots rather than from the hull's centre.
    if let Some(firepoint) = descendant(level, nodes, root_id, MUZZLE_NODE) {
        commands
            .entity(hull)
            .insert((Firepoint(firepoint), AimPivot::default()));
    }

    let weapons = weapon_models(level, nodes, root_id);

    // The weapons modelled as a pair of side cannons fire from their tips in
    // turn: the pulse guns, whose models are `RFPG_R`/`RFPG_L` and
    // `ERFPG_R`/`ERFPG_L`.
    let twins = TWIN_NODES.map(|name| descendant(level, nodes, root_id, name));
    if let [Some(right), Some(left)] = twins {
        let paired: Vec<String> = weapons
            .iter()
            .filter_map(|(name, _)| name.strip_suffix(TWIN_SUFFIX))
            .map(str::to_string)
            .collect();
        commands
            .entity(hull)
            .insert(TwinBarrels::new([right, left], paired));
    }
    // Hidden until the loadout says which to show, so the first frame is not
    // every weapon at once.
    for (_, entity) in &weapons {
        commands.entity(*entity).insert(Visibility::Hidden);
    }

    let model = PlayerModel {
        root,
        turret,
        gun,
        muzzle,
        tracks,
        weapons,
    };

    info!(
        "player model: `{PLAYER_TEMPLATE}` re-parented onto the hull, turret {}, gun {}, \
         {} track node(s), muzzle {}",
        if turret.is_some() { "found" } else { "MISSING" },
        if gun.is_some() { "found" } else { "MISSING" },
        model.tracks.len(),
        match muzzle {
            Some(at) => format!("at {at:.2?}"),
            None => "MISSING".to_string(),
        },
    );

    commands.entity(hull).insert(model.clone());

    Some(model)
}

/// Every node id in a subtree, breadth-first.
fn subtree(level: &LevelData, root: u32) -> Vec<u32> {
    let mut queue = std::collections::VecDeque::from([root]);
    let mut out = Vec::new();

    while let Some(id) = queue.pop_front() {
        // A child index that points nowhere is dropped, not fatal: the level
        // loader already warns about those at load.
        let Some(node) = level.node(id) else {
            continue;
        };
        out.push(id);
        queue.extend(node.common().children.iter().copied());
    }

    out
}

/// The node id of the first node with this name, anywhere in the level.
///
/// `bft_00` is unique in all 13 levels, so there is nothing to disambiguate.
/// m1's `bft` twin is a different name and deliberately not used.
fn find_in_level(level: &LevelData, name: &str) -> Option<u32> {
    (0..level.nodes.len() as u32).find(|id| level.node(*id).is_some_and(|node| node.name() == name))
}

/// The gun's children that are weapon models: every one but the firepoints.
///
/// All fifteen hang off the one `gun` node, each named for its weapon's
/// `NAME`, and gamez stores them all active: the game shows the selected one.
fn weapon_models(level: &LevelData, nodes: &NodeIndex, root: u32) -> Vec<(String, Entity)> {
    let Some(gun) = subtree(level, root)
        .into_iter()
        .find(|id| level.node(*id).is_some_and(|node| node.name() == GUN_NODE))
        .and_then(|id| level.node(id))
    else {
        return Vec::new();
    };

    gun.common()
        .children
        .iter()
        .filter_map(|id| Some((level.node(*id)?.name().to_string(), nodes.get(*id)?)))
        .filter(|(name, _)| !name.starts_with(FIREPOINT_PREFIX))
        .collect()
}

/// Whether the gun's `node` is the model of the weapon called `weapon`.
///
/// The node names are the weapons' `NAME`s, with an `_R`/`_L` suffix on the
/// twin-barrelled pulse guns: `RFPG` is `RFPG_R` and `RFPG_L`, while `ERFPG`
/// is its own pair and must not match `RFPG`.
pub fn is_weapon_model(node: &str, weapon: &str) -> bool {
    node == weapon
        || node
            .strip_prefix(weapon)
            .is_some_and(|rest| rest.starts_with('_'))
}

/// Whether a weapon model is one of a side-cannon pair (`RFPG_R`,
/// `ERFPG_L`) rather than a weapon under the turret doors.
pub fn is_side_model(node: &str) -> bool {
    node.ends_with("_R") || node.ends_with("_L")
}

/// Draw the side cannons of the pulse gun either trigger fires.
///
/// Only the side cannons: the weapons under the turret doors come out and go
/// away through them, which is `player_rig::swap_weapons`.
pub fn show_selected_weapon(
    table: Option<Res<WeaponTable>>,
    players: Query<(&PlayerModel, &WeaponLoadout), With<PlayerVehicle>>,
    mut visibilities: Query<&mut Visibility>,
) {
    let Ok((model, loadout)) = players.single() else {
        return;
    };
    let Some(table) = table else {
        return;
    };
    let fired = |secondary: bool| {
        loadout
            .trigger(secondary)
            .and_then(|(slot, strong)| table.slot(slot, strong))
            .map(|def| def.hud_name.as_str())
    };
    // With a pulse gun selected, both triggers name it.
    let shown = [fired(false), fired(true)];

    for (name, entity) in model.weapons.iter().filter(|(name, _)| is_side_model(name)) {
        let wanted = if shown
            .iter()
            .flatten()
            .any(|weapon| is_weapon_model(name, weapon))
        {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if let Ok(mut visibility) = visibilities.get_mut(*entity)
            && *visibility != wanted
        {
            *visibility = wanted;
        }
    }
}

/// The entity for the first node of this name under `root`.
///
/// Scoped to the subtree because vehicle part names are not unique across a
/// level — every enemy prototype has its own `turret` and firepoints — so
/// `NodeIndex::by_name` alone would return whichever was spawned first.
fn descendant(level: &LevelData, nodes: &NodeIndex, root: u32, name: &str) -> Option<Entity> {
    subtree(level, root)
        .into_iter()
        .find(|id| level.node(*id).is_some_and(|node| node.name() == name))
        .and_then(|id| nodes.get(id))
}

/// Where `fpnt_c` sits relative to the subtree's root, with the gun centred.
///
/// Composes the transforms from `root` down to the firepoint, skipping the
/// root's own — that one is the build-script placeholder (in m1, 240 m away
/// at −70°) which the clone replaces with the hull's placement.
fn muzzle_in_hull_space(level: &LevelData, axes: &AxisConvention, root: u32) -> Option<Vec3> {
    let path = path_to(level, root, MUZZLE_NODE)?;

    let composed = path
        .iter()
        .skip(1)
        .filter_map(|id| level.node(*id))
        .fold(Transform::IDENTITY, |acc, node| {
            acc * node_transform(node, axes)
        });

    Some(composed.translation)
}

/// The chain of node ids from `root` to the first node named `name`.
///
/// Breadth-first, so the shallowest match wins, matching
/// [`ClonedTemplate::find`](bft_sim::ai::ClonedTemplate::find).
fn path_to(level: &LevelData, root: u32, name: &str) -> Option<Vec<u32>> {
    let mut queue = std::collections::VecDeque::from([vec![root]]);

    while let Some(path) = queue.pop_front() {
        let id = *path.last()?;
        // A child index that points nowhere is dropped, not fatal: the level
        // loader already warns about those at load.
        let Some(node) = level.node(id) else {
            continue;
        };
        if node.name() == name {
            return Some(path);
        }
        for child in &node.common().children {
            let mut next = path.clone();
            next.push(*child);
            queue.push_back(next);
        }
    }

    None
}

/// Half the distance between the two tracks' centre lines, in metres.
///
/// Measured: `rtracks` spans x 0.82…1.67 in `bft_00`'s frame and `ltracks`
/// mirrors it, so each track runs 1.25 m out from the centre line.
const TRACK_HALF_GAUGE: f32 = 1.25;

/// How far one track has rolled from a hull's motion, per side, signed.
///
/// A tracked vehicle turns by running its tracks at different speeds, so a
/// pivot turn on the spot rolls them in opposite directions and a vehicle
/// driving straight rolls both alike. Forward is the hull's −z and a positive
/// yaw rate turns left, so the right track runs faster in a left turn.
pub fn track_speeds(forward_speed: f32, yaw_rate: f32) -> [f32; 2] {
    let turn = yaw_rate * TRACK_HALF_GAUGE;
    // [right, left], the order of `TRACK_NODES`.
    [forward_speed + turn, forward_speed - turn]
}

/// The frame of a track's cycle after rolling `travelled` metres.
///
/// Signed, so a track rolling backwards runs its cycle backwards.
pub fn track_frame(travelled: f32, frames: usize) -> usize {
    if frames == 0 {
        return 0;
    }
    ((travelled * TRACK_FRAMES_PER_METRE).floor() as i64).rem_euclid(frames as i64) as usize
}

/// Advance the tracks' texture cycles from the vehicle's motion.
///
/// The interp script binds `rtracks` and `ltracks` a 4-frame cycle with speed
/// `0.0` and looping off — the only such binding in the game — which
/// docs/data/interp.md reads as "the game advances the frame, not the clock".
/// This is the game. The cycle lives on the tracks' mesh parts
/// (`claim_animated_parts`), and [`CycledTexture`]'s own system writes out
/// whatever frame is set here.
///
/// Each track rolls by itself ([`track_speeds`]): turning on the spot runs
/// them against each other, as a tracked vehicle's do.
pub fn advance_track_cycle(
    time: Res<Time>,
    players: Query<
        (
            &PlayerModel,
            &GlobalTransform,
            &avian3d::prelude::LinearVelocity,
            &avian3d::prelude::AngularVelocity,
        ),
        With<PlayerVehicle>,
    >,
    children: Query<&Children>,
    mut cycles: Query<&mut CycledTexture>,
    mut travelled: Local<[f32; 2]>,
) {
    let Ok((model, hull, velocity, angular)) = players.single() else {
        return;
    };

    let rotation = hull.rotation();
    let forward = velocity.0.dot(rotation * Vec3::NEG_Z);
    let yaw_rate = angular.0.dot(rotation * Vec3::Y);
    let speeds = track_speeds(forward, yaw_rate);

    // Distance, not speed: the frame is a function of how far the track has
    // rolled, so a vehicle that stops mid-frame stays on it.
    for (side, entity) in model.tracks.iter().enumerate().take(2) {
        travelled[side] += speeds[side] * time.delta_secs();

        let mut stack = vec![*entity];
        while let Some(entity) = stack.pop() {
            if let Ok(mut cycle) = cycles.get_mut(entity) {
                let frame = track_frame(travelled[side], cycle.frames.len());
                if cycle.frame != frame {
                    cycle.frame = frame;
                }
            }
            if let Ok(kids) = children.get(entity) {
                stack.extend(kids.iter());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The twin pulse-gun barrels are the pulse gun's; the enhanced pulse
    /// gun's pair is not, though its name ends in the same four letters.
    #[test]
    fn a_weapon_owns_its_own_nodes_and_no_others() {
        assert!(is_weapon_model("RFPG_R", "RFPG"));
        assert!(is_weapon_model("RFPG_L", "RFPG"));
        assert!(is_weapon_model("HEM", "HEM"));
        assert!(!is_weapon_model("ERFPG_R", "RFPG"));
        assert!(!is_weapon_model("P-HEM", "HEM"));
        assert!(!is_weapon_model("LASERD", "LASER"));
        assert!(!is_weapon_model("LASER", "LASERD"));
    }
}
