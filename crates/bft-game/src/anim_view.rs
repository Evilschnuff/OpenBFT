//! What the anim handlers leave in the sim, drawn.
//!
//! The sim keeps the state — [`AnimOpacity`] on nodes, the definitions'
//! lights in [`AnimLights`], the zoom in [`AnimCamera`], the screen tint in
//! [`AnimScreenFx`], the fog in `WorldEnv` — and never touches a renderer
//! (NFR-4). This module is the other half:
//!
//! - opacity becomes material alpha, on per-node copies of the materials;
//! - an effect template the VM placed somewhere is shown (`bft-game` hides
//!   template roots, [`crate::enemies::hide_template_root`]);
//! - the lights become `PointLight`s on the nodes they ride;
//! - the zoom narrows or widens the 3d camera's field of view, and the clip
//!   planes follow `CameraState`;
//! - the tint is a full-screen quad over everything;
//! - `ObjectCycleTexture` rewinds a node's texture cycle.
//!
//! Fog needs nothing here: `FogState` changes `WorldEnv`, and
//! [`apply_world_env`](crate::level_view::apply_world_env) already follows it.

use std::collections::HashMap;

use bevy::prelude::*;
use bft_sim::anim::{AnimCamera, AnimLights, AnimOpacity, AnimOutput, AnimPlaced, AnimScreenFx};
use bft_sim::level::NodeActive;

use crate::level_view::POINT_LIGHT_INTENSITY;
use crate::materials::CycledTexture;

/// The anim presentation systems.
pub struct AnimViewPlugin;

impl Plugin for AnimViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnimLightEntities>().add_systems(
            Update,
            (
                show_placed_templates,
                apply_anim_opacity,
                sync_anim_lights,
                apply_anim_camera,
                draw_screen_tint,
                rewind_texture_cycles,
            ),
        );
    }
}

/// Show a template the VM has placed somewhere.
///
/// `Visible` rather than `Inherited`, because the template's root — the
/// node itself or an ancestor — is hidden, and a placed effect has to show
/// through that. [`crate::level_view::mirror_node_visibility`] keeps it so.
pub fn show_placed_templates(mut placed: Query<(&NodeActive, &mut Visibility), Added<AnimPlaced>>) {
    for (active, mut visibility) in &mut placed {
        *visibility = if active.0 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// A mesh whose material this module has made its own, and the alpha the
/// shared original had.
#[derive(Component, Debug, Clone, Copy)]
pub struct FadedMaterial {
    pub base_alpha: f32,
    pub base_mode: AlphaMode,
}

/// Fade a node's meshes to its [`AnimOpacity`].
///
/// Level materials are shared between every node that uses them, so the
/// first fade gives each mesh its own copy — otherwise one fading smoke puff
/// fades every surface with the same texture.
pub fn apply_anim_opacity(
    mut commands: Commands,
    changed: Query<(Entity, &AnimOpacity), Changed<AnimOpacity>>,
    mut removed: RemovedComponents<AnimOpacity>,
    children: Query<&Children>,
    opacities: Query<(), With<AnimOpacity>>,
    mut meshes: Query<(
        &mut MeshMaterial3d<StandardMaterial>,
        Option<&FadedMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut targets: Vec<(Entity, Option<f32>)> = changed
        .iter()
        .map(|(entity, opacity)| (entity, Some(opacity.0)))
        .collect();
    targets.extend(removed.read().map(|entity| (entity, None)));

    for (node, opacity) in targets {
        let mut stack = vec![node];
        while let Some(entity) = stack.pop() {
            if let Ok((mut handle, faded)) = meshes.get_mut(entity) {
                let (base_alpha, base_mode) = match faded {
                    Some(faded) => (faded.base_alpha, faded.base_mode),
                    None => {
                        let Some(original) = materials.get(&handle.0).cloned() else {
                            continue;
                        };
                        let base = (original.base_color.alpha(), original.alpha_mode);
                        handle.0 = materials.add(original);
                        commands.entity(entity).insert(FadedMaterial {
                            base_alpha: base.0,
                            base_mode: base.1,
                        });
                        base
                    }
                };
                if let Some(mut material) = materials.get_mut(&handle.0) {
                    let alpha = base_alpha * opacity.unwrap_or(1.0);
                    material.base_color.set_alpha(alpha);
                    material.alpha_mode = if alpha < 1.0 && base_mode == AlphaMode::Opaque {
                        AlphaMode::Blend
                    } else {
                        base_mode
                    };
                }
            }
            if let Ok(kids) = children.get(entity) {
                // A child node with its own opacity fades by that instead.
                stack.extend(kids.iter().filter(|kid| opacities.get(*kid).is_err()));
            }
        }
    }
}

/// The `PointLight` entity spawned for each definition light.
#[derive(Resource, Debug, Default)]
pub struct AnimLightEntities(HashMap<(Entity, String), Entity>);

/// Mirror [`AnimLights`] onto `PointLight`s.
///
/// A light rides the node its `LightState` put it at; one with no node is
/// not drawn, since there is nowhere meaningful to put it.
pub fn sync_anim_lights(
    mut commands: Commands,
    lights: Res<AnimLights>,
    mut spawned: ResMut<AnimLightEntities>,
    mut existing: Query<(&mut PointLight, &mut Visibility, &mut Transform)>,
    parents: Query<&ChildOf>,
    alive: Query<(), With<GlobalTransform>>,
) {
    if !lights.is_changed() {
        return;
    }

    // Lights the sim has forgotten (a new level) or whose node went with
    // the level.
    spawned.0.retain(|key, entity| {
        let keep = lights.0.contains_key(key) && existing.contains(*entity);
        if !keep && alive.contains(*entity) {
            commands.entity(*entity).despawn();
        }
        keep
    });

    for (key, light) in &lights.0 {
        let Some((node, offset)) = light.at else {
            continue;
        };
        if !alive.contains(node) {
            continue;
        }
        let point = PointLight {
            color: light.light.color,
            intensity: POINT_LIGHT_INTENSITY,
            range: light.light.far.max(0.1),
            shadow_maps_enabled: false,
            ..default()
        };
        let visibility = if light.active {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };

        match spawned.0.get(key).copied() {
            Some(entity) => {
                if let Ok((mut current, mut shown, mut transform)) = existing.get_mut(entity) {
                    *current = point;
                    *shown = visibility;
                    transform.translation = offset;
                }
                // A later `LightState` may move the light to another node.
                if parents.get(entity).map(ChildOf::parent).ok() != Some(node) {
                    commands.entity(entity).insert(ChildOf(node));
                }
            }
            None => {
                let entity = commands
                    .spawn((
                        Name::new(format!("anim light {}", key.1)),
                        point,
                        visibility,
                        Transform::from_translation(offset),
                        ChildOf(node),
                    ))
                    .id();
                spawned.0.insert(key.clone(), entity);
            }
        }
    }
}

/// The field of view a camera had before any zoom.
#[derive(Component, Debug, Clone, Copy)]
pub struct UnzoomedFov(pub f32);

/// Apply [`AnimCamera`] to every perspective 3d camera.
///
/// Zoom divides the tangent of the half-angle, so 0.85 — `bft_00`'s death
/// camera — widens the view by about fifteen per cent.
pub fn apply_anim_camera(
    mut commands: Commands,
    camera: Res<AnimCamera>,
    mut cameras: Query<
        (Entity, &mut Projection, Option<&UnzoomedFov>),
        crate::ui::hud::sensor::MainCamera3d,
    >,
) {
    for (entity, mut projection, unzoomed) in &mut cameras {
        let Projection::Perspective(perspective) = projection.as_mut() else {
            continue;
        };
        let base = match unzoomed {
            Some(base) => base.0,
            None => {
                commands.entity(entity).insert(UnzoomedFov(perspective.fov));
                perspective.fov
            }
        };
        if !camera.is_changed() && unzoomed.is_some() {
            continue;
        }
        let zoom = camera.zoom.y.max(0.05);
        perspective.fov = 2.0 * ((base * 0.5).tan() / zoom).atan();
        if let Some(near) = camera.clip_near {
            perspective.near = near.max(0.01);
        }
        if let Some(far) = camera.clip_far {
            perspective.far = far.max(perspective.near + 1.0);
        }
    }
}

/// The full-screen quad [`AnimScreenFx`]'s tint is drawn with.
#[derive(Component, Debug)]
pub struct ScreenTint;

/// Draw the screen tint, or hide it.
pub fn draw_screen_tint(
    mut commands: Commands,
    screen: Res<AnimScreenFx>,
    mut quads: Query<(&mut BackgroundColor, &mut Visibility), With<ScreenTint>>,
) {
    if !screen.is_changed() {
        return;
    }
    let (color, visibility) = match screen.tint {
        Some(tint) => (Color::LinearRgba(tint), Visibility::Visible),
        None => (Color::NONE, Visibility::Hidden),
    };

    match quads.single_mut() {
        Ok((mut background, mut shown)) => {
            background.0 = color;
            *shown = visibility;
        }
        Err(_) if screen.tint.is_some() => {
            commands.spawn((
                Name::new("anim screen tint"),
                ScreenTint,
                Node {
                    position_type: PositionType::Absolute,
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                BackgroundColor(color),
                // Over the world and the HUD alike, as the original's frame
                // buffer effect was; under the nuke's own flash.
                GlobalZIndex(i32::MAX - 2),
                visibility,
            ));
        }
        Err(_) => {}
    }
}

/// `ObjectCycleTexture`: put a node's texture cycle back on the given frame.
pub fn rewind_texture_cycles(
    mut outputs: MessageReader<AnimOutput>,
    children: Query<&Children>,
    mut cycles: Query<&mut CycledTexture>,
) {
    for output in outputs.read() {
        if output.kind != "ObjectCycleTexture" {
            continue;
        }
        let Some(node) = output.node else {
            continue;
        };
        let frame = output
            .value
            .get("reset")
            .and_then(|reset| reset.as_u64())
            .unwrap_or(0) as usize;

        let mut stack = vec![node];
        while let Some(entity) = stack.pop() {
            if let Ok(mut cycle) = cycles.get_mut(entity) {
                cycle.frame = frame.min(cycle.frames.len().saturating_sub(1));
                cycle.acc = 0.0;
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

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
        ));
        app.init_asset::<StandardMaterial>()
            .init_resource::<AnimCamera>()
            .init_resource::<AnimScreenFx>()
            .init_resource::<AnimLights>()
            .add_message::<AnimOutput>();
        app
    }

    /// Two nodes sharing one material: fading one leaves the other opaque.
    #[test]
    fn opacity_fades_a_private_copy_of_the_material() {
        let mut app = app();
        app.add_systems(Update, apply_anim_opacity);
        let shared = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let faded = app.world_mut().spawn(AnimOpacity(0.25)).id();
        let mesh = app
            .world_mut()
            .spawn((MeshMaterial3d(shared.clone()), ChildOf(faded)))
            .id();
        let other = app.world_mut().spawn(MeshMaterial3d(shared.clone())).id();

        app.update();

        let materials = app.world().resource::<Assets<StandardMaterial>>();
        let mine = &app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(mesh)
            .expect("mesh")
            .0;
        assert_ne!(*mine, shared, "the fading mesh has its own material");
        let mine = materials.get(mine).expect("material");
        assert!((mine.base_color.alpha() - 0.25).abs() < 1e-6);
        assert_eq!(mine.alpha_mode, AlphaMode::Blend);
        let theirs = &app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(other)
            .expect("mesh")
            .0;
        assert_eq!(
            materials.get(theirs).expect("material").base_color.alpha(),
            1.0
        );
    }

    /// Zoom 0.85 widens a 60° view; 1 leaves it alone.
    #[test]
    fn zoom_widens_the_field_of_view() {
        let mut app = app();
        app.add_systems(Update, apply_anim_camera);
        let base = 60f32.to_radians();
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Projection::Perspective(PerspectiveProjection {
                    fov: base,
                    ..default()
                }),
            ))
            .id();

        app.update();
        app.world_mut().resource_mut::<AnimCamera>().zoom = Vec2::splat(0.85);
        app.update();

        let Some(Projection::Perspective(perspective)) = app.world().get::<Projection>(camera)
        else {
            panic!("a perspective camera");
        };
        let expected = 2.0 * ((base * 0.5).tan() / 0.85).atan();
        assert!((perspective.fov - expected).abs() < 1e-5);
        assert!(perspective.fov > base);
    }

    /// A tint puts a full-screen quad up; clearing it hides the quad.
    #[test]
    fn the_screen_tint_shows_and_hides() {
        let mut app = app();
        app.add_systems(Update, draw_screen_tint);
        app.world_mut().resource_mut::<AnimScreenFx>().tint =
            Some(LinearRgba::new(1.0, 1.0, 1.0, 0.9));
        app.update();

        let mut quads = app
            .world_mut()
            .query_filtered::<(&BackgroundColor, &Visibility), With<ScreenTint>>();
        let (color, visibility) = quads.single(app.world()).expect("one quad");
        assert_eq!(*visibility, Visibility::Visible);
        assert!((color.0.alpha() - 0.9).abs() < 1e-6);

        app.world_mut().resource_mut::<AnimScreenFx>().tint = None;
        app.update();
        let (_, visibility) = quads.single(app.world()).expect("one quad");
        assert_eq!(*visibility, Visibility::Hidden);
    }

    /// A placed template shows through its hidden root.
    #[test]
    fn a_placed_template_is_visible() {
        let mut app = app();
        app.add_systems(Update, show_placed_templates);
        let placed = app
            .world_mut()
            .spawn((NodeActive(true), Visibility::Hidden, AnimPlaced))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(placed),
            Some(&Visibility::Visible)
        );
    }

    /// `ObjectCycleTexture` rewinds the cycle on the node.
    #[test]
    fn a_texture_cycle_is_rewound() {
        let mut app = app();
        app.add_systems(Update, rewind_texture_cycles);
        let mut cycle = CycledTexture::new(vec![Handle::default(); 4], 10.0, true);
        cycle.frame = 3;
        let node = app.world_mut().spawn(cycle).id();
        app.world_mut()
            .resource_mut::<Messages<AnimOutput>>()
            .write(AnimOutput {
                kind: "ObjectCycleTexture",
                value: serde_json::json!({ "name": "wakepoly", "reset": 0 }),
                def: "boat_wake".to_string(),
                at: None,
                node: Some(node),
            });
        app.update();
        assert_eq!(
            app.world().get::<CycledTexture>(node).expect("cycle").frame,
            0
        );
    }
}
