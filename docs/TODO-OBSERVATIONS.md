# Observations from play

Where playing the engine shows it differing from the original game.

- [ ] The fuses at the m1 arena entrance vanish instead of showing their destroyed model.
- [ ] The live-action videos do not play yet.
- [ ] Several weapons are incomplete:
  - [ ] The laser fires a simple projectile instead of a bouncing, continuous ray.
  - [ ] The flamethrower shows no flame projectiles.
  - [ ] The sonic weapon shows no ripple effect on a hit.
  - [ ] The mortar does not show the original's sprite explosion on a hit.
  - [x] Camera-guided missiles (with their own camera).
- [x] Enemy AI does not act like the original's; many enemies are passive.
- [x] Enemies are not animated (turrets mostly do not turn).
- [ ] Destructible objects do not get the green reticle highlight.
- [ ] Destructible objects may have the wrong health (they die too fast).
- [ ] The rocket turrets in the m1 arena do not shoot at the player.
- [ ] The rocket turrets are not destructible.
- [x] Destroyed enemies leave fire and smoke sprites behind forever (a frozen fire sprite).
- [ ] The camera does not pull in in front of colliders, so it sees through the world.
- [x] Because the camera does not pull in, the tank sometimes aims behind itself: the mouse ray starts inside a collider.
- [ ] Enemies always drop all their pickups, instead of a chance at one of them.
- [ ] The player starts without the mortar, but the original starts with it (there are no mortar pickups).
- [ ] Player health does not grant extra lives, or at least the HUD does not show the red crosses.
- [ ] The game does not capture the mouse.
- [x] The tank is still a bit too floaty: it can drive up ledges the original could not, but cannot use mortar craters as ramps. The original used those jumps deliberately to reach some areas, such as the rock next to the m1 pier with the lighthouse mine weapon pickup.
