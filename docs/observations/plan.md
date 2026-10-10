# Observation plan

What to observe in the original game next: questions that the data files
and the manual leave open. Each item has a file in `tools/observe/items/`
with its markset, its analysis and how to record it; once recorded,
`observe result <item>` writes its entry into `<topic>.md` here. The
precision in brackets is what the item expects to reach. How it all works:
[README.md](README.md).

All items are recorded in m1; saves-1's VTOL arrival is at the start of m2,
where the BFT arrives from m1. Approximate items are recorded three times;
exact ones once, and again only if two runs disagree.

Nothing has been recorded yet, so no item has a result.

## Items

- **craters-1** Make a crater with the mortar and drive into it: does the
  BFT sink below the ground around it? (exact)
- **craters-2** Look down into a fresh crater: the rim's outline (how many
  sides), and whether the floor is flat or a bowl. Compare with
  declient.zrd's POINTS. (exact)
- **craters-3** Crater size for each weapon whose weapons.zrd row has
  CRATER, against its numbers and declient.zrd's RADIUS and DEPTH. Wall
  slope from a low camera at the rim, against SLOPE. (approximate)
- **hud-1** The original's options screen: screenshot every HUD-related
  option and its wording. (exact)
- **briefing-1** Record a briefing from its first frame: what the
  transport bar does, how long it takes, when PROCEED appears, what a key
  press does before and after. (exact, durations approximate)
- **saves-1** Save and load from the menu and with the keys the manual
  gives; try while being carried (the truck at the start of m1, a VTOL
  departure, a VTOL arrival) and once in normal play; screenshot every
  message. (exact)
- **menus-1** Mission ends and transitions: is the picture blurred, and
  how? Does the MAINMENU3 layout (dialog.zrd) ever appear, e.g. after a
  lost mission? (exact)
- **canisters-1** Hold a Nano-Canister and take damage until the meter
  empties: what happens on screen, and what health comes back. (exact)
- **canisters-2** Collect canisters past the manual's limit: does the
  extra one stay on the ground? (exact)
- **canisters-3** A large hit that empties the meter: is the new health
  dented at once? (approximate, hard)
- **canisters-4** Do canisters carry over to the next mission? (exact)
