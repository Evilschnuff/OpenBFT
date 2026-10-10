# Messages

The string table extracted from `messages.dll` and its key families.

The extraction turns `messages.dll` into **749 messages** (`language_id` 1033,
US English). Values use `%1!s!`-style Windows message-table placeholders
(`%1`, `%2`, `%n` newline), which need translating to Rust format strings.
The text is in a Windows message-table resource; read it through
`unzbd rc messages`.

Key families (all keys start `MSG_`):

- **Weapons:** `MSG_DESC_<id>` long names for all 19 `weapons.zrd`
  entries (`MSG_DESC_RFPG` "Rapid Fire Pulse Gun" … `MSG_DESC_TG_PN`
  "Tether Guided Practical Nuke"), `MSG_<NAME>_W` short HUD names
  (`MSG_HE_MORTAR_W`), `MSG_KILL_VERB_*` (12 verbs for the multiplayer
  kill line), `MSG_AMMO_LIMIT`, `MSG_AMMO_PICKUP(S)`, `MSG_WEAPON_INACTIVE`
  "%1!s! not operative in Sub Mode", `MSG_LOCK_ON` / `MSG_LOCKEDON`.
- **Forms:** `MSG_AMPHIB_PICKUP` / `MSG_HOVER_PICKUP` / `MSG_SUB_PICKUP`
  "… Technology Enabled".
- **Missions:** `MSG_CAMPAIGN<1..13>_NAME` (level titles, reader.md),
  `MSG_M<1..6>_O<n>_{SUMMARY,OBJ,COMP}` — **32 objectives**: m1 4, m2 5,
  m3 6, m4 6, m5 6, m6 5 — the only objective texts; `MSG_OBJECTIVES`,
  `MSG_LOAD_NEXT_MISSION`, `MSG_GAME_OVER`.
- **HUD/status:** `MSG_SCORE`, `MSG_KILLS`, `MSG_NANITE`, `MSG_NANITE100`,
  `MSG_SHIELDS_AT_MAX`, `MSG_SHIELD_INCREASE`.
- **Multiplayer:** `MSG_NETWORK_*`, `MSG_NET_*` (race lobby: "Wait For
  Players To Join The Race", "Hit Spacebar To Begin Race Countdown"),
  `MSG_LAPS`, `MSG_ONE_LAP_TO_GO`, `MSG_WOL_*` (Westwood Online lobby),
  `MSG_CMD_TAUNT_*`, `MSG_CANT_SAVE_MP`.
- **UI:** save/load dialogs, key names (`MSG_KEY_*`), command help
  (`MSG_CMD_*_VERBOSE`), error strings.

Confidence: verified.
