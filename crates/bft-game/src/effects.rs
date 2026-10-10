//! Visual effects: sprite effects, tracers, beams and the nuke's flash.
//!
//! The original's effects are data. `effects.zrd` names 13 sprite effects,
//! each a model (`expbunk.flt`, `smoke1.flt`, `fire1.flt`…) and the texture
//! cycle it plays (docs/data/materials.md, READER-SCHEMA §2.7); anim `Effect` events
//! and weapon `IMPACT/EFFECT` keys place them, and [`anim_effects`] draws
//! each as a copy of its model for as long as its cycle runs. Everything
//! else a shot does where it lands — `impact_miss`'s debris and flare, the
//! pulse guns' ricochets, the kill animations — is an anim definition the
//! sim plays (`call_impact_animations`, `call_destruction_animations`).
//!
//! What is left here is ours. A few procedurally drawn sprites ([`sprites`])
//! stand in where the data has nothing to draw: an effect whose model no
//! level carries (the two splash cycles), a death with no animation, a
//! projectile with no model. A [`SpawnEffect`] makes one as a
//! [`Billboard`]: a camera-facing quad whose texture steps through its
//! frames and whose alpha fades over its life. Projectiles with no model get
//! a [`Tracer`], a quad stretched along their velocity, beams a streak, and
//! the nuke a full-screen white flash.

use avian3d::prelude::ColliderAabb;
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::level::LevelData;
use bft_sim::ai::templates::{self, LevelTemplates};
use bft_sim::anim::AnimOutput;
use bft_sim::damage::Destroyed;
use bft_sim::states::AppState;
use bft_sim::weapons::{Mover, Projectile, ProjectileHit, ProjectileSpawned, Surface, WeaponTable};

use bft_sim::level::node_transform;

use bft_assets::textures::{TexturePool, TextureSet};

use crate::level_view::LoadedLevel;
use crate::materials::{CycledTexture, EffectTextureCycles};

pub mod sprites;

/// Every placeholder effect.
pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SpawnEffect>()
            .add_systems(Startup, make_sprites)
            .add_systems(
                Update,
                (
                    (
                        weapon_effects,
                        dress_projectiles,
                        destruction_effects,
                        anim_effects,
                    )
                        .run_if(in_state(AppState::InGame)),
                    spawn_effects,
                    (
                        animate_billboards,
                        expire_effect_sprites,
                        face_billboards,
                        steer_tracers,
                        fade_flashes,
                    ),
                )
                    .chain(),
            );
    }
}

/// Which placeholder to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectKind {
    /// A brief flash where a shot leaves the barrel.
    MuzzleFlash,
    /// A small burst where a direct-fire shot lands.
    Impact,
    /// A fireball: area weapons, destroyed things, `exp_*` anim effects.
    Explosion,
    /// A slow grey plume that rises.
    Smoke,
    /// A looping flame on a node: it burns while the node is visible, for
    /// [`FIRE_SECONDS`] at most, and dies down over the second half.
    Fire,
    /// White water.
    Splash,
    /// The practical nuke: a huge fireball and a white screen.
    Nuke,
}

/// Show an effect.
///
/// `attached` exists for `fire1`, the second-commonest anim
/// effect, loops on its node until the node is switched off, which needs the
/// node. With `attached`, `at` is an offset in that entity's space.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct SpawnEffect {
    pub kind: EffectKind,
    pub at: Vec3,
    /// The surface normal at an impact, which lifts the effect off it.
    pub normal: Option<Vec3>,
    /// Multiplies the kind's size.
    pub scale: f32,
    pub attached: Option<Entity>,
}

impl SpawnEffect {
    /// An effect of the kind's own size at a world position.
    pub fn at(kind: EffectKind, at: Vec3) -> SpawnEffect {
        SpawnEffect {
            kind,
            at,
            normal: None,
            scale: 1.0,
            attached: None,
        }
    }
}

/// A camera-facing, frame-animated quad.
#[derive(Component, Debug, Clone)]
pub struct Billboard {
    pub frames: Vec<Handle<Image>>,
    /// Frames per second. The last frame holds once reached, unless the
    /// billboard loops.
    pub fps: f32,
    /// Seconds since it appeared.
    pub age: f32,
    /// Seconds it lives; `f32::INFINITY` for ever.
    pub lifetime: f32,
    /// Whether the frames wrap rather than holding the last one.
    pub looping: bool,
}

impl Billboard {
    /// The frame to show at the current age.
    pub fn frame(&self) -> usize {
        let count = self.frames.len().max(1);
        let step = (self.age.max(0.0) * self.fps) as usize;
        if self.looping {
            step % count
        } else {
            step.min(count - 1)
        }
    }

    /// How opaque it is: 1 for the first half of its life, fading to 0.
    pub fn opacity(&self) -> f32 {
        if !self.lifetime.is_finite() {
            return 1.0;
        }
        let t = (self.age / self.lifetime).clamp(0.0, 1.0);
        if t < 0.5 { 1.0 } else { 2.0 * (1.0 - t) }
    }

    /// Whether it has lived out its life.
    pub fn finished(&self) -> bool {
        self.age >= self.lifetime
    }
}

/// A projectile's streak. A child of the projectile, so it goes with it.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Tracer;

/// How an effect kind looks.
#[derive(Debug, Clone, Copy)]
pub struct EffectLook {
    /// Edge length of the quad at scale 1, metres.
    pub size: f32,
    pub lifetime: f32,
    pub color: Color,
    /// Additive for fire and light; alpha-blended for smoke and water.
    pub additive: bool,
    /// How much it grows over its life: final size = size × (1 + grow).
    pub grow: f32,
    /// Metres per second upward.
    pub rise: f32,
}

/// The placeholder look of each kind. Presentation, not gameplay tuning, so
/// constants here rather than a RON file.
pub fn look(kind: EffectKind) -> EffectLook {
    let (size, lifetime, color, additive, grow, rise) = match kind {
        EffectKind::MuzzleFlash => (1.6, 0.08, Color::srgb(1.0, 0.85, 0.5), true, 0.2, 0.0),
        EffectKind::Impact => (2.0, 0.35, Color::srgb(1.0, 0.6, 0.25), true, 0.6, 0.0),
        EffectKind::Explosion => (7.0, 0.9, Color::srgb(1.0, 0.55, 0.2), true, 0.8, 1.0),
        EffectKind::Smoke => (
            6.0,
            2.5,
            Color::srgba(0.35, 0.33, 0.3, 0.7),
            false,
            1.5,
            3.0,
        ),
        EffectKind::Fire => (
            3.0,
            FIRE_SECONDS,
            Color::srgb(1.0, 0.5, 0.15),
            true,
            0.0,
            0.0,
        ),
        EffectKind::Splash => (
            4.0,
            0.7,
            Color::srgba(0.85, 0.92, 1.0, 0.8),
            false,
            1.0,
            2.0,
        ),
        EffectKind::Nuke => (120.0, 3.5, Color::srgb(1.0, 0.9, 0.7), true, 1.5, 8.0),
    };
    EffectLook {
        size,
        lifetime,
        color,
        additive,
        grow,
        rise,
    }
}

/// The longest a placeholder flame burns, seconds.
///
/// `fire1` is `effects.zrd`'s one `LOOPING ON` cycle, and an anim `Effect`
/// that places it never stops it: the scripts rely on hiding the node it
/// burns on (a tree's `destroyed` wreck at 3 s, a dead man at about a
/// second). A flame on a node that stays up — or on one the VM has already
/// finished with — burned on for good, so it now dies down on its own.
///
/// // approximation: longer than any script keeps its node up to show one,
/// short enough that nothing is left alight.
pub const FIRE_SECONDS: f32 = 8.0;

impl EffectKind {
    /// Whether its frames loop over its life rather than playing once.
    pub fn loops(self) -> bool {
        self == EffectKind::Fire
    }
}

/// Which placeholder an `effects.zrd` name stands for.
///
/// The nine names anim `Effect` events use, counted over all 13 levels:
/// `exp_crate` 1145, `fire1` 366, `videxp` 266, `exp_bunk` 241, `smoke1` 209,
/// `exp_box` 29, `exp_fireup` 26, `watblast` 14, `splash_dam` 3.
pub fn effect_kind_for_name(name: &str) -> EffectKind {
    let name = name.to_ascii_lowercase();
    if name.starts_with("fire") {
        EffectKind::Fire
    } else if name.contains("smok") || name.contains("steam") {
        EffectKind::Smoke
    } else if name.contains("splash") || name.contains("splsh") || name.starts_with("wat") {
        EffectKind::Splash
    } else if name.contains("exp") || name.contains("brst") {
        EffectKind::Explosion
    } else {
        EffectKind::Impact
    }
}

/// The sprites and the quad every effect shares.
#[derive(Resource, Debug, Clone)]
pub struct EffectSprites {
    pub quad: Handle<Mesh>,
    pub puff: Vec<Handle<Image>>,
    pub flash: Vec<Handle<Image>>,
    pub flame: Vec<Handle<Image>>,
    /// The material all tracers share.
    pub tracer: Handle<StandardMaterial>,
}

impl EffectSprites {
    fn frames(&self, kind: EffectKind) -> (&[Handle<Image>], f32) {
        match kind {
            EffectKind::MuzzleFlash => (&self.flash, sprites::FLASH_FRAMES as f32 / 0.08),
            EffectKind::Fire => (&self.flame, 12.0),
            _ => {
                let lifetime = look(kind).lifetime;
                (&self.puff, sprites::PUFF_FRAMES as f32 / lifetime)
            }
        }
    }
}

/// Per-effect motion and tint, beside [`Billboard`].
#[derive(Component, Debug, Clone, Copy)]
struct EffectMotion {
    size: f32,
    grow: f32,
    rise: f32,
    color: Color,
    shown: usize,
}

/// A beam weapon's streak: shooter to hit point, gone in a tenth of a second.
#[derive(Component, Debug, Clone, Copy)]
struct BeamFlash {
    age: f32,
}

/// How long a beam's streak shows, seconds.
const BEAM_SECONDS: f32 = 0.1;

/// The white screen after a nuke.
#[derive(Component, Debug, Clone, Copy)]
pub struct NukeFlash {
    pub age: f32,
    pub duration: f32,
}

/// How long the nuke's white-out takes to clear, seconds.
const NUKE_FLASH_SECONDS: f32 = 1.5;

/// How long a tracer is per m/s of projectile speed.
const TRACER_SECONDS: f32 = 0.04;

/// Tracer width, metres.
const TRACER_WIDTH: f32 = 0.3;

/// How much larger a mine is drawn than its model.
///
/// // approximation: the four mine models (`mine_ph1.flt` … `mine_re2.flt`)
/// carry no scale, unlike the mortar shell's 4 × 4 × 8, and their body is
/// 0.84 m across. Sized by eye against the original's footage, where a
/// lying mine is far larger than the mine model on the launcher rail: about
/// 3.8 m, two thirds of the BFT's 5.66 m (`bft_00`'s bounding box). A third
/// of it was still half the size the footage shows.
pub const MINE_MODEL_SCALE: f32 = 5.66 * 2.0 / 3.0 / 0.84;

fn make_sprites(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut frames = |make: fn(usize, usize) -> Image, count: usize| {
        (0..count)
            .map(|frame| images.add(make(frame, count)))
            .collect::<Vec<_>>()
    };
    let puff = frames(sprites::puff, sprites::PUFF_FRAMES);
    let flash = frames(sprites::flash, sprites::FLASH_FRAMES);
    let flame = frames(sprites::flame, sprites::FLAME_FRAMES);

    let tracer = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.85, 0.45),
        base_color_texture: Some(flash[0].clone()),
        unlit: true,
        alpha_mode: AlphaMode::Add,
        cull_mode: None,
        ..default()
    });

    commands.insert_resource(EffectSprites {
        quad: meshes.add(Rectangle::new(1.0, 1.0)),
        puff,
        flash,
        flame,
        tracer,
    });
}

/// A beam's streak, and the nuke's white-out, from the weapon messages.
fn weapon_effects(
    mut commands: Commands,
    mut hits: MessageReader<ProjectileHit>,
    table: Option<Res<WeaponTable>>,
    sprites: Option<Res<EffectSprites>>,
    shooters: Query<&GlobalTransform>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let (Some(table), Some(sprites)) = (table, sprites) else {
        hits.clear();
        return;
    };

    // No muzzle flash here any more: a shot plays its weapon's own
    // `FIRE/ANIMATION` through the anim VM (`call_fire_animations`), and a
    // weapon whose data names none has no flash in the original.
    for hit in hits.read() {
        let Some(def) = table.by_index(hit.weapon) else {
            continue;
        };

        if def.mover == Mover::Beam
            && let Ok(from) = shooters.get(hit.projectile)
        {
            let material = materials.add(StandardMaterial {
                base_color: Color::srgb(0.6, 0.9, 1.0),
                base_color_texture: Some(sprites.flash[0].clone()),
                unlit: true,
                alpha_mode: AlphaMode::Add,
                cull_mode: None,
                ..default()
            });
            let start = from.translation() + Vec3::Y;
            commands.spawn((
                Name::new("beam"),
                BeamFlash { age: 0.0 },
                Mesh3d(sprites.quad.clone()),
                MeshMaterial3d(material),
                streak(start, hit.at, None, TRACER_WIDTH * 2.0),
                DespawnOnExit(AppState::InGame),
            ));
        }

        // The impact itself is the data's: `call_impact_animations` plays
        // the weapon's `IMPACT` animations and `EFFECT` sprite. What is left
        // here is the nukes' white-out, which no anim draws.
        if def.impact_always {
            spawn_nuke_flash(&mut commands);
        }
    }
}

/// The practical nuke's white screen, clearing over [`NUKE_FLASH_SECONDS`].
fn spawn_nuke_flash(commands: &mut Commands) {
    commands.spawn((
        Name::new("nuke flash"),
        NukeFlash {
            age: 0.0,
            duration: NUKE_FLASH_SECONDS,
        },
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::WHITE),
        GlobalZIndex(i32::MAX - 1),
        DespawnOnExit(AppState::InGame),
    ));
}

/// Give each new projectile its look: the weapon's `FLYOUT/MODEL`, cloned
/// from the level's template, or the placeholder [`Tracer`] where the level
/// has no such template.
///
/// The pulse gun's model is itself a `tracer` mesh (`rfpg_grn.flt`), the
/// mortar's a shell, the missiles' a missile.
///
/// // approximation: `FLYOUT/MODEL_ANIMATION` (the pulse bolts' green and red
/// point lights) is not played. It loops for ever, and the VM retires a copy
/// only when its script ends, so every shot would leave an instance behind.
#[allow(clippy::too_many_arguments)]
fn dress_projectiles(
    mut commands: Commands,
    mut spawned: MessageReader<ProjectileSpawned>,
    table: Option<Res<WeaponTable>>,
    sprites: Option<Res<EffectSprites>>,
    loaded: Option<Res<LoadedLevel>>,
    levels: Res<Assets<LevelData>>,
    axes: Option<Res<AxisConvention>>,
    mut index: Local<Option<(AssetId<LevelData>, LevelTemplates)>>,
    alive: Query<(), With<Projectile>>,
) {
    let (Some(table), Some(sprites)) = (table, sprites) else {
        spawned.clear();
        return;
    };
    let level = loaded
        .as_ref()
        .and_then(|loaded| Some((loaded.data.id(), levels.get(&loaded.data)?)));
    if let Some((id, level)) = level
        && index.as_ref().is_none_or(|(built_for, _)| *built_for != id)
    {
        *index = Some((id, LevelTemplates::from_level(level)));
    }
    let axes = axes
        .map(|axes| *axes)
        .unwrap_or(AxisConvention::GAMEZ_DEFAULT);

    for event in spawned.read() {
        // A projectile can hit on the tick it was fired and be gone by the
        // frame that reads this.
        if !alive.contains(event.entity) {
            continue;
        }
        let Some(def) = table.by_index(event.weapon) else {
            continue;
        };
        // The sim builds without a renderer, so the projectile has no
        // `Visibility` of its own for its model to inherit.
        commands.entity(event.entity).insert(Visibility::default());

        let template = level
            .map(|(_, level)| level)
            .zip(index.as_ref())
            .zip(def.flyout.model.as_deref())
            .and_then(|((level, (_, index)), name)| Some((level, index.root(name)?)));

        if let Some((level, root)) = template {
            // The root's own transform, not identity: the mortar shell's
            // carries its 4 x 4 x 8 scale.
            let mut at = level
                .node(root)
                .map(|node| node_transform(node, &axes))
                .unwrap_or_default();
            if def.mover == Mover::Mine {
                at.scale *= MINE_MODEL_SCALE;
            }
            let cloned = templates::clone_template(
                &mut commands,
                level,
                &axes,
                root,
                at,
                &format!("flyout {}", def.id),
            );
            commands.entity(cloned.root).insert(ChildOf(event.entity));
            continue;
        }

        // Mines sit still and need no streak.
        if def.mover != Mover::Mine {
            commands.spawn((
                Name::new("tracer"),
                Tracer,
                Mesh3d(sprites.quad.clone()),
                MeshMaterial3d(sprites.tracer.clone()),
                Transform::default(),
                // Hidden until `steer_tracers` has placed it.
                Visibility::Hidden,
                ChildOf(event.entity),
            ));
        }
    }
}

/// What plays its own death animation: an enemy, or a turret.
type AnimatedDeath = Or<(With<bft_sim::ai::Enemy>, With<bft_sim::ai::TurretAi>)>;

/// A fireball and a plume where something died with no animation to show it.
///
/// An enemy plays its kind's death animation and a turret its
/// `DESTROY_ANIM`, and a weapon with a `KILL_ANIMATION` (`kill_mortar`, …)
/// plays that on the victim; drawing this fireball over them as well was
/// what made a kill look wrong. It stays for whatever dies with none.
fn destruction_effects(
    mut destroyed: MessageReader<Destroyed>,
    mut effects: MessageWriter<SpawnEffect>,
    bounds: Query<&ColliderAabb>,
    animated: Query<(), AnimatedDeath>,
    table: Option<Res<WeaponTable>>,
) {
    for event in destroyed.read() {
        let kill_animation = event
            .weapon
            .and_then(|weapon| table.as_ref()?.by_index(weapon))
            .and_then(|def| def.impact.get(&Surface::Default))
            .is_some_and(|impact| impact.kill_animation.is_some());
        if kill_animation || animated.contains(event.entity) {
            continue;
        }
        let radius = bounds
            .get(event.entity)
            .map(|aabb| ((aabb.max - aabb.min) * 0.5).max_element())
            .unwrap_or(2.0);
        let scale = (radius / 3.0).clamp(0.5, 3.0);
        for kind in [EffectKind::Explosion, EffectKind::Smoke] {
            effects.write(SpawnEffect {
                kind,
                at: event.at,
                normal: None,
                scale,
                attached: None,
            });
        }
    }
}

/// The `Effect` outputs — anim `Effect` events and weapon `IMPACT/EFFECT`
/// keys — drawn as the game's own sprite effects.
///
/// Each `effects.zrd` record opens with the model it plays on: `expbunk.flt`
/// for `exp_bunk`, `smoke1.flt` for `smoke1`. The level carries each as a
/// prototype, so the effect is a clone of it placed at the event, running
/// its record's cycle, and gone when the cycle has played — or, for
/// `fire1`, the one that loops, after [`FIRE_SECONDS`]. The cycle is set
/// from the record by name rather than looked up by texture: `boxexp.flt`
/// and `bftmbrst.flt` open on the same frame.
///
/// A name the level has no model for (the two splash cycles, whose models
/// no level carries) falls back to a placeholder [`SpawnEffect`].
#[allow(clippy::too_many_arguments)]
fn anim_effects(
    mut commands: Commands,
    mut outputs: MessageReader<AnimOutput>,
    mut effects: MessageWriter<SpawnEffect>,
    cycles: Option<Res<EffectTextureCycles>>,
    loaded: Option<Res<LoadedLevel>>,
    levels: Res<Assets<LevelData>>,
    axes: Option<Res<AxisConvention>>,
    pool: Res<TexturePool>,
    sets: Res<Assets<TextureSet>>,
    exists: Query<(), With<GlobalTransform>>,
    mut index: Local<Option<(AssetId<LevelData>, LevelTemplates)>>,
) {
    let level = loaded
        .as_ref()
        .and_then(|loaded| Some((loaded.data.id(), levels.get(&loaded.data)?)));
    if let Some((id, level)) = level
        && index.as_ref().is_none_or(|(built_for, _)| *built_for != id)
    {
        *index = Some((id, LevelTemplates::from_level(level)));
    }
    let axes = axes
        .map(|axes| *axes)
        .unwrap_or(AxisConvention::GAMEZ_DEFAULT);

    for output in outputs.read() {
        if output.kind != "Effect" {
            continue;
        }
        let Some(name) = output.value.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let Some(node) = output.node.or(output.at) else {
            continue;
        };
        if !exists.contains(node) {
            continue;
        }
        let offset = output
            .value
            .get("at_node")
            .and_then(|at| at.get("pos"))
            .map(|pos| {
                let axis = |k: &str| pos.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                Vec3::new(axis("x"), axis("y"), axis("z"))
            })
            .unwrap_or(Vec3::ZERO);

        let record = cycles.as_ref().and_then(|cycles| cycles.0.by_name(name));
        let template = level
            .map(|(_, level)| level)
            .zip(index.as_ref())
            .zip(record)
            .and_then(|((level, (_, index)), record)| {
                Some((level, index.root(&record.model)?, record))
            });

        let Some((level, root, record)) = template else {
            effects.write(SpawnEffect {
                kind: effect_kind_for_name(name),
                at: offset,
                normal: None,
                scale: 1.0,
                attached: Some(node),
            });
            continue;
        };

        let cloned = templates::clone_template(
            &mut commands,
            level,
            &axes,
            root,
            Transform::from_translation(offset),
            &format!("effect {name}"),
        );
        let lifetime = if record.looping {
            FIRE_SECONDS
        } else if record.speed > 0.0 {
            record.maps.len() as f32 / record.speed
        } else {
            FIRE_SECONDS
        };
        commands
            .entity(cloned.root)
            .insert((EffectSprite { age: 0.0, lifetime }, ChildOf(node)));

        let frames: Vec<Handle<Image>> = record
            .maps
            .iter()
            .filter_map(|frame| pool.image(frame, &sets))
            .collect();
        if !frames.is_empty() {
            let cycle = CycledTexture::new(frames, record.speed, record.looping);
            for (_, entity) in &cloned.nodes {
                commands.entity(*entity).insert(cycle.clone());
            }
        }
    }
}

/// An `effects.zrd` sprite effect: a clone of its model, living as long as
/// its cycle plays.
#[derive(Component, Debug, Clone, Copy)]
pub struct EffectSprite {
    pub age: f32,
    pub lifetime: f32,
}

/// Retire each sprite effect once its cycle has played.
fn expire_effect_sprites(
    mut commands: Commands,
    time: Res<Time>,
    mut sprites: Query<(Entity, &mut EffectSprite)>,
) {
    for (entity, mut sprite) in &mut sprites {
        sprite.age += time.delta_secs();
        if sprite.age >= sprite.lifetime {
            commands.entity(entity).despawn();
        }
    }
}

/// Turn requests into billboards.
fn spawn_effects(
    mut commands: Commands,
    mut requests: MessageReader<SpawnEffect>,
    sprites: Option<Res<EffectSprites>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    exists: Query<(), With<GlobalTransform>>,
    burning: Query<(&EffectMotion, &Billboard, &ChildOf)>,
) {
    let Some(sprites) = sprites else {
        requests.clear();
        return;
    };

    for request in requests.read() {
        let look = look(request.kind);
        let (frames, fps) = sprites.frames(request.kind);

        if let Some(parent) = request.attached {
            if exists.get(parent).is_err() {
                continue;
            }
            // One flame per node: a definition that re-runs does not stack
            // them.
            if request.kind == EffectKind::Fire
                && burning.iter().any(|(_, billboard, child_of)| {
                    child_of.parent() == parent && billboard.looping
                })
            {
                continue;
            }
        }

        let lift = request.normal.map_or(Vec3::ZERO, |n| n * look.size * 0.25);
        let size = look.size * request.scale.max(0.01);
        let material = materials.add(StandardMaterial {
            base_color: look.color,
            base_color_texture: frames.first().cloned(),
            unlit: true,
            alpha_mode: if look.additive {
                AlphaMode::Add
            } else {
                AlphaMode::Blend
            },
            cull_mode: None,
            // Effects are light and smoke; fog would grey out a fireball.
            fog_enabled: false,
            ..default()
        });

        let mut entity = commands.spawn((
            Name::new(format!("effect {:?}", request.kind)),
            Billboard {
                frames: frames.to_vec(),
                fps,
                age: 0.0,
                lifetime: look.lifetime,
                looping: request.kind.loops(),
            },
            EffectMotion {
                size,
                grow: look.grow,
                rise: look.rise,
                color: look.color,
                shown: 0,
            },
            Mesh3d(sprites.quad.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(request.at + lift).with_scale(Vec3::splat(size)),
            DespawnOnExit(AppState::InGame),
        ));
        if let Some(parent) = request.attached {
            entity.insert(ChildOf(parent));
        }

        if request.kind == EffectKind::Nuke {
            spawn_nuke_flash(&mut commands);
        }
    }
}

/// Age every billboard: step its frame, grow it, lift it, fade it, and
/// despawn it when done.
fn animate_billboards(
    mut commands: Commands,
    time: Res<Time>,
    mut billboards: Query<(
        Entity,
        &mut Billboard,
        &mut EffectMotion,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut beams: Query<(Entity, &mut BeamFlash)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let dt = time.delta_secs();

    for (entity, mut billboard, mut motion, mut transform, material) in &mut billboards {
        billboard.age += dt;
        if billboard.finished() {
            commands.entity(entity).despawn();
            continue;
        }

        let life = if billboard.lifetime.is_finite() {
            (billboard.age / billboard.lifetime).clamp(0.0, 1.0)
        } else {
            0.0
        };
        transform.scale = Vec3::splat(motion.size * (1.0 + motion.grow * life));
        transform.translation.y += motion.rise * dt;

        let frame = billboard.frame();
        let opacity = billboard.opacity();
        if let Some(mut material) = materials.get_mut(&material.0) {
            if frame != motion.shown {
                material.base_color_texture = billboard.frames.get(frame).cloned();
                motion.shown = frame;
            }
            let base = motion.color.to_srgba();
            material.base_color = if matches!(material.alpha_mode, AlphaMode::Add) {
                // Additive has no alpha; darken toward black instead.
                Color::srgb(
                    base.red * opacity,
                    base.green * opacity,
                    base.blue * opacity,
                )
            } else {
                Color::srgba(base.red, base.green, base.blue, base.alpha * opacity)
            };
        }
    }

    for (entity, mut beam) in &mut beams {
        beam.age += dt;
        if beam.age >= BEAM_SECONDS {
            commands.entity(entity).despawn();
        }
    }
}

/// Turn every billboard to face the camera.
///
/// Screen-aligned: each takes the camera's rotation, corrected for its
/// parent's where it hangs off a node.
fn face_billboards(
    camera: Query<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>,
    mut billboards: Query<(&mut Transform, Option<&ChildOf>), With<Billboard>>,
    parents: Query<&GlobalTransform>,
) {
    let Some(camera) = camera.iter().next() else {
        return;
    };
    let facing = camera.rotation();
    for (mut transform, child_of) in &mut billboards {
        let parent = child_of
            .and_then(|c| parents.get(c.parent()).ok())
            .map_or(Quat::IDENTITY, GlobalTransform::rotation);
        transform.rotation = parent.inverse() * facing;
    }
}

/// Stretch each tracer along its projectile's velocity.
fn steer_tracers(
    camera: Query<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>,
    projectiles: Query<(&Projectile, &Transform), Without<Tracer>>,
    mut tracers: Query<(&mut Transform, &mut Visibility, &ChildOf), With<Tracer>>,
) {
    let eye = camera.iter().next().map(GlobalTransform::translation);

    for (mut transform, mut visibility, child_of) in &mut tracers {
        let Ok((projectile, parent)) = projectiles.get(child_of.parent()) else {
            continue;
        };
        let speed = projectile.velocity.length();
        if speed < 1.0 {
            *visibility = Visibility::Hidden;
            continue;
        }
        let length = (speed * TRACER_SECONDS).clamp(1.5, 12.0);
        let tail = parent.translation - projectile.velocity / speed * length;
        let world = streak(tail, parent.translation, eye, TRACER_WIDTH);

        // The tracer is a child; express the world pose in the parent's
        // frame. Projectiles are unscaled roots, so rotation and
        // translation are all there is to undo.
        let inverse = parent.rotation.inverse();
        transform.translation = inverse * (world.translation - parent.translation);
        transform.rotation = inverse * world.rotation;
        transform.scale = world.scale;
        *visibility = Visibility::Inherited;
    }
}

/// A quad from `from` to `to`, `width` wide, turned about its length to face
/// `eye` where there is one.
pub fn streak(from: Vec3, to: Vec3, eye: Option<Vec3>, width: f32) -> Transform {
    let along = to - from;
    let length = along.length();
    let axis = if length > 1e-4 {
        along / length
    } else {
        Vec3::Y
    };
    let middle = (from + to) * 0.5;

    // The quad's normal (+Z) points as close to the eye as the axis allows.
    let toward = eye.map_or(Vec3::Y, |eye| eye - middle);
    let mut normal = toward - axis * toward.dot(axis);
    if normal.length_squared() < 1e-6 {
        normal = axis.any_orthonormal_vector();
    }
    let normal = normal.normalize();
    let side = axis.cross(normal);

    Transform {
        translation: middle,
        rotation: Quat::from_mat3(&Mat3::from_cols(side, axis, normal)),
        scale: Vec3::new(width, length.max(0.01), 1.0),
    }
}

/// Fade the nuke's white-out.
fn fade_flashes(
    mut commands: Commands,
    time: Res<Time>,
    mut flashes: Query<(Entity, &mut NukeFlash, &mut BackgroundColor)>,
) {
    for (entity, mut flash, mut color) in &mut flashes {
        flash.age += time.delta_secs();
        let left = 1.0 - flash.age / flash.duration;
        if left <= 0.0 {
            commands.entity(entity).despawn();
        } else {
            color.0 = Color::srgba(1.0, 1.0, 1.0, left.clamp(0.0, 1.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn billboard(frames: usize, fps: f32, lifetime: f32) -> Billboard {
        Billboard {
            frames: vec![Handle::default(); frames],
            fps,
            age: 0.0,
            lifetime,
            looping: false,
        }
    }

    #[test]
    fn a_billboard_steps_frames_at_its_rate_and_holds_the_last() {
        let mut b = billboard(8, 10.0, 2.0);
        assert_eq!(b.frame(), 0);
        b.age = 0.099;
        assert_eq!(b.frame(), 0);
        b.age = 0.1;
        assert_eq!(b.frame(), 1);
        b.age = 0.75;
        assert_eq!(b.frame(), 7);
        b.age = 1.9;
        assert_eq!(b.frame(), 7, "holds the last frame");
        assert!(!b.finished());
        b.age = 2.0;
        assert!(b.finished());
    }

    #[test]
    fn a_looping_billboard_wraps_until_its_life_is_up() {
        let mut b = Billboard {
            looping: true,
            ..billboard(6, 12.0, 4.0)
        };
        b.age = 0.5;
        assert_eq!(b.frame(), 0, "6 frames at 12 fps wrap at 0.5 s");
        b.age = 0.6;
        assert_eq!(b.frame(), 1);
        b.age = 3.0;
        assert!(b.opacity() < 1.0, "and dies down over its second half");
        assert!(!b.finished());
        b.age = 4.0;
        assert!(b.finished());
    }

    /// A destroyed prop's or a dead enemy's flame goes out by itself, rather
    /// than burning on for as long as the node it was placed on stays up.
    #[test]
    fn a_flame_is_the_one_kind_that_loops_and_it_still_goes_out() {
        let fire = look(EffectKind::Fire);
        assert!(fire.lifetime.is_finite() && fire.lifetime > 0.0);
        assert!(EffectKind::Fire.loops());
        for kind in [
            EffectKind::MuzzleFlash,
            EffectKind::Impact,
            EffectKind::Explosion,
            EffectKind::Smoke,
            EffectKind::Splash,
            EffectKind::Nuke,
        ] {
            assert!(!kind.loops(), "{kind:?}");
            assert!(look(kind).lifetime.is_finite(), "{kind:?}");
        }
    }

    #[test]
    fn a_billboard_is_opaque_then_fades_out() {
        let mut b = billboard(1, 1.0, 2.0);
        b.age = 0.9;
        assert_eq!(b.opacity(), 1.0);
        b.age = 1.5;
        assert!((b.opacity() - 0.5).abs() < 1e-6);
        b.age = 2.0;
        assert_eq!(b.opacity(), 0.0);
    }

    #[test]
    fn the_nine_anim_effect_names_map_to_sensible_kinds() {
        use EffectKind::*;
        for (name, kind) in [
            ("exp_crate", Explosion),
            ("fire1", Fire),
            ("videxp", Explosion),
            ("exp_bunk", Explosion),
            ("smoke1", Smoke),
            ("exp_box", Explosion),
            ("exp_fireup", Explosion),
            ("watblast", Splash),
            ("splash_dam", Splash),
            ("something_new", Impact),
        ] {
            assert_eq!(effect_kind_for_name(name), kind, "{name}");
        }
    }

    #[test]
    fn a_streak_spans_its_ends_and_faces_the_eye() {
        let from = Vec3::new(0.0, 0.0, 0.0);
        let to = Vec3::new(10.0, 0.0, 0.0);
        let eye = Vec3::new(5.0, 0.0, 20.0);
        let t = streak(from, to, Some(eye), 0.5);

        assert!((t.translation - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5);
        assert!((t.scale.y - 10.0).abs() < 1e-5);
        // The quad's local Y runs along the streak …
        assert!((t.rotation * Vec3::Y - Vec3::X).length() < 1e-5);
        // … and its face (+Z) toward the eye.
        assert!((t.rotation * Vec3::Z - Vec3::Z).length() < 1e-5);
    }

    #[test]
    fn a_nuke_is_the_biggest_thing_on_screen() {
        let nuke = look(EffectKind::Nuke).size;
        for kind in [
            EffectKind::MuzzleFlash,
            EffectKind::Impact,
            EffectKind::Explosion,
            EffectKind::Smoke,
            EffectKind::Fire,
            EffectKind::Splash,
        ] {
            assert!(look(kind).size < nuke, "{kind:?}");
        }
    }

    /// A level with one effect prototype, `fx.flt`, whose model shows
    /// `wall.tif`, and the World.
    fn effect_level() -> LevelData {
        use bft_assets::test_support::{MINIMAL_MODEL, WORLD_NODE, gamez_from};

        let nodes = format!(
            "[{WORLD_NODE},{},{},{}]",
            r#"{ "name": "group", "zone_id": 3, "model_index": -1,
                 "parent_indices": [0], "child_indices": [],
                 "flags": { "active": true, "tree_valid": true },
                 "data": { "Object3d": { "transform": "Initial", "signs": 128 } } }"#,
            r#"{ "name": "fx.flt", "zone_id": 3, "model_index": -1,
                 "parent_indices": [], "child_indices": [3],
                 "flags": { "active": true, "tree_valid": true },
                 "data": { "Object3d": { "transform": "Initial", "signs": 128 } } }"#,
            r#"{ "name": "blast", "zone_id": 3, "model_index": 0,
                 "parent_indices": [2], "child_indices": [],
                 "flags": { "active": true, "tree_valid": true },
                 "data": { "Object3d": { "transform": "Initial", "signs": 128 } } }"#
        );
        let bytes = gamez_from(
            r#"[{ "Textured": { "texture_index": 0, "soil": "Default", "cycle": null, "flag": false } }]"#,
            r#"[{ "name": "wall.tif" }]"#,
            MINIMAL_MODEL,
            &nodes,
        );
        bft_assets::level::parse_gamez(&bytes, "fx.gamez.zip").expect("the fixture loads")
    }

    /// An `Effect` naming an `effects.zrd` record plays a copy of the
    /// record's model where it was placed, running the record's cycle, and
    /// the copy is gone once the cycle has played.
    #[test]
    fn an_effect_plays_its_own_model_and_goes_when_its_cycle_ends() {
        use bft_assets::reader::effects::{EffectCycle, EffectCycles};

        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::transform::TransformPlugin,
        ));
        app.init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<StandardMaterial>()
            .init_asset::<LevelData>()
            .init_asset::<TextureSet>()
            .init_resource::<crate::mesh_builder::MeshCache>()
            .init_resource::<crate::materials::MaterialCache>()
            .init_resource::<TexturePool>()
            .init_resource::<Time>()
            .insert_resource(AxisConvention::GAMEZ_DEFAULT)
            .add_message::<AnimOutput>()
            .add_message::<SpawnEffect>()
            .add_observer(crate::level_view::attach_model)
            .add_systems(Update, (anim_effects, expire_effect_sprites).chain());

        let frames = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            [images.add(Image::default()), images.add(Image::default())]
        };
        let info = bft_assets::textures::TextureInfo {
            width: 1,
            height: 1,
            alpha: bft_assets::textures::TextureAlpha::None,
            stretch: bft_assets::textures::TextureStretch::None,
        };
        let set = TextureSet {
            images: [
                ("wall".to_string(), frames[0].clone()),
                ("wall2".to_string(), frames[1].clone()),
            ]
            .into(),
            info: [("wall".to_string(), info), ("wall2".to_string(), info)].into(),
        };
        let set = app
            .world_mut()
            .resource_mut::<Assets<TextureSet>>()
            .add(set);
        app.world_mut().resource_mut::<TexturePool>().push(set);
        app.insert_resource(EffectTextureCycles(EffectCycles {
            cycles: vec![EffectCycle {
                name: "exp_test".into(),
                model: "fx.flt".into(),
                speed: 4.0,
                looping: false,
                maps: vec!["wall.tif".into(), "wall2.tif".into()],
            }],
        }));
        let level = app
            .world_mut()
            .resource_mut::<Assets<LevelData>>()
            .add(effect_level());
        app.insert_resource(LoadedLevel::loading(
            bft_assets::catalog::LevelId(1),
            level,
            Vec::new(),
            None,
            None,
            None,
            None,
        ));

        let at = app.world_mut().spawn(Transform::default()).id();
        app.world_mut()
            .resource_mut::<Messages<AnimOutput>>()
            .write(AnimOutput {
                kind: "Effect",
                value: serde_json::json!({
                    "name": "exp_test",
                    "at_node": { "pos": { "x": 1.0, "y": 2.0, "z": 3.0 } },
                }),
                def: "test".into(),
                at: Some(at),
                node: Some(at),
            });
        app.update();

        let mut sprites = app
            .world_mut()
            .query::<(Entity, &EffectSprite, &Transform, &ChildOf)>();
        let (copy, sprite, transform, parent) = sprites
            .single(app.world())
            .expect("one sprite effect plays");
        assert_eq!(parent.parent(), at);
        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, 3.0));
        assert!((sprite.lifetime - 0.5).abs() < 1e-6, "two frames at 4 fps");

        let blast = app
            .world()
            .get::<Children>(copy)
            .and_then(|kids| kids.first().copied())
            .expect("the model node");
        let cycle = app
            .world()
            .get::<CycledTexture>(blast)
            .expect("running the record's cycle");
        assert_eq!(cycle.frames, frames.to_vec());
        assert_eq!(
            app.world().resource::<Messages<SpawnEffect>>().len(),
            0,
            "and no placeholder besides"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.6));
        app.update();
        assert!(app.world().get_entity(copy).is_err(), "gone once played");
    }
}
