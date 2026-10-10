# Open questions

What is not yet known about the original game, and what this engine does not
yet do or has not yet been checked to do. The facts behind each line are in
[data/README.md](data/README.md) and [READER-SCHEMA.md](READER-SCHEMA.md).

## About the original game

- What do the weapon-row columns `a`, `c`, `e` and `f` in `vehicle.zrd` mean?
- What is the third value of a mode's `friction` in `vehicle.zrd` for? The first two read as static and kinetic grip.
- What does the `flag` on a textured `gamez` material mean? It is false on every material in all 13 levels.
- Was m9's reference to `surf00.tif`, which no archive holds, meant to be `surf01.tif`?
- What does a demo install look like? The engine's demo detection (one to six mission directories) is a guess.
- What does the `flags` word on a model's embedded point lights mean? 27 distinct values occur.
- Does a polygon's `priority` (non-zero on 795 polygons, values 1–5 and 10) set the draw order of transparent surfaces?
- Are the two closed models that are wound inside out authoring slips in the original data?
- What does a LOD node's `field00` (0 or 1) distinguish?
- What does the waypoint `flag` in `net_NN.zrd` mean? It is 12 on every node.
- How do `defilade` and `hide` nets behave? Only two nets state a non-zero `hide_times`, so the data barely exercises hiding.
- Are the dwell and range values in `net_NN.zrd` seconds and metres?
- Does a net's `attack_buddy` make a group share a target? It is 0 in every shipped net.
- Is `ai.zrd`'s `FIRE_LIMITS` the shortest and longest burst in seconds?
- Do `LOS: 1` and `LOS: 2` in `ai.zrd` differ?
- Does a positive `reset_time` (0.1–10 s, on 34 definitions) re-arm a definition after the delay, or re-arm it and replay it at once?
- What do an objective's `AUTOPLAY` and `HINT_ACTIVE` do?
- Does `ObjectMotion`'s `forward_rotation` spin a node about its own x axis or about its direction of travel?
- What does `CallObjectConnector`'s `save_index` keep?
- What does `Callback 20` mean? It occurs only in the mission intros.
- Could the player skip the mission intros, and with which key?
- Was the player's input locked during the exit cinematics?
- What starts `radar_spin` in m2–m6? Nothing in the scripts calls it.
- Do the form-change `*_morph` groups of `bft_00` blend between the track and pontoon meshes or swap them?
- What do the weapon swap, the chassis lean and the recoil look like? The engine's are guesses from the model's geometry.
- Does the pulse guns' scrolling strip (`RFPG_RSCROLL`) scroll only while firing, or all the time?
- How fast does the tank-track texture cycle advance with ground speed? The data gives four frames at speed 0 and no rate.
- When does the sub's `pinging` sound play? The engine loops it while the sub is submerged.
- Is the `low_shield` threshold of 10 a percentage of maximum health?
- What is the tether-guided missile's ceiling (`WeaponSetMaxTetherAltitude 30.0`) measured from? The engine measures it from the ground under the missile.
- How does a positional sound's volume fall off between its `RANGE` minimum and maximum? The engine falls off linearly.
- What are the 15 unreferenced `-1`/`-2` WAV variants in the sound archives for?
- What does an anim definition's `save_log` flag mean? If it marks what a save has to keep, saves could leave cosmetic copies out.
- How did the original score points? No reader file gives points per kill; the engine scores a kill as the victim's full health.
- Did the original letterbox the 3D view between the HUD strips (`TYPEII`), or draw it full-screen beneath them?
- What do the six unread `STATS` records in `hud.zrd` hold? Their positions suggest the multiplayer scoreboard.
- What are `MAINMENU2` and `MAINMENU3` in `dialog.zrd` for? Their buttons suggest the multiplayer in-game menu and a menu after a lost mission.
- Did the in-game menu pause a single-player game?
- What do the commands dialog's `LIST_OFFSET` and the two empty boxes above its column headings mean?

## About this engine

- Multiplayer is not implemented: no networking, deathmatch rules, host or join.
- There is no game mode for the race map (m9) or the teleporter arena (m13).
- The original's multiplayer dialogs (`MP_NEW_GAME`, `NETEXIT`, `MPEXIT`) are not wired up.
- Can a multiplayer game be saved? Our decision, not an observation: this engine's multiplayer follows modern conventions rather than the original's, so the observation item on saving (saves-1) leaves multiplayer out. The original's string table has a refusal for it (`MSG_CANT_SAVE_MP`, SAVEGAME.md §1).
- Once mech3ax's output format is stable, should the engine depend on its crates instead of running `unzbd`?
- Should the extraction cache default to a directory next to the data root rather than the platform cache directory?
- Should model vertices be welded instead of duplicated per polygon? The saving has not been measured.
- Does loading every texture archive up front keep a level load within the 5 s budget?
- How bright should the sun and the ambient light be? The data gives only a diffuse/ambient split, so the absolute values are tuning guesses.
- Should the typed views over the reader files (vehicles, weapons, pickups, AI, pickup placement, player) move from `bft-sim` into `bft-assets::reader`, next to the mission and sound views?
- Changing form plays no transition animation, does not swap the model's form geometry and does not rebuild the hull collider from the new form's outline.
- The hover, amphib and sub bobbing (`*_wave`), the hover altitude gains (`alt_control`) and the chassis motion (`chas_smooth`, `chas_pitch`, `chas_roll`) are read but not applied.
- Collisions do not damage vehicles: the damage is computed from a closing-speed approximation of the impulse, logged, and not applied.
- Enemy vehicles keep a box collider; the track collider has not been checked against the real levels for them.
- Enemy vehicles' tracks and chassis lean are not animated.
- The player's muzzle is a fixed hull offset, so a shot does not follow the turret's yaw or the barrel's pitch.
- Enemies aim at the player's hull origin rather than its `target` node.
- Turret `DEACTIVATE` conditions are read, but nothing switches a turret off.
- SAM sites are not spawned as turrets, because the VTOL they shoot at is a scripted set piece with no entity.
- A turret's yaw part is set to a pure yaw, dropping any rest rotation the node has in `gamez`.
- When two props share a non-template operand script (`destroy_turret`, `destroy_bunker`, `destroy_bridge`, m9's signs) within one run, they share one instance.
- Cloned enemies carry the `gamez` active flags rather than their template's reset states.
- Entry and death animations outside m1 have not been checked against the prerequisites and execution conditions that external calls honour.
- `CallAnimation`'s `wait_for_completion` is not honoured; a waiting call runs concurrently.
- `NodeUndercover` conditions always read as false, because there is no roof test.
- `FbfxCsinwaveFromTo` keeps its timing and draws nothing.
- `ObjectMotion` stops at its first landing.
- Anim `morph` channels are not honoured.
- Connectors assume a one-metre model along z.
- `FIRE/RANDOM_ROTATE` does not spin the pulse guns' muzzle flash.
- `FLYOUT/MODEL_ANIMATION` (the pulse bolts' point lights) is not played.
- A destruction plays one of `sounds.ron`'s explosion sounds even when the victim's kill animation has sound events of its own.
- Callbacks 25, 26 and 27 (scripted damage to the player) and 911–914 (pickups and props) are logged and not acted on.
- The right mouse button does not toggle between a guided missile's view and the BFT's during the flight.
- A guided missile is not saved as guided; after a load it flies straight.
- The Nano-Canister is not held and applied on a key; a nanite pickup heals at once, so the HUD's canister slots stay empty.
- The HUD draws no lock-on marker and no threat arrows.
- Objective voice lines (`READ_SOUND`, `HINT_SOUND`) are not played.
- The result screen shows no accuracy, because shots are not counted.
- Only the ammo and health cheats work; the others are refused.
- The options dialog's lighting, perspective, detail, texture memory, effects and sound quality controls are drawn but do nothing, and the controls dialog is inert.
- The save list shows the newest ten saves and does not scroll.
- The `lucida_console` bitmap fonts and `BACKGROUND_VIDEOS` are not used.
- The live-action videos are not played.

### Not yet verified in play

These are covered by tests but not yet confirmed in the running game.

- The death cinematic.
- The full menu flow with a mouse, and the gamepad bindings on hardware.
- The pause menu's dialogs and the live save picture (unit tests only).
- Positional panning, loudness balance and engine pitch (checked numerically only).
- Doors, elevators, triggered events, the exit cinematics, the elevator and teleport camera snap and the animation copies (checked in the simulation only).
- A campaign played from start to finish.
- The player model's tracks, lean, weapon swap, recoil and trails, tuned against the original.
- The enemy steering defaults over long play on the real levels.
- The dev tools' panel toggle, tree selection, Ctrl+click picking and labels, and their tick timings under a profiler.
- The HUD and the menus in 4K and 21:9, and side by side with the original; the HUD's nearest-or-linear filtering is chosen by reasoning, not on screen.
