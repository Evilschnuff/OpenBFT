# `assets/tuning/` — our own override data

These files are **ours**. No extracted game data lives here, or anywhere else
under `assets/` — see the repo `README.md`.

They are an **override layer**, not the primary source. Recoil's own reader
archives (`zrdr.zbd`) are the game's database: vehicle physics, weapon
ballistics, enemy placement, AI parameters and pickups are all read from them
(DESIGN §5.3). Each table is built by reading the reader values first and then
applying whatever these files set, so every leaf is optional and an **empty
file is the correct default**. That is what the stubs here are.

They exist for two things the reader data cannot give us:

- values the original stores in units we have not pinned down, which need
  correcting by hand;
- constants *our* implementation needs that the original has no equivalent
  for — spring stiffness, engine force, AI reaction ticks, aim error.

A value in the second group is a guess until measured. Mark it with a
`// approximation` comment saying which reader field it was derived from.

| File | Overrides | Primary source |
|---|---|---|
| `vehicle.ron` | per-kind vehicle handling, plus a `player` section | `vehicle{,_easy,_hard}.zrd`, `player.zrd` |
| `weapons.ron` | the 19 weapons' ballistics | `weapons.zrd` `BALLISTICS` |
| `ai.ron` | the knobs of our steering implementation | `ai.zrd`, `aiv*.zrd`, `net_*.zrd` |
| `camera.ron` | chase camera offsets | `vehicle.zrd` `camback`, `player.zrd` |
| `craters.ron` | extra level nodes that take craters, per level | gamez node flag `can_modify` |
