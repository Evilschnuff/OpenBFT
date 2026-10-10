//! The anim handlers' checks against the shipped levels.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built. Only names and numbers docs/data/anims.md already publishes are
//! asserted; no game data enters the repository.
//!
//! Build the cache first with `open-bft extract`.

use std::collections::{BTreeMap, HashMap};

use avian3d::prelude::{Collider, ColliderDisabled};
use bevy::prelude::*;
use bft_assets::anim::{AnimSet, EventDataDesc, parse_anim_set};
use bft_assets::axes::AxisConvention;
use bft_assets::level::{LevelData, parse_gamez};
use bft_sim::anim::{
    AnimCallback, AnimInstance, AnimProgram, CallAnim, MISSION_END_CALLBACK, NodeActiveChanged,
    PlayerLocation, apply_initial_node_states, spawn_anim_instances,
};
use bft_sim::level::{LevelRoot, NodeActive, NodeIndex, NodeName};
use bft_sim::schedule::SIM_HZ;

fn cache() -> Option<std::path::PathBuf> {
    let root = bft_assets::test_support::data_root()?;
    match bft_assets::DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.join("anim").is_dir() => Some(paths.cache.path),
        Ok(paths) => {
            eprintln!(
                "no extraction cache at {}; run `open-bft extract`. Skipping.",
                paths.cache.path.display()
            );
            None
        }
        Err(e) => {
            eprintln!("cannot resolve the data paths ({e}); skipping");
            None
        }
    }
}

fn anim(cache: &std::path::Path, level: u16) -> Option<AnimSet> {
    let bytes = std::fs::read(cache.join(format!("anim/m{level}-anim.anim.zip"))).ok()?;
    Some(parse_anim_set(&bytes).expect("a real anim archive parses"))
}

fn gamez(cache: &std::path::Path, level: u16) -> Option<LevelData> {
    let bytes = std::fs::read(cache.join(format!("gamez/m{level}-gamez.gamez.zip"))).ok()?;
    Some(parse_gamez(&bytes, "gamez.zip").expect("a real gamez archive parses"))
}

/// Every event of a definition, its reset state included.
fn events(set: &AnimSet) -> impl Iterator<Item = &EventDataDesc> {
    set.defs.iter().flat_map(|def| {
        def.reset_state
            .iter()
            .chain(def.sequences.iter().flat_map(|seq| seq.events.iter()))
            .map(|event| &event.data)
    })
}

/// Every event kind in all 13 levels has a handler, or is control flow the
/// VM runs itself: zero `Unsupported`.
#[test]
fn every_event_kind_in_the_game_has_a_handler() {
    let Some(cache) = cache() else { return };
    let handlers = bft_sim::anim::all_handlers();

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for level in 1..=13u16 {
        let Some(set) = anim(&cache, level) else {
            continue;
        };
        for data in events(&set) {
            *counts.entry(data.kind().to_string()).or_default() += 1;
        }
    }
    if counts.is_empty() {
        return;
    }

    let control = ["Loop", "If", "Elseif", "Else", "Endif"];
    let missing: Vec<_> = counts
        .iter()
        .filter(|(kind, _)| !control.contains(&kind.as_str()) && handlers.get(kind).is_none())
        .collect();
    assert!(missing.is_empty(), "kinds with no handler: {missing:?}");
    assert_eq!(
        counts.len(),
        37,
        "the 37 kinds of docs/data/anims.md: {counts:?}"
    );
}

/// A headless app with a whole real level spawned — hierarchy, colliders,
/// scoped definitions and their initial states — as `bft-game` builds it.
fn real_level(cache: &std::path::Path, level: u16) -> Option<(App, AnimSet)> {
    let (set, data) = (anim(cache, level)?, gamez(cache, level)?);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(AxisConvention::GAMEZ_DEFAULT);
    app.finish();
    app.update();

    app.world_mut()
        .run_system_cached_with(
            |In(level): In<LevelData>, mut commands: Commands, axes: Res<AxisConvention>| {
                bft_sim::level::spawn_level_into(&mut commands, &level, &axes);
            },
            data,
        )
        .expect("the level spawns");

    app.world_mut()
        .run_system_cached_with(
            |In(set): In<AnimSet>,
             mut commands: Commands,
             nodes: Res<NodeIndex>,
             children: Query<&Children>,
             names: Query<&NodeName>,
             templates: Query<(), With<bft_sim::ai::Template>>,
             roots: Query<Entity, With<LevelRoot>>| {
                let Some(root) = roots.iter().next() else {
                    return;
                };
                let scopes = spawn_anim_instances(
                    &mut commands,
                    &set,
                    &nodes,
                    &children,
                    &names,
                    &templates,
                    root,
                );
                commands.insert_resource(scopes);
                commands.insert_resource(AnimProgram(set));
            },
            set.clone(),
        )
        .expect("the instances spawn");

    app.world_mut()
        .run_system_cached(
            |program: Res<AnimProgram>,
             scopes: Res<bft_sim::anim::AnimScopes>,
             nodes: Res<NodeIndex>,
             mut actives: Query<&mut NodeActive>| {
                apply_initial_node_states(&program.0, &scopes, &nodes, &mut actives);
            },
        )
        .expect("the initial states apply");

    // A few ticks so transforms propagate and the reset states run.
    for _ in 0..3 {
        app.update();
    }
    Some((app, set))
}

fn step(app: &mut App, seconds: f32) {
    for _ in 0..(seconds * SIM_HZ as f32).ceil() as usize {
        app.update();
    }
}

fn call(app: &mut App, name: &str) {
    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::new(name));
}

/// The node a definition names, resolved in its own scope the way the VM
/// does.
fn scoped(app: &App, set: &AnimSet, anim_name: &str, node: &str) -> Entity {
    let index = *set.lookup(anim_name).first().expect("the animation exists");
    let scopes = app.world().resource::<bft_sim::anim::AnimScopes>();
    scopes.0[index]
        .get(node)
        .copied()
        .or_else(|| {
            app.world()
                .resource::<NodeIndex>()
                .by_name(node)
                .first()
                .copied()
        })
        .expect("the node exists")
}

fn local(app: &App, entity: Entity) -> Vec3 {
    app.world()
        .get::<Transform>(entity)
        .expect("a node")
        .translation
}

fn colliders_disabled(app: &mut App, root: Entity) -> Option<bool> {
    let mut stack = vec![root];
    let mut any = None;
    while let Some(entity) = stack.pop() {
        if app.world().get::<Collider>(entity).is_some() {
            let disabled = app.world().get::<ColliderDisabled>(entity).is_some();
            any = Some(any.unwrap_or(true) && disabled);
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    any
}

/// m1: the dock house's doors slide open from where `gamez` put them, once
/// the three windows `dh_doors_open` requires are broken.
///
/// Every one of the eight windows calls it; before calls honoured
/// prerequisites, the first window shot opened the doors.
#[test]
fn m1s_dock_house_doors_open() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let right = scoped(&app, &set, "dh_doors_open", "dh_door_right");
    let before = local(&app, right);
    assert!(
        (before - Vec3::new(20.0, 2.0, -185.721)).length() < 1e-2,
        "the door starts where gamez has it: {before}"
    );

    call(&mut app, "broken_win_01");
    call(&mut app, "broken_win_05");
    step(&mut app, 1.0);
    assert!(
        (local(&app, right) - before).length() < 1e-4,
        "two of the three windows leave the doors shut"
    );

    call(&mut app, "broken_win_07");
    step(&mut app, 1.0);
    let moving = local(&app, right);
    assert!(moving.x > 20.5 && moving.x < 28.0, "sliding: {moving}");

    step(&mut app, 3.0);
    assert!(
        !app.world().get::<NodeActive>(right).expect("node").0,
        "the slid door is switched off at the end of its sequence"
    );
    assert!((local(&app, right).x - 28.0).abs() < 1e-2);
}

/// m2: `open_gate_01` drops *its own* `locked` six metres — there are three
/// `locked` nodes in m2 — then swaps it for `open`, and the dropped gate
/// stops blocking. It needs both of the gate's turrets damaged first.
#[test]
fn m2s_first_gate_drops_and_stops_blocking() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 2) else {
        return;
    };
    let locked = scoped(&app, &set, "open_gate_01", "locked");
    let open = scoped(&app, &set, "open_gate_01", "open");
    let others: Vec<Entity> = app
        .world()
        .resource::<NodeIndex>()
        .by_name("locked")
        .iter()
        .copied()
        .filter(|entity| *entity != locked)
        .collect();
    assert_eq!(others.len(), 2, "m2 has three `locked` nodes");
    assert!(!app.world().get::<NodeActive>(open).expect("node").0);

    call(&mut app, "gtrt_damage_01");
    call(&mut app, "open_gate_01");
    step(&mut app, 0.5);
    assert_eq!(local(&app, locked), Vec3::ZERO, "one turret keeps it shut");

    call(&mut app, "gtlt_damage_01");
    call(&mut app, "open_gate_01");
    step(&mut app, 1.5);
    let y = local(&app, locked).y;
    assert!((-4.0..-2.0).contains(&y), "half way down: {y}");
    for other in &others {
        assert_eq!(local(&app, *other), Vec3::ZERO, "the other gates stay put");
    }

    // `open` appears three seconds after the drop starts, and `locked` goes
    // three seconds after that: both events are `{Event, 3.0}`.
    step(&mut app, 2.0);
    assert!((local(&app, locked).y + 6.0).abs() < 1e-2);
    assert!(app.world().get::<NodeActive>(open).expect("node").0);
    step(&mut app, 3.0);
    assert!(!app.world().get::<NodeActive>(locked).expect("node").0);
    if let Some(disabled) = colliders_disabled(&mut app, locked) {
        assert!(disabled, "the dropped gate no longer collides");
    }
}

/// m3: the boiler elevator's door panels slide down in their staggered
/// order.
#[test]
fn m3s_elevator_door_opens() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 3) else {
        return;
    };
    let panel = scoped(&app, &set, "door1_open", "bdor11");
    let last = scoped(&app, &set, "door1_open", "bdor13");

    call(&mut app, "door1_open");
    step(&mut app, 2.0);
    assert!(local(&app, panel).y < -3.0, "the first panel is moving");
    assert_eq!(
        local(&app, last).y,
        0.0,
        "the last waits for its 3 s offset"
    );

    step(&mut app, 3.0);
    assert!((local(&app, panel).y + 10.8).abs() < 1e-2);
    assert!((local(&app, last).y + 3.6).abs() < 1e-2);
}

/// Every campaign level runs its entry animation and its startup
/// definitions for twenty seconds on the real hierarchy without a node
/// ending up somewhere non-finite, and the timed events actually run.
#[test]
fn every_campaign_levels_opening_runs_cleanly() {
    let Some(cache) = cache() else { return };

    let entry: HashMap<u16, &str> = HashMap::from([
        (1, "start_single_player"),
        (2, "m2_start_animation"),
        (3, "m3_start_animation"),
        (4, "m4_start_animation"),
        (5, "m5_start_animation"),
        (6, "m6_start_animation"),
    ]);

    for level in 1..=6u16 {
        let Some((mut app, _)) = real_level(&cache, level) else {
            continue;
        };
        call(&mut app, entry[&level]);
        step(&mut app, 20.0);

        let mut transforms = app.world_mut().query::<(&Transform, &NodeName)>();
        for (transform, name) in transforms.iter(app.world()) {
            assert!(
                transform.translation.is_finite()
                    && transform.rotation.is_finite()
                    && transform.scale.is_finite(),
                "m{level}: {} went non-finite: {transform:?}",
                name.0
            );
        }

        let moved = app
            .world_mut()
            .query::<&bft_sim::anim::AnimRest>()
            .iter(app.world())
            .count();
        let running = app
            .world_mut()
            .query::<&AnimInstance>()
            .iter(app.world())
            .filter(|instance| instance.is_running() || instance.completed)
            .count();
        eprintln!("m{level}: {running} definitions ran, {moved} nodes moved");
        assert!(running > 1, "m{level}: its opening started something");
    }
}

/// Every animation of every level, all called at once and run for ten
/// seconds on the real hierarchy, leaves every transform and every physics
/// position a real number.
///
/// m1's `xplflare.flt` shrinks at −20/s until its scale hits exactly zero;
/// a singular world matrix decomposes into NaN, which must not reach avian's
/// `Position`, where it panics.
#[test]
fn every_animation_at_once_keeps_every_transform_finite() {
    use avian3d::prelude::Position;

    let Some(cache) = cache() else { return };
    for level in 1..=13u16 {
        let Some((mut app, set)) = real_level(&cache, level) else {
            continue;
        };
        let names: Vec<String> = set.by_anim_name.keys().cloned().collect();
        for name in &names {
            call(&mut app, name);
        }
        for tick in 0..(SIM_HZ as usize * 10) {
            app.update();
            let mut nodes = app.world_mut().query::<(
                &Transform,
                &GlobalTransform,
                Option<&NodeName>,
                Option<&Position>,
            )>();
            for (transform, global, name, position) in nodes.iter(app.world()) {
                let finite = transform.translation.is_finite()
                    && transform.rotation.is_finite()
                    && transform.scale.is_finite()
                    && global.translation().is_finite()
                    && position.is_none_or(|p| p.0.is_finite());
                assert!(
                    finite,
                    "m{level} tick {tick}: {:?} went non-finite: {transform:?} {position:?}",
                    name.map(|n| &n.0)
                );
            }
        }
    }
}

/// Every destructible in the campaign breaks when shot.
///
/// Definitions with no enabled collider to aim at are counted, not shot:
/// the ones that start switched off, and the facade props (seagulls, burning
/// tyres) that get no collider at all.
///
/// For each `WeaponHit`/`WeaponOrCollideHit` definition without activation
/// prerequisites, a hit worth more than its `health` lands on a collider
/// under its `anim_root_name`, and the definition must run. This is what
/// failed before: hit definitions were never armed, and hits on a child of
/// the node that takes them were not counted. The 92 definitions rooted at a
/// piece their object lacks take no hits, and are counted with the rest that
/// have nothing to aim at.
#[test]
fn every_campaign_destructible_breaks_when_shot() {
    use avian3d::prelude::Collider;
    use bft_sim::damage::{DamageKind, NodeDamage};

    let Some(cache) = cache() else { return };
    for level in 1..=6u16 {
        let Some((mut app, set)) = real_level(&cache, level) else {
            continue;
        };
        let scopes = app
            .world()
            .resource::<bft_sim::anim::AnimScopes>()
            .0
            .clone();
        let index = app.world().resource::<NodeIndex>().clone();

        let mut targets: Vec<(usize, Entity, f32)> = Vec::new();
        let mut no_collider = 0usize;
        for (def_index, def) in set.defs.iter().enumerate() {
            if !def.activation.takes_weapon_hits() || !def.prereqs.is_empty() {
                continue;
            }
            let root = index.by_name(&def.name).first().copied();
            let hit_root = if def.anim_root_name == def.name {
                root
            } else {
                scopes[def_index].get(&def.anim_root_name).copied()
            };
            // The first enabled collider under it.
            let mut found = None;
            let mut stack: Vec<Entity> = hit_root.into_iter().collect();
            while let Some(entity) = stack.pop() {
                let world = app.world();
                if world.get::<Collider>(entity).is_some()
                    && world.get::<ColliderDisabled>(entity).is_none()
                {
                    found = Some(entity);
                    break;
                }
                if let Some(children) = world.get::<Children>(entity) {
                    stack.extend(children.iter());
                }
            }
            match found {
                Some(entity) => targets.push((def_index, entity, def.health)),
                None => no_collider += 1,
            }
        }

        for (_, entity, health) in &targets {
            let name = app
                .world()
                .get::<NodeName>(*entity)
                .map(|n| n.0.clone())
                .unwrap_or_default();
            app.world_mut()
                .resource_mut::<Messages<NodeDamage>>()
                .write(NodeDamage {
                    node: *entity,
                    name,
                    amount: health + 1.0,
                    kind: DamageKind::Projectile(0),
                    at: Vec3::ZERO,
                });
        }
        step(&mut app, 0.5);

        let ran: HashMap<usize, bool> = app
            .world_mut()
            .query::<&AnimInstance>()
            .iter(app.world())
            // Started, even if something stopped it since: shooting every
            // prop at once lets `vwbus` stop `shak` mid-run.
            .map(|instance| (instance.def, instance.started))
            .collect();
        let silent: Vec<&str> = targets
            .iter()
            .filter(|(def, _, _)| !ran.get(def).copied().unwrap_or(false))
            .map(|(def, _, _)| set.defs[*def].anim_name.as_str())
            .collect();
        eprintln!(
            "m{level}: {} shot, {} did not run, {no_collider} with no enabled collider",
            targets.len(),
            silent.len()
        );
        assert!(
            silent.is_empty(),
            "m{level}: shot but did not break: {silent:?}"
        );
    }
}

/// Run `seconds` of ticks and return every `AnimCallback` value they raised.
fn step_callbacks(app: &mut App, seconds: f32) -> Vec<i32> {
    let mut values = Vec::new();
    for _ in 0..(seconds * SIM_HZ as f32).ceil() as usize {
        app.update();
        let messages = app.world().resource::<Messages<AnimCallback>>();
        values.extend(messages.iter_current_update_messages().map(|c| c.value));
    }
    values
}

fn instance<'a>(app: &'a mut App, set: &AnimSet, anim_name: &str) -> &'a AnimInstance {
    let def = *set.lookup(anim_name).first().expect("the animation exists");
    app.world_mut()
        .query::<&AnimInstance>()
        .iter(app.world())
        .find(|instance| instance.def == def)
        .expect("every definition has an instance")
}

/// m1's VTOL comes only when all four targets are down, and the mission ends
/// only when the player then reaches it.
///
/// Each of `rocket_hit11`–`13` and `gate_lower` calls `m1pickup`, which is
/// gated on all four; `m1pickup` calls `m1_exit_trigger`, a 5.5 m `ByRange`
/// around the VTOL, whose exit animation raises callback 99. Calls used to
/// skip both gates, so the first rocket shot at the start of the level ended
/// the mission.
#[test]
fn m1s_vtol_waits_for_every_target_and_the_player() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };

    call(&mut app, "rocket_hit11");
    let fired = step_callbacks(&mut app, 3.0);
    assert!(!fired.contains(&MISSION_END_CALLBACK), "{fired:?}");
    assert!(
        !instance(&mut app, &set, "m1pickup").started,
        "one rocket down leaves three prerequisites"
    );

    call(&mut app, "rocket_hit12");
    call(&mut app, "rocket_hit13");
    call(&mut app, "gate_lower");
    // `gate_lower` calls `m1pickup` 2.5 s in, while it is still running.
    let fired = step_callbacks(&mut app, 5.0);
    assert!(!fired.contains(&MISSION_END_CALLBACK), "{fired:?}");
    assert!(
        instance(&mut app, &set, "m1pickup").started,
        "all four down brings the VTOL"
    );
    let trigger = instance(&mut app, &set, "m1_exit_trigger");
    assert!(
        trigger.armed && !trigger.started,
        "the exit trigger waits for the player"
    );

    // Walk the player onto the (moving) VTOL.
    let mut fired = Vec::new();
    for _ in 0..120 {
        let at = instance(&mut app, &set, "m1_exit_trigger").root_position;
        app.insert_resource(PlayerLocation {
            position: at,
            zone: None,
        });
        fired.extend(step_callbacks(&mut app, 0.5));
        if fired.contains(&MISSION_END_CALLBACK) {
            break;
        }
    }
    assert!(
        fired.contains(&MISSION_END_CALLBACK),
        "boarding the VTOL ends the mission: {fired:?}"
    );
}

/// m1: breaking one old-pier pillar breaks that pillar and nothing else.
///
/// `oldpiergone_01` calls the shared `generic_6` on `oldpier_01`. Its names
/// resolved in `generic6`'s own `healthy` subtree, where the sibling
/// `destroyed` and the parts are not, so they fell back to the level-wide
/// lookup: all 91 `destroyed` nodes flashed on and every `part1`–`part6` in
/// the level vanished.
#[test]
fn m1s_pier_pillar_breaks_alone() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let pier = scoped(&app, &set, "oldpiergone_01", "oldpier_01");
    let healthy = scoped(&app, &set, "oldpiergone_01", "healthy");

    let mut changed = Vec::new();
    call(&mut app, "oldpiergone_01");
    for _ in 0..(6.0 * SIM_HZ) as usize {
        app.update();
        let messages = app.world().resource::<Messages<NodeActiveChanged>>();
        changed.extend(
            messages
                .iter_current_update_messages()
                .map(|change| (change.node, change.name.clone(), change.active)),
        );
    }

    let under_pier = |mut entity: Entity| loop {
        if entity == pier {
            return true;
        }
        match app.world().get::<ChildOf>(entity) {
            Some(parent) => entity = parent.parent(),
            None => return false,
        }
    };
    let wrecks: Vec<&(Entity, String, bool)> = changed
        .iter()
        .filter(|(_, name, _)| name == "destroyed" || name.starts_with("part"))
        .collect();
    assert!(!wrecks.is_empty(), "the pillar broke");
    let elsewhere: Vec<&str> = wrecks
        .iter()
        .filter(|(node, _, _)| !under_pier(*node))
        .map(|(_, name, _)| name.as_str())
        .collect();
    assert!(elsewhere.is_empty(), "other props changed: {elsewhere:?}");
    assert!(
        !app.world().get::<NodeActive>(healthy).expect("node").0,
        "the intact pillar is gone"
    );
}

/// Whether `entity` is `ancestor` or under it.
fn is_under(app: &App, mut entity: Entity, ancestor: Entity) -> bool {
    loop {
        if entity == ancestor {
            return true;
        }
        match app.world().get::<ChildOf>(entity) {
            Some(parent) => entity = parent.parent(),
            None => return false,
        }
    }
}

/// Every node named `name` under `ancestor`.
fn named_under(app: &mut App, ancestor: Entity, name: &str) -> Vec<Entity> {
    let mut query = app.world_mut().query::<(Entity, &NodeName)>();
    let found: Vec<Entity> = query
        .iter(app.world())
        .filter(|(_, node)| node.0 == name)
        .map(|(entity, _)| entity)
        .collect();
    found
        .into_iter()
        .filter(|entity| is_under(app, *entity, ancestor))
        .collect()
}

/// How many copies ([`bft_sim::anim::AnimCopy`]) are running, of one
/// definition or of any.
fn copies_of(app: &mut App, set: &AnimSet, anim_name: Option<&str>) -> usize {
    let mut query = app.world_mut().query::<&AnimInstance>();
    query
        .iter(app.world())
        .filter(|instance| instance.copy.is_some())
        .filter(|instance| anim_name.is_none_or(|name| set.defs[instance.def].anim_name == name))
        .count()
}

/// Two pillars broken within `generic_6`'s four seconds both finish.
///
/// Each call plays its own copy. With one shared `generic_6` instance the
/// second call would restart it on the second pier, and the first pier's
/// `destroyed` wreck, due to go off at 4 s, would stay on for good.
#[test]
fn m1s_two_piers_broken_together_both_finish() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let piers = [
        scoped(&app, &set, "oldpiergone_01", "oldpier_01"),
        scoped(&app, &set, "oldpiergone_02", "oldpier_02"),
    ];

    call(&mut app, "oldpiergone_01");
    step(&mut app, 1.0);
    call(&mut app, "oldpiergone_02");
    step(&mut app, 0.1);
    assert_eq!(
        copies_of(&mut app, &set, Some("generic_6")),
        2,
        "one generic_6 per pier"
    );
    step(&mut app, 8.0);

    for pier in piers {
        for name in ["healthy", "destroyed", "part1", "part2", "part3"] {
            for node in named_under(&mut app, pier, name) {
                assert!(
                    !app.world().get::<NodeActive>(node).expect("a node").0,
                    "{name} under {pier} is still showing"
                );
            }
        }
    }
    assert_eq!(
        copies_of(&mut app, &set, Some("generic_6")),
        0,
        "both runs finished and went"
    );
}

/// A kill animation plays as a copy where it is called, its own `AtNode`
/// calls (`redparts`, `ring_red.flt`) play as copies of theirs, and every
/// one of them is gone once it has finished. The prototypes are untouched.
#[test]
fn m1s_kill_animation_plays_at_the_victim_and_cleans_up() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let prototype = scoped(&app, &set, "redparts", "expredparts.flt");
    let before: Vec<(Entity, bool)> = {
        let mut query = app.world_mut().query::<(Entity, &NodeActive)>();
        query
            .iter(app.world())
            .map(|(entity, active)| (entity, active.0))
            .filter(|(entity, _)| is_under(&app, *entity, prototype))
            .collect()
    };

    let victim = scoped(&app, &set, "oldpiergone_01", "oldpier_01");
    let at = app
        .world()
        .get::<GlobalTransform>(victim)
        .expect("a node")
        .translation();
    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::at("kill_laser", victim, Vec3::ZERO));
    step(&mut app, 0.1);

    let running = copies_of(&mut app, &set, None);
    assert!(running >= 2, "kill_laser and what it calls: {running}");
    let mut roots = app.world_mut().query::<(&NodeName, &GlobalTransform)>();
    let placed = roots
        .iter(app.world())
        .filter(|(name, _)| name.0 == "kill_laser")
        .any(|(_, global)| global.translation().distance(at) < 1e-3);
    assert!(placed, "a kill_laser copy stands at the victim");

    step(&mut app, 20.0);
    assert_eq!(
        copies_of(&mut app, &set, None),
        0,
        "every copy finished and went"
    );
    for (entity, active) in before {
        assert_eq!(
            app.world().get::<NodeActive>(entity).map(|state| state.0),
            Some(active),
            "the prototype played"
        );
    }
}

/// A dead scout plays `scou_easy` on its own model: the death animation of
/// its kind, rooted at the prototype it was cloned from.
#[test]
fn m1s_dead_scout_plays_its_death_animation_on_its_model() {
    let Some(cache) = cache() else { return };
    let Some(level) = gamez(&cache, 1) else {
        return;
    };
    let Some((mut app, _)) = real_level(&cache, 1) else {
        return;
    };
    let templates = bft_sim::ai::LevelTemplates::from_level(&level);
    let root = templates.root("scou_easy").expect("m1 has an easy scout");

    let at = Vec3::new(2337.6, 7.1, 1615.5);
    let (hull, model) = app
        .world_mut()
        .run_system_cached_with(
            move |In((level, root, at)): In<(LevelData, u32, Vec3)>, mut commands: Commands| {
                let hull = commands
                    .spawn((
                        bft_sim::ai::Enemy {
                            instance: "scou_easy_01".into(),
                            kind: bft_sim::vehicle::tuning::VehicleKindId("scou_easy".into()),
                        },
                        Transform::from_translation(at),
                    ))
                    .id();
                let cloned = bft_sim::ai::clone_template(
                    &mut commands,
                    &level,
                    &AxisConvention::GAMEZ_DEFAULT,
                    root,
                    Transform::IDENTITY,
                    "scou_easy_01 model",
                );
                commands.entity(cloned.root).insert(ChildOf(hull));
                (hull, cloned.root)
            },
            (level, root, at),
        )
        .expect("the scout spawns");
    step(&mut app, 0.1);
    let shadow = named_under(&mut app, model, "shadow")[0];
    assert!(app.world().get::<NodeActive>(shadow).expect("a node").0);

    app.world_mut()
        .resource_mut::<Messages<bft_sim::damage::Destroyed>>()
        .write(bft_sim::damage::Destroyed {
            entity: hull,
            by: None,
            at,
            weapon: None,
        });
    step(&mut app, 0.5);

    assert!(
        !app.world().get::<NodeActive>(shadow).expect("a node").0,
        "scou_easy turned this scout's shadow off"
    );
    // Both of `fly_descision`'s branches — a wreck left standing, or the
    // hull flung up and blown apart — end in `enemy_gone`, which takes the
    // intact model away.
    let healthy = named_under(&mut app, model, "healthy")[0];
    step(&mut app, 8.0);
    assert!(
        !app.world().get::<NodeActive>(healthy).expect("a node").0,
        "the intact scout is gone"
    );
}

/// The player's dust trail: driving `bft_00` along lays fading
/// segments behind both tracks that stay where they were laid, and a stopped
/// trail's segments are all gone a second or so later.
#[test]
fn m1s_dust_trail_lays_segments_that_stay_put_and_fade() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let bft = scoped(&app, &set, bft_sim::trails::DUST_LOOP, "bft_00");
    call(&mut app, bft_sim::trails::DUST_LOOP);

    // Two metres a second along x for two seconds, a tick at a time.
    let start = local(&app, bft);
    let ticks = (2.0 * SIM_HZ) as usize;
    for tick in 0..ticks {
        let at = start + Vec3::X * (2.0 * tick as f32 / SIM_HZ as f32);
        app.world_mut()
            .get_mut::<Transform>(bft)
            .expect("bft_00")
            .translation = at;
        app.update();
    }

    assert_eq!(
        copies_of(&mut app, &set, Some(bft_sim::trails::DUST_TRAIL)),
        2,
        "one trail per track"
    );
    let segments = |app: &mut App| -> Vec<(Entity, Vec3)> {
        let mut query = app.world_mut().query::<(Entity, &NodeName, &Transform)>();
        query
            .iter(app.world())
            .filter(|(_, name, _)| name.0 == "exhaust_trail_fade")
            .map(|(entity, _, transform)| (entity, transform.translation))
            .collect()
    };
    let before = segments(&mut app);
    // About a second's worth, one a tick per track, less the prototype.
    assert!(
        before.len() > 40,
        "fading segments behind the tracks: {}",
        before.len()
    );
    let spread = before
        .iter()
        .map(|(_, at)| at.x)
        .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
    assert!(
        spread.1 - spread.0 > 1.0,
        "laid along the path, not in one place: {spread:?}"
    );

    app.world_mut()
        .get_mut::<Transform>(bft)
        .expect("bft_00")
        .translation += Vec3::X * 5.0;
    app.update();
    let after: HashMap<Entity, Vec3> = segments(&mut app).into_iter().collect();
    let moved = before
        .iter()
        .filter(|(entity, at)| {
            after
                .get(entity)
                .is_some_and(|now| now.distance(*at) > 1e-3)
        })
        .count();
    assert_eq!(moved, 0, "a laid segment stays where it was laid");

    for name in [bft_sim::trails::DUST_LOOP, bft_sim::trails::DUST_TRAIL] {
        app.world_mut()
            .resource_mut::<Messages<bft_sim::anim::StopAnim>>()
            .write(bft_sim::anim::StopAnim {
                name: name.to_string(),
            });
    }
    step(&mut app, 2.0);
    assert_eq!(
        copies_of(&mut app, &set, None),
        0,
        "every segment faded and went"
    );
}

/// Shoot `node` for `amount`, as a projectile would.
fn shoot(app: &mut App, node: Entity, amount: f32) {
    use bft_sim::damage::{DamageKind, NodeDamage};

    let name = app
        .world()
        .get::<NodeName>(node)
        .map(|name| name.0.clone())
        .unwrap_or_default();
    app.world_mut()
        .resource_mut::<Messages<NodeDamage>>()
        .write(NodeDamage {
            node,
            name,
            amount,
            kind: DamageKind::Projectile(0),
            at: Vec3::ZERO,
        });
}

fn is_active(app: &App, entity: Entity) -> bool {
    app.world().get::<NodeActive>(entity).expect("a node").0
}

fn started(app: &mut App, set: &AnimSet, anim_name: &str) -> Vec<bool> {
    let defs = set.lookup(anim_name);
    app.world_mut()
        .query::<&AnimInstance>()
        .iter(app.world())
        .filter(|instance| defs.contains(&instance.def))
        .map(|instance| instance.started)
        .collect()
}

/// m1's fuel station roof falls toward whichever post was shot: two
/// definitions of one name, rooted at `postright` and `postwrong`, each
/// showing its own wreck. Both list the `healthy` group the posts share,
/// and when that list decided who took a hit, one shot fired both and the
/// second definition hid the first one's roof.
#[test]
fn m1s_fuel_station_roof_falls_toward_the_post_shot() {
    let Some(cache) = cache() else { return };
    for (post, shown, hidden) in [
        ("postright", "destroyed", "destroyed1"),
        ("postwrong", "destroyed1", "destroyed"),
    ] {
        let Some((mut app, set)) = real_level(&cache, 1) else {
            return;
        };
        let station = scoped(&app, &set, "fuel_station", "fuel_station");
        let post_node = named_under(&mut app, station, post)[0];
        let healthy = named_under(&mut app, station, "healthy")[0];
        let shown_node = named_under(&mut app, station, shown)[0];
        let hidden_node = named_under(&mut app, station, hidden)[0];
        assert!(!is_active(&app, shown_node), "{shown} starts hidden");

        shoot(&mut app, post_node, 20.0);
        step(&mut app, 3.0);

        assert!(!is_active(&app, healthy), "shooting {post} drops the roof");
        assert!(is_active(&app, shown_node), "shooting {post} shows {shown}");
        assert!(!is_active(&app, hidden_node), "and not {hidden}");
        let runs = started(&mut app, &set, "fuel_station");
        assert_eq!(
            runs.iter().filter(|run| **run).count(),
            1,
            "one collapse, not both: {runs:?}"
        );
    }
}

/// A hit on one prop does not break another that merely names something
/// it hit. m1's `shak` lists the `vwbus` it launches, so shooting the bus
/// must not break the shack and fly the bus off; `gate_part1` lists a
/// `dbase` its gate lacks and `liteh_hit` a `part1` the lighthouse lacks,
/// so any wreck's `dbase` or debris in the level must not feed them.
#[test]
fn m1s_props_take_only_their_own_hits() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };

    // The bus: 12 health to the shack's 4.
    let bus = scoped(&app, &set, "vwbus", "vwbus");
    let body = named_under(&mut app, bus, "body")[0];
    shoot(&mut app, body, 6.0);

    // Every standing `dbase` and `part1` outside the gate and the lighthouse.
    let gate = scoped(&app, &set, "gate_part1", "hshit_1");
    let lighthouse = scoped(&app, &set, "liteh_hit", "lighthouse");
    let mut wrecks = Vec::new();
    for name in ["dbase", "part1"] {
        let candidates: Vec<Entity> = app.world().resource::<NodeIndex>().by_name(name).to_vec();
        for node in candidates {
            if !is_under(&app, node, gate) && !is_under(&app, node, lighthouse) {
                wrecks.push(node);
            }
        }
    }
    assert!(!wrecks.is_empty(), "m1 has wrecks to shoot");
    for node in wrecks {
        shoot(&mut app, node, 20.0);
    }
    step(&mut app, 1.0);

    assert_eq!(
        started(&mut app, &set, "shak"),
        vec![false],
        "the shack stands"
    );
    assert_eq!(
        started(&mut app, &set, "vwbus"),
        vec![false],
        "6 of 12: the bus too"
    );
    assert_eq!(
        started(&mut app, &set, "gate_part1"),
        vec![false],
        "the gate stands"
    );
    assert_eq!(
        started(&mut app, &set, "liteh_hit"),
        vec![false],
        "the lighthouse stands"
    );
}

/// The first node named `name` under the first node named `parent`.
fn node_under(app: &mut App, parent: &str, name: &str) -> Entity {
    let parent = app.world().resource::<NodeIndex>().by_name(parent)[0];
    named_under(app, parent, name)[0]
}

/// A definition's listed names mean the nodes the data points at: each
/// `objects`/`nodes` entry carries the node the original engine bound it
/// to. m1's `hshit_1` gate resets a `dbase` its model lacks; the data binds
/// it to the VTOL wreck's, and resolved level-wide it hid every wreck base
/// in the level at load — the fuses' burnt bases among them.
#[test]
fn m1s_listed_names_bind_to_the_nodes_the_data_points_at() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let vtol = app.world().resource::<NodeIndex>().by_name("vtol2")[0];
    let gate_dbase = scoped(&app, &set, "gate_part1", "dbase");
    assert!(is_under(&app, gate_dbase, vtol), "bound to vtol2's wreck");

    // So the fuses' burnt bases survive the load.
    let fuse = app.world().resource::<NodeIndex>().by_name("bay1")[0];
    let base = named_under(&mut app, fuse, "dbase")[0];
    assert!(
        is_active(&app, base),
        "the fuse's burnt base is not hidden at load"
    );
}

/// Shooting a fuse under an arena laser swaps it for its burnt-out base
/// (`powrdest.tif`) in place, rather than leaving a hole in the wall. The
/// fuse calls its sparks at a node under it, and placing the fuse's root
/// there moved the whole fuse kilometres off.
#[test]
fn m1s_shot_fuse_leaves_its_burnt_base() {
    let Some(cache) = cache() else { return };
    let Some((mut app, _set)) = real_level(&cache, 1) else {
        return;
    };
    let healthy = node_under(&mut app, "bay1", "healthy");
    let panel = node_under(&mut app, "bay1", "g6");
    let destroyed = node_under(&mut app, "bay1", "destroyed");
    let base = node_under(&mut app, "bay1", "dbase");
    let place = |app: &App| {
        app.world()
            .get::<GlobalTransform>(base)
            .expect("a node")
            .translation()
    };
    let before = place(&app);

    shoot(&mut app, panel, 20.0);
    step(&mut app, 5.0);

    assert!(!is_active(&app, healthy), "the fuse blew");
    assert!(
        is_active(&app, destroyed) && is_active(&app, base),
        "its burnt base shows"
    );
    let after = place(&app);
    assert!(
        before.distance(after) < 0.01,
        "where the fuse was, not {before} -> {after}"
    );
}

/// m1's force-field generator: each shot on an emitter sends a shockwave
/// across, and a hit on a shockwave destroys the generator, drops the
/// field and lights the arena's lasers once the player is there. The
/// emitters' definitions have no timed reset and do not invalidate
/// themselves, so they fire again — read as one-shot, two missed
/// shockwaves left the generator, and so the mission, unwinnable.
#[test]
fn m1s_generator_falls_to_its_shockwave_and_lights_the_arena() {
    use bft_sim::anim::PlayerLocation;

    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    // The generator only powers up once `hivolt` is down.
    let hivolt = node_under(&mut app, "hivolt", "healthy");
    shoot(&mut app, hivolt, 20.0);
    step(&mut app, 3.0);

    let emitter = node_under(&mut app, "frcgen", "genleft");
    let shockwave = node_under(&mut app, "frcgen", "shwave");
    for attempt in 0..2 {
        shoot(&mut app, emitter, 1.0);
        step(&mut app, 0.1);
        assert!(
            is_active(&app, shockwave),
            "shot {attempt} sends a shockwave"
        );
        step(&mut app, 1.0);
        assert!(!is_active(&app, shockwave), "which crosses and goes");
    }
    assert_eq!(started(&mut app, &set, "destroy_the_gen"), vec![false]);

    shoot(&mut app, emitter, 1.0);
    step(&mut app, 0.1);
    shoot(&mut app, shockwave, 1.0);
    step(&mut app, 4.0);
    assert_eq!(started(&mut app, &set, "destroy_the_gen"), vec![true]);
    assert_eq!(
        started(&mut app, &set, "force_off"),
        vec![true],
        "the field drops"
    );

    // The lasers wait for the player in the arena's zone.
    let beam = app.world().resource::<NodeIndex>().by_name("beam1")[0];
    let position = app
        .world()
        .get::<GlobalTransform>(beam)
        .expect("a node")
        .translation();
    let zone = app
        .world()
        .get::<bft_sim::level::NodeZone>(beam)
        .map(|zone| zone.0);
    app.world_mut().insert_resource(PlayerLocation {
        position: Some(position),
        zone,
    });
    let mut lit = false;
    for _ in 0..(3.0 * SIM_HZ) as usize {
        app.update();
        lit |= is_active(&app, beam);
    }
    assert!(lit, "the arena's lasers fire");
}

/// A palm group is shot through its box, not its crossed quads. The palms
/// are billboards with no surface, and the group's one solid node, `g10`,
/// is two crossed quads flagged `intersect_bbox`: the original tested hits
/// against that node's box. Shot at the quads, most shots passed through.
#[test]
fn m1s_palm_group_is_hit_anywhere_in_its_box() {
    use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
    use bft_sim::level::{GameLayer, HitBoxOf};

    let Some(cache) = cache() else { return };
    let Some((mut app, _set)) = real_level(&cache, 1) else {
        return;
    };
    step(&mut app, 0.1);
    let group = app.world().resource::<NodeIndex>().by_name("plant_01")[0];
    let centre = app
        .world()
        .get::<GlobalTransform>(group)
        .expect("a node")
        .translation();

    let mut rays = Vec::new();
    for height in [3.0, 8.0, 14.0] {
        for step in 0..8 {
            let angle = step as f32 * std::f32::consts::FRAC_PI_4 + 0.3;
            // Just outside the box, so nothing else stands in between.
            let offset = Vec3::new(angle.cos(), 0.0, angle.sin()) * 9.0;
            rays.push((centre + offset + Vec3::Y * height, -offset.normalize()));
        }
    }
    let hits = app
        .world_mut()
        .run_system_cached_with(
            |In(rays): In<Vec<(Vec3, Vec3)>>, spatial: SpatialQuery, boxes: Query<&HitBoxOf>| {
                let filter = SpatialQueryFilter::from_mask([
                    GameLayer::Level,
                    GameLayer::Vehicle,
                    GameLayer::HitBox,
                ]);
                rays.iter()
                    .map(|(origin, dir)| {
                        let dir = Dir3::new(*dir).expect("a direction");
                        spatial
                            .cast_ray(*origin, dir, 60.0, true, &filter)
                            .map(|hit| boxes.get(hit.entity).map_or(hit.entity, |b| b.0))
                    })
                    .collect::<Vec<_>>()
            },
            rays,
        )
        .expect("the casts run");
    let on_tree = hits
        .iter()
        .filter(|hit| hit.is_some_and(|entity| is_under(&app, entity, group)))
        .count();
    assert_eq!(
        on_tree,
        hits.len(),
        "{on_tree} of {} shots hit the palms",
        hits.len()
    );
}

/// `smoke1` plays a copy where it is called, and the copy goes when done.
///
/// Its definition is rooted at the `smoke1` model node *under* the
/// `smoke1.flt` prototype, not at the prototype's root, so it was never
/// treated as a prototype: a placed call replayed the hidden original where
/// it was parked. `exp_medium`'s debris trails call it every 0.2 s, and not
/// one puff was ever seen.
#[test]
fn m1s_smoke_puff_plays_where_it_is_called() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let victim = scoped(&app, &set, "oldpiergone_01", "oldpier_01");
    let at = app
        .world()
        .get::<GlobalTransform>(victim)
        .expect("a node")
        .translation();

    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::at("smoke1", victim, Vec3::Y));
    step(&mut app, 0.1);

    assert_eq!(copies_of(&mut app, &set, Some("smoke1")), 1, "a copy plays");
    // Showing while it fades in, not only once it has.
    let mut puffs = app
        .world_mut()
        .query::<(&NodeName, &GlobalTransform, &NodeActive)>();
    let shown = puffs
        .iter(app.world())
        .filter(|(name, global, active)| {
            name.0 == "smoke1" && active.0 && global.translation().distance(at + Vec3::Y) < 1.0
        })
        .count();
    assert_eq!(shown, 1, "the puff is showing at the call");

    step(&mut app, 3.0);
    assert_eq!(copies_of(&mut app, &set, Some("smoke1")), 0, "and gone");
}

/// Where m1 has an easy scout, by the water.
const SCOUT_BY_THE_WATER: Vec3 = Vec3::new(2337.6, 8.1, 1615.5);

/// A mortar-sized explosion's debris trail smoke behind it, and stops.
///
/// `exp_medium` throws nine parts; three call `smoke1`/`smoke2` every 0.2 s
/// until they land, when their `bounce_sequence` stops the trail. A part
/// that fell in the water never landed, and its trail ran for ever.
#[test]
fn m1s_debris_trails_smoke_and_stop() {
    let Some(cache) = cache() else { return };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let name =
        bft_sim::anim::fire_animation_name(&set, "exp_medium.flt").expect("m1 has exp_medium");
    let level_root = {
        let mut roots = app.world_mut().query_filtered::<Entity, With<LevelRoot>>();
        roots.iter(app.world()).next().expect("a level root")
    };

    // A dozen, around the harbour where a dead scout of m1's sits, so some
    // of the randomly thrown parts come down in the water.
    for index in 0..12 {
        let angle = index as f32 * std::f32::consts::TAU / 12.0;
        let offset = Vec3::new(angle.cos(), 0.0, angle.sin()) * 25.0;
        app.world_mut()
            .resource_mut::<Messages<CallAnim>>()
            .write(CallAnim::at(
                name.clone(),
                level_root,
                SCOUT_BY_THE_WATER + offset,
            ));
    }

    let mut most = 0;
    for _ in 0..20 {
        step(&mut app, 0.1);
        most = most.max(
            copies_of(&mut app, &set, Some("smoke1")) + copies_of(&mut app, &set, Some("smoke2")),
        );
    }
    assert!(
        most >= 30,
        "the flying parts trail smoke: at most {most} puffs"
    );

    // Parts that fall in the water land on its surface, and one that lands
    // nowhere stops when its time is up.
    step(&mut app, 10.0);
    let trails =
        copies_of(&mut app, &set, Some("smoke1")) + copies_of(&mut app, &set, Some("smoke2"));
    assert_eq!(trails, 0, "every trail stopped");
    assert_eq!(
        copies_of(&mut app, &set, None),
        0,
        "and the explosion finished"
    );
}

/// The global weapon table, for the impact tests.
fn weapon_table(cache: &std::path::Path) -> Option<bft_sim::weapons::WeaponTable> {
    let bytes = std::fs::read(cache.join("reader/zrdr.rdr.zip")).ok()?;
    let reader = bft_assets::reader::parse_reader_data(&bytes).expect("the global reader parses");
    Some(
        bft_sim::weapons::build_weapon_table(
            &reader,
            &bft_sim::weapons::WeaponOverrides::default(),
            bft_sim::weapons::DEFAULT_TETHER_MAX_ALTITUDE,
        )
        .expect("the weapon table builds"),
    )
}

/// The `Effect` names the VM has put out, collected across updates.
#[derive(Resource, Default)]
struct EffectsSeen(Vec<String>);

fn collect_effects(
    mut outputs: MessageReader<bft_sim::anim::AnimOutput>,
    mut seen: ResMut<EffectsSeen>,
) {
    for output in outputs.read() {
        if output.kind == "Effect"
            && let Some(name) = output.value.get("name").and_then(|name| name.as_str())
        {
            seen.0.push(name.to_string());
        }
    }
}

/// Land one of a weapon's shots at `at`, on `hit` if given.
fn land(app: &mut App, weapon: usize, at: Vec3, hit: Option<Entity>) {
    app.world_mut()
        .resource_mut::<Messages<bft_sim::weapons::ProjectileHit>>()
        .write(bft_sim::weapons::ProjectileHit {
            projectile: Entity::PLACEHOLDER,
            at,
            normal: Vec3::Y,
            hit,
            hit_node: None,
            surface: bft_sim::weapons::Surface::Default,
            weapon,
        });
}

/// Where a shot lands plays what its `IMPACT` block names.
///
/// A mortar round in the dirt plays `impact_miss` — orange debris, a flare
/// and the `exp_bunk` sprite; the same round on a target it leaves standing plays its
/// `DAMAGE_ANIMATION`, `lg_impact.flt`; a mine goes off as its `EFFECT`.
#[test]
fn m1s_shots_land_with_their_impact_animations() {
    let Some(cache) = cache() else { return };
    let Some(weapons) = weapon_table(&cache) else {
        return;
    };
    let Some((mut app, set)) = real_level(&cache, 1) else {
        return;
    };
    let index = |name: &str| {
        weapons
            .weapons
            .iter()
            .position(|def| def.hud_name == name)
            .unwrap_or_else(|| panic!("a {name} weapon"))
    };
    let (mortar, mine) = (index("MDM"), index("P-MDM"));
    app.insert_resource(weapons);
    app.init_resource::<EffectsSeen>();
    app.add_systems(Update, collect_effects);

    land(&mut app, mortar, SCOUT_BY_THE_WATER, None);
    step(&mut app, 0.1);
    // `impact_miss` itself is over in the tick it starts; the flare it
    // calls is what is still burning.
    let flare = bft_sim::anim::fire_animation_name(&set, "flare_orange.flt").expect("a flare");
    assert!(
        copies_of(&mut app, &set, Some(&flare)) >= 1,
        "a miss plays impact_miss, and its flare"
    );
    let impact_miss_spawned = app.world().resource::<EffectsSeen>().0.clone();
    assert!(
        impact_miss_spawned.iter().any(|name| name == "exp_bunk"),
        "which shows the exp_bunk sprite: {impact_miss_spawned:?}"
    );

    let target = app
        .world_mut()
        .spawn((
            bft_sim::damage::Health::new(100.0),
            Transform::from_translation(SCOUT_BY_THE_WATER),
        ))
        .id();
    land(&mut app, mortar, SCOUT_BY_THE_WATER, Some(target));
    step(&mut app, 0.1);
    let damage = bft_sim::anim::fire_animation_name(&set, "lg_impact.flt").expect("lg_impact");
    assert!(
        copies_of(&mut app, &set, Some(&damage)) >= 1,
        "a target left standing plays the damage animation"
    );

    app.world_mut().resource_mut::<EffectsSeen>().0.clear();
    land(&mut app, mine, SCOUT_BY_THE_WATER, None);
    step(&mut app, 0.1);
    assert!(
        app.world()
            .resource::<EffectsSeen>()
            .0
            .iter()
            .any(|name| name == "exp_bunk"),
        "a mine goes off as its EFFECT"
    );
}
