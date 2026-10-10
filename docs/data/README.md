# Data notes — what the real Recoil data looks like

What has been measured in the real data. Every section ends with a
confidence line. Nothing here may contain game data beyond short
illustrative excerpts.

Source: retail EA release, English, version 1.0 (from `DATA.TAG`). The
original mixed-mode CD holds an InstallShield 5 installer (`data1.cab`, 220
files) plus four CD-audio tracks and the briefing videos. install.md
describes the tree a full install produces (game files + hardware textures
+ software textures + one sound-fidelity set + videos + fonts).

(V), (I) and (U) after a finding mark it verified, inferred or unknown, in
the sense READER-SCHEMA.md's legend gives them.

One file per topic:

- [install.md](install.md) — the retail install tree and which level is which, the pinned mech3ax commit, and the commands that extract the data.
- [coordinates.md](coordinates.md) — up axis, unit scale, handedness and the transform forms of the gamez data.
- [nodes.md](nodes.md) — what the gamez node list holds: its JSON shape, counts, flags, zones and names, the hierarchy, and the player and enemy subtrees.
- [models.md](models.md) — polygons, model types, flags, normals, UVs and embedded lights in the gamez models.
- [materials.md](materials.md) — materials, soil, cycled textures and the texture archives, and how gamez texture names resolve.
- [water.md](water.md) — how water, lava and quicksand are built in the levels and how the engine finds them.
- [reader.md](reader.md) — what the global and per-mission reader archives hold, with the corrections to READER-SCHEMA.md found while building the tables.
- [interp.md](interp.md) — what the interp scripts contain, the build/runtime split, and the runtime subset the engine extracts.
- [anims.md](anims.md) — the compiled anim definitions: fields, activation and timing, event handlers, instancing, cinematics and the missions they drive.
- [messages.md](messages.md) — the string table extracted from `messages.dll` and its key families.
- [sounds.md](sounds.md) — the three sound archives, their naming, and how sound names resolve through `sounds.zrd`.
- [quirks.md](quirks.md) — every warning mech3ax emitted during extraction, and the inconsistencies between levels and between mech3ax's types and its output.
- [hud.md](hud.md) — the HUD layout records, their art, and what they mean according to the manual.
- [menus.md](menus.md) — the front-end dialogs and briefings: their widgets, art and meanings.
