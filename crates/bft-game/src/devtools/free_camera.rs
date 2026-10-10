//! Free camera in play: pause the game and fly the play camera around, then
//! drop back into the chase view on the same key.
//!
//! `Action::FreeCamera` (F2) toggles it; `Action::Pause` (Esc) also leaves
//! it, rather than opening the pause menu. Nothing new is needed to fly: the
//! viewer's [`FlyCamera`] systems drive any camera carrying the component, so
//! entering puts one on the chase camera and leaving takes it off, and
//! `chase_camera::follow_target` skips a camera that has one. While it is on:
//!
//! - [`Paused`] holds the sim still, as the pause menu does;
//! - [`gameplay_input_enabled`](crate::ui::gameplay_input_enabled) is false,
//!   so W/A/S/D fly instead of drive and a click neither fires nor aims;
//! - the HUD stays up, and looping sounds carry on.
//!
//! On leaving, the chase camera eases back behind the vehicle from wherever
//! the flight ended.

use bevy::prelude::*;
use bft_sim::states::{AppState, Paused};

use crate::camera::FlyCamera;
use crate::chase_camera::ChaseCamera;
use crate::ui::{Action, ActionInput, PauseMenuOpen};

/// Whether the play camera is flying free.
///
/// Its presence also tells [`GameplayGate`](crate::ui::GameplayGate) and the
/// pause menu that the dev tools own this key pair; without the dev tools it
/// does not exist and neither changes behaviour.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FreeCameraActive(pub bool);

pub(super) struct FreeCameraPlugin;

impl Plugin for FreeCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FreeCameraActive>()
            .add_systems(Update, toggle.run_if(in_state(AppState::InGame)))
            .add_systems(OnExit(AppState::InGame), reset);
    }
}

/// F2 enters and leaves; Esc leaves.
fn toggle(
    mut commands: Commands,
    input: ActionInput,
    mut active: ResMut<FreeCameraActive>,
    mut paused: ResMut<Paused>,
    menu: Option<Res<PauseMenuOpen>>,
    cameras: Query<(Entity, &Transform), With<ChaseCamera>>,
) {
    let toggle = input.just_pressed(Action::FreeCamera);

    if !active.0 {
        // Not over the pause menu: that already owns `Paused`, and handing it
        // back on leaving would unpause under the menu.
        if !toggle || menu.is_some_and(|menu| menu.0) || paused.0 {
            return;
        }
        let Some((camera, transform)) = cameras.iter().next() else {
            return;
        };

        let eye = transform.translation;
        let (fly, _) = FlyCamera::looking_at(eye, eye + *transform.forward());
        commands.entity(camera).insert(fly);
        active.0 = true;
        paused.0 = true;
        info!("free camera on: F2 or Esc to return");
        return;
    }

    if toggle || input.just_pressed(Action::Pause) {
        for (camera, _) in &cameras {
            commands.entity(camera).remove::<FlyCamera>();
        }
        active.0 = false;
        paused.0 = false;
        info!("free camera off");
    }
}

/// Leaving the game in free camera must not leave the flag set for the next.
fn reset(mut active: ResMut<FreeCameraActive>) {
    active.set_if_neq(FreeCameraActive(false));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Binding, InputBindings};

    fn app() -> App {
        let mut app = App::new();
        // No `InputPlugin`: it clears `just_pressed` at the start of each
        // frame, before `Update` sees a press the test made. The test owns
        // the key state instead.
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_state::<AppState>()
            .init_resource::<Paused>()
            .init_resource::<PauseMenuOpen>()
            .init_resource::<InputBindings>()
            .add_plugins(FreeCameraPlugin);
        app.world_mut()
            .resource_mut::<NextState<AppState>>()
            .set(AppState::InGame);
        app.update();
        app
    }

    fn press(app: &mut App, action: Action) {
        let key = app
            .world()
            .resource::<InputBindings>()
            .get(action)
            .iter()
            .find_map(|binding| match binding {
                Binding::Key(key) => Some(*key),
                _ => None,
            })
            .expect("a key binding");
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(key);
        app.update();
        let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        keys.release(key);
        keys.clear();
        app.update();
    }

    #[test]
    fn esc_does_nothing_while_driving() {
        let mut app = app();
        chase_camera(&mut app);

        press(&mut app, Action::Pause);

        assert!(!app.world().resource::<FreeCameraActive>().0);
        assert!(!app.world().resource::<Paused>().0);
    }

    fn chase_camera(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                ChaseCamera::following(Entity::PLACEHOLDER, Default::default()),
                Transform::from_xyz(1.0, 2.0, 3.0),
            ))
            .id()
    }

    #[test]
    fn the_same_key_pauses_to_fly_and_resumes() {
        let mut app = app();
        let camera = chase_camera(&mut app);

        press(&mut app, Action::FreeCamera);
        assert!(app.world().resource::<FreeCameraActive>().0);
        assert!(app.world().resource::<Paused>().0);
        assert!(app.world().get::<FlyCamera>(camera).is_some());

        press(&mut app, Action::FreeCamera);
        assert!(!app.world().resource::<FreeCameraActive>().0);
        assert!(!app.world().resource::<Paused>().0);
        assert!(app.world().get::<FlyCamera>(camera).is_none());
    }

    #[test]
    fn esc_leaves_free_camera() {
        let mut app = app();
        let camera = chase_camera(&mut app);

        press(&mut app, Action::FreeCamera);
        assert!(app.world().resource::<FreeCameraActive>().0, "entered");
        press(&mut app, Action::Pause);

        assert!(!app.world().resource::<FreeCameraActive>().0);
        assert!(!app.world().resource::<Paused>().0);
        assert!(app.world().get::<FlyCamera>(camera).is_none());
    }

    #[test]
    fn not_from_the_pause_menu() {
        let mut app = app();
        chase_camera(&mut app);
        app.world_mut().resource_mut::<PauseMenuOpen>().0 = true;
        app.world_mut().resource_mut::<Paused>().0 = true;

        press(&mut app, Action::FreeCamera);

        assert!(!app.world().resource::<FreeCameraActive>().0);
    }
}
