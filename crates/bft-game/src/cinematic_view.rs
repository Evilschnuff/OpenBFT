//! The cinematics' presentation: the view during a cinematic, the skip key, and
//! handing the view back.
//!
//! While [`PlayerControl`] is `Cinematic` the play camera looks through the
//! level's `camera1`, which the scripts fly with SI motion scripts:
//!
//! - the chase camera is placed on `camera1`'s world transform every frame,
//!   after propagation, so the frame shows this tick's pose. `camera1`'s
//!   **−Z is its forward**, Bevy's camera convention, so no flip is needed:
//!   over all six intros it points −Z at the VTOL (or m1's truck) in 228 of
//!   229 half-second samples and +Z in none (docs/data/anims.md).
//!   `AnimCamera`'s zoom already reaches every perspective camera
//!   (`anim_view::apply_anim_camera`);
//! - the HUD is hidden (`ui::hud::show_hud`) and gameplay input is off
//!   ([`gameplay_input_enabled`](crate::ui::gameplay_input_enabled)); the sim
//!   zeroes the player's input as well;
//! - during an intro, Esc, Space, Enter or a click sends [`SkipCinematic`].
//!   Esc there skips rather than pausing.
//!
//! When control returns, the chase camera snaps behind the hull rather than
//! swinging round from wherever `camera1` left it. It snaps the same way
//! when a script moves the player (`Callback 17`: elevators, teleports).

use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bft_sim::anim::AnimProgram;
use bft_sim::cinematic::{PlayerControl, PlayerTeleported, SkipCinematic, is_skippable};
use bft_sim::level::NodeIndex;
use bft_sim::states::AppState;

use crate::chase_camera::ChaseCamera;
use crate::devtools::free_camera::FreeCameraActive;
use crate::ui::{Action, ActionInput, PauseMenuOpen};

/// The node the scripts fly as the cinematic camera.
pub const CINEMATIC_CAMERA_NODE: &str = "camera1";

/// The cinematics' presentation systems.
pub struct CinematicViewPlugin;

impl Plugin for CinematicViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (skip_on_key, snap_chase_camera).run_if(in_state(AppState::InGame)),
        )
        .add_systems(
            PostUpdate,
            look_through_camera1
                .after(TransformSystems::Propagate)
                .run_if(in_state(AppState::InGame)),
        );
    }
}

/// Place the play camera on `camera1` while a cinematic runs.
///
/// In `PostUpdate`, after propagation, and writing the camera's
/// `GlobalTransform` as well as its `Transform`: the chase camera's own
/// `Update` system has already placed it behind the hull this frame, and
/// only a write after propagation reaches the frame being drawn.
pub fn look_through_camera1(
    control: Res<PlayerControl>,
    nodes: Option<Res<NodeIndex>>,
    globals: Query<&GlobalTransform, Without<ChaseCamera>>,
    mut cameras: Query<(&mut Transform, &mut GlobalTransform), With<ChaseCamera>>,
) {
    if !control.is_cinematic() {
        return;
    }
    let Some(eye) = nodes
        .as_ref()
        .and_then(|nodes| nodes.by_name(CINEMATIC_CAMERA_NODE).first().copied())
        .and_then(|node| globals.get(node).ok())
    else {
        return;
    };
    // Position and orientation only: a node's scale is not a lens.
    let (_, rotation, translation) = eye.to_scale_rotation_translation();
    let view = Transform::from_translation(translation).with_rotation(rotation);

    for (mut transform, mut global) in &mut cameras {
        *transform = view;
        *global = GlobalTransform::from(view);
    }
}

/// Esc, Space, Enter or a click skips an intro.
///
/// Only an intro: an exit ends the mission and has nothing to skip to. Not
/// over the pause menu or while the dev tools' free camera flies, which own
/// those keys then.
// Seven parameters: the input, the three states that decide whether a press
// is a skip, and the outbox.
#[allow(clippy::too_many_arguments)]
pub fn skip_on_key(
    input: ActionInput,
    keys: Res<ButtonInput<KeyCode>>,
    control: Res<PlayerControl>,
    program: Option<Res<AnimProgram>>,
    menu: Option<Res<PauseMenuOpen>>,
    free_camera: Option<Res<FreeCameraActive>>,
    mut skips: MessageWriter<SkipCinematic>,
) {
    if !control.is_cinematic()
        || menu.is_some_and(|menu| menu.0)
        || free_camera.is_some_and(|free| free.0)
    {
        return;
    }
    let pressed = input.just_pressed(Action::Pause)
        || input.just_pressed(Action::Fire)
        || keys.any_just_pressed([KeyCode::Space, KeyCode::Enter, KeyCode::NumpadEnter]);
    if !pressed {
        return;
    }
    if program.is_some_and(|program| is_skippable(&control, &program.0)) {
        skips.write(SkipCinematic);
    }
}

/// Put the chase camera straight behind the hull when the view comes back
/// from a cinematic, and when a script teleported the player.
pub fn snap_chase_camera(
    control: Res<PlayerControl>,
    mut was_cinematic: Local<bool>,
    mut moved: MessageReader<PlayerTeleported>,
    mut cameras: Query<&mut ChaseCamera>,
) {
    let handed_back = *was_cinematic && !control.is_cinematic();
    *was_cinematic = control.is_cinematic();
    let teleported = moved.read().count() > 0;

    if handed_back || teleported {
        for mut camera in &mut cameras {
            camera.snap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::InputBindings;
    use bft_assets::anim::{AnimDefDesc, AnimSet, EventDataDesc, EventDesc, EventStart};

    fn app(control: PlayerControl) -> App {
        let mut app = App::new();
        // No `InputPlugin`: it clears `just_pressed` before `Update` sees a
        // press the test made. The test owns the key state.
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<InputBindings>()
            .init_resource::<PauseMenuOpen>()
            .init_state::<AppState>()
            .add_message::<SkipCinematic>()
            .add_message::<PlayerTeleported>()
            .insert_resource(control)
            .insert_resource(AnimProgram(program()))
            .add_plugins(CinematicViewPlugin);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InGame);
        app.update();
        app
    }

    /// An intro (its reset hands over) and an exit (its does not).
    fn program() -> AnimSet {
        let mut set = AnimSet::default();
        let mut intro = AnimDefDesc {
            anim_name: "m2_start_animation".into(),
            ..Default::default()
        };
        intro.reset_state = vec![EventDesc {
            start: EventStart::WithPrevious,
            data: EventDataDesc::Callback(15),
        }];
        let exit = AnimDefDesc {
            anim_name: "m2_exit_animation".into(),
            ..Default::default()
        };
        for (index, def) in [intro, exit].into_iter().enumerate() {
            set.by_anim_name.insert(def.anim_name.clone(), vec![index]);
            set.defs.push(def);
        }
        set
    }

    fn cinematic(anim: &str) -> PlayerControl {
        PlayerControl::Cinematic {
            anim: anim.into(),
            carried: true,
        }
    }

    fn press_space(app: &mut App) -> usize {
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Space);
        app.update();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(KeyCode::Space);
        keys.clear();
        app.world_mut()
            .resource_mut::<Messages<SkipCinematic>>()
            .drain()
            .count()
    }

    #[test]
    fn a_key_skips_an_intro() {
        let mut app = app(cinematic("m2_start_animation"));
        assert_eq!(press_space(&mut app), 1);
    }

    #[test]
    fn an_exit_is_not_skipped() {
        let mut app = app(cinematic("m2_exit_animation"));
        assert_eq!(press_space(&mut app), 0);
    }

    #[test]
    fn nothing_is_skipped_while_driving() {
        let mut app = app(PlayerControl::Driving);
        assert_eq!(press_space(&mut app), 0);
    }

    /// The view is `camera1`'s, forward and all, while the cinematic runs.
    #[test]
    fn the_play_camera_looks_through_camera1() {
        let mut app = app(cinematic("m2_start_animation"));
        let eye = Transform::from_xyz(10.0, 20.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y);
        let node = app
            .world_mut()
            .spawn((eye, GlobalTransform::from(eye)))
            .id();
        let mut index = NodeIndex::default();
        index.insert(0, CINEMATIC_CAMERA_NODE, node);
        app.insert_resource(index);
        let camera = app
            .world_mut()
            .spawn((
                ChaseCamera::following(Entity::PLACEHOLDER, Default::default()),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();

        app.update();

        let seen = app.world().get::<GlobalTransform>(camera).unwrap();
        assert!((seen.translation() - eye.translation).length() < 1e-4);
        assert!((seen.forward().as_vec3() - eye.forward().as_vec3()).length() < 1e-4);
    }
}
