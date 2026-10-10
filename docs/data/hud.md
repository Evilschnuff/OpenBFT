# The HUD (`hud.zrd`, `image.zbd`)

The HUD layout records, their art, and what they mean according to the manual.

From parsing `hud.zrd`, looking at every
texture it names, and reading the manual's HUD pages (`MANUAL.PDF`
pp. 18–19, 40). Parser: `bft_assets::hud`; format: READER-SCHEMA §2.7a;
checks against the install: `crates/bft-assets/tests/real_hud.rs` and
`crates/bft-game/tests/real_ui.rs`.

**Records.** Three shapes recur. A *placed image* is `[texture, x, y,
flag, flag]` with the flags spelt `"TRUE"`/`"FALSE"`: the first is set on
exactly the textures `image.zbd` stores with alpha (checked one for one),
the second only on `targ_ret`, whose position is the centre of the sensor
view — it places the image by its centre. A *rect* is `[x0, y0, x1, y1]`
corners, not a size: `SHIELD`'s fill is the inner window of `health`,
`OBJECTIVE`'s fourth record the slot inside `objectab`. Positions under
`SENSOR` and `SHIELD` are relative to their frame image; `objectab`'s is
relative to `monitor` (it sits in the monitor's dark top-right corner, the
enemy bar under it). Confidence: verified against the art.

**Wide screens.** `tops800`/`tops1024` and `iback800`/`iback1024` are the
640 strips with 160 and 384 columns of plain metal *inserted*, not
stretched: the top strip's middle slot moves by half the extra and its
right slot by all of it, and the icon strip's panel stays centred with the
extra split between its ends (measured on the dark runs of each image).
That is anchoring by thirds of the width, and `800 − 255 = 545` /
`1024 − 255 = 769` are exactly the space between the monitor (left) and
the health gauge plus forms (right). The engine extends the same rule to
any width. Confidence: verified.

**Weapon icons.** Each `WEAPON` row's five frames are: first variant,
second variant, greyed (nothing to fire), first selected, second selected
(the selected frames carry the green border the manual describes). The two
`icm_*` minis are "a small icon near the lower-right corner … another
weapon is associated with the larger icon's selection key" (manual): the
first variant's, then the second's. Row 7 confirms the order by mixing
names — `ic_sonc*` frames for the sonic wave, `ic_arc*` for the arc sabre,
and `icm_g2` (orange) before `icm_g1` (blue). Two frames in the archive
are unused by the layout (`ic_nplm4`, `ic_arc2`). Confidence: verified.

**Reticle.** The manual gives three states: the bare circle ("not
currently pointing at an object within range"), four red triangles ("will
hit an object if fired") and green triangles ("the current target can be
damaged"). So `ret_off` is nothing in range, `ret_range` is *something in
range*, `ret_on` is damageable. Confidence: verified (manual).

**Forms.** `*_off` is an empty panel (not collected), `*_on` the form's
picture with a dim light, `*_pik` the same with the light lit (the current
form). `MODES` rows are tank, amphib, hover, sub. Confidence: verified
against the art.

**Nanites.** `hnanite` (a red cross) is the *Nano-Canister*: "you can hold
as many as 3 … if the BFT health meter is depleted, the Nano-Canister
automatically dissolves" (manual p. 40), and `MSG_CMD_NANITE` "Apply
Nanite" is a key for using one. `NANITE`'s three slots show the canisters
held. `bft-sim` heals on pickup instead, so there is nothing to show yet.
Confidence: verified (manual, strings).

**Numbers and text.** `STATS`' first two points are the score and the
clock slots of `topstrip`; its other six records are unread (their
positions suggest the multiplayer scoreboard). `SHIELD`'s `[x, y, digits,
flag]` is the percentage over the bar ("the number is a percentage of the
BFT's maximum health", manual). `FONTS` names Windows faces (`Arial`,
`Symbol` for `AMMO`) with a `CreateFont`-style height, width and weight.
"Incoming transmission - Press O key" is `MSG_INCOMING`; the score's own
string is `MSG_SCORE` ("Score %1!d!"). No reader file gives points per
kill. Confidence: verified for the keys, inferred for `STATS` and the
readout record.

**Message tab.** "Mission reviews, mission objectives, weapon descriptions
… appear in a bar that grows out from this tab" (manual). `OBJECTIVE`
holds the slide time, the bar's rect and colour, the two text offsets and a
last `[margin, width]`-shaped record read as the text box. Confidence:
inferred for the bar growing to the right over the icon strip (its width
matches the strip's).

**Menu pointer.** Every dialog in `dialog.zrd` ends with `CURSOR BITMAP
Cursor`: the 10×18 arrow `cursor` in `image.zbd`, tip at its top-left
texel. Confidence: verified.

**`cockpit.zrd`** has its own key list — `SCORE`, `SPEED`,
`LEFT_BACKGROUND`, `RIGHT_BACKGROUND` among `hud.zrd`'s — which is where
READER-SCHEMA's `SCORE`/`SPEED` came from; `hud.zrd` has neither. Nothing
here reads it, and its art is in no archive.
