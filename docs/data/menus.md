# The menus (`dialog.zrd`, `briefing.zrd`, `image.zbd`)

The front-end dialogs and briefings: their widgets, art and meanings.

From parsing both files, compositing
every dialog from `image.zbd` outside the repository, and the manual's
menu pages (`MANUAL.PDF` pp. 9–11, 15). Parser:
`bft_assets::dialog`; format: READER-SCHEMA §2.6, §2.7b; checks against
the install: `crates/bft-assets/tests/real_dialogs.rs` and
`crates/bft-game/tests/real_menus.rs`.

**Everything resolves.** All 245 bitmaps the two files name (207 in
`dialog.zrd`, 38 in `briefing.zrd`) are in `image.zbd` once lower-cased,
and every `MSG_*` key they name is in the string table. Every control's
rollover and select sound is `snd_rollover` or `snd_select`, the whole of
`sounds.zrd`'s `DIALOG` set. Confidence: verified.

**Strips and canvas.** Every dialog's first background is an 84×768 strip
(83 for `mainmenu`) at `y = −144`: 768 lines centred on a 480-line canvas.
The rest of each design sits right of it, joined to it by drawn pipes, so a
dialog is one piece hung off the left edge. Confidence: verified.

**What was behind the dialogs.** Before a game, the splash: `Recoil.exe`
carries one bitmap resource, `GAMEBMP`, a 640×480 8-bit DIB — the red
wallpaper with RECOIL across the middle, the Zipper and EA logos and the
copyright line. It is in no archive; the splash loader reads it from the
program file's standard resource table. It served as both the loading
splash and the front-end menus' background. In play, the game blurred:
`fmv.zrd` ends `MISSIONOVER` and each `M<n>` with `BLUR`. Confidence:
verified for the resource; recalled from the original for its use;
inferred for the blur (from `fmv.zrd`'s playlists).

**Widgets, by the keys a control has** (the parser's `ControlKind`):
`CHECKED` a checkbox whose tick is text; `CYCLE` states of `TEXT` and an
optional `BITMAP` (the resolution previews, the multiplayer map
thumbnails); `RADIO` a list of full controls, whose `ACTIVATE BITMAP` is
the chosen look (`HotBT_S`); `FILLBITMAP` a bar; `LISTSIZE`… a list;
`FONT` + `TEXTOFFSET` a button with text; `LABEL` alone a field (an empty
text the game fills or the player types); `SCROLLING_TEXT` the credits.
Checkboxes and cycles have no idle bitmap: their boxes are in the
background art and only the hover bitmap is theirs. Confidence: verified.

**Meanings from the art.** `INTENSITY`'s three buttons read EASY, MEDIUM,
HARD (`HotBT`, `NukeBT`, `MeltBT`). `DISABLE` bitmaps say why:
`LoadGame_no` and `SaveGame_no` "can not load/save while moving",
`NewGame_D` "winner picks next session", `start_no` "not in demo".
Confidence: verified.

**Unmapped dialogs.** `MAINMENU2` has Resume, Options, Controls, Credits
and Quit and no New, Save or Load — the in-game menu of a multiplayer
game, by its buttons. `MAINMENU3` has Load, New and Quit alone over the new
game art; nothing says whether or when it is shown — a guess is the menu
after a lost mission. `NETEXIT`, `MPEXIT`,
`MP_NEW_GAME` and `WOL_NEW_GAME` are multiplayer. Confidence: inferred.

**Save and load dialogs.** Nine `LIST_n` rows, three above the name bar
(`GAMENAME`, over `LoadFrame3`) and six below; the arrows move the
selection through the bar. Confidence: inferred from the layout.

**Commands dialog.** `COMMANDS_DIALOG` is a table: a command column and
four columns headed KEY A, KEY B, MOUSE, JOYSTICK in the art, seven rows
(`LISTSIZE [7, 2]`) at `SPACING` 15, the command set's name on
`CMD_SET_BTN` (`MSG_CMDSET_*`), the description under it
(`MSG_CMD_*_VERBOSE`). `LIST_OFFSET [[0, −25], [0, 18]]` is unknown: the
first offset from the list lands on the column headings, not on the empty
boxes above them. Confidence: verified for the table, unknown for the
offsets and the boxes.

**Credits.** `SCROLLING_TEXT` is `[[text, x, gap, font]]` lines: indent,
extra space above, font. `RECT [[x0, y0], [x1, y1]]` is the window inside
`credit_bak` the text rolls over, `SCROLL_RATE` seconds per pixel. The line
after the title has a 200-pixel gap, so the title rolls through alone.
`BACK` and `QUIT` share a position: the roll's front-end and end-of-game
exits. Confidence: verified for the fields, inferred for `QUIT`.

**Briefings.** `CAMPAIGN1`–`6` have the objective panel (`OBJECTIVE_PICT`,
`OBJECTIVE_SUMMARY`, `OBJECTIVE_DESC` with `WORDWRAP`, `MISSION_NAME`,
`MESSAGES`, `TRANSMISSION_HALTED`) and `LOCATOR1`… on the map;
`CAMPAIGN7`–`13` only the map, name and transport bar. The number of
locators is the number of objectives, hidden ones included, on m1–m5 (m6
has one for five); m1's second, checked against the map, falls on that
objective's place (the command post) on `map_c1`. No bitmap is named for a locator. Objective pictures come from
each mission's `objectives.zrd`, not from here. The summaries' `M<k>:`
prefix ("M1: Operation Short Circuit") numbers a campaign's missions —
read as what `MISSION_NAME` shows. The voice-over is `snd_briefing_c<n>`,
the one sound in `sounds.zrd`'s set `BRIEFING<n>` (`BRIEFING1`–`6`).
Confidence: verified for the fields and counts, inferred for locator
meaning and `MISSION_NAME`.

**Transport.** `hud.zrd`'s `BRIEFING` gives the bar's words:
`LOADING_TEXT` "Transporting" in magenta with `STAGES [4]` — matching
`image.zbd`'s `transport1`–`4`, the word pre-rendered at four brightnesses,
which no file names — and `READY_TEXT` "PROCEED" in green. `NUMBER`,
`PICT`, `DESC`, `NAME` and `MISSION_NUM [4, 5, 6, 4, 6, 4]` are unread.
Confidence: verified for the words; the flow (load behind the briefing,
then hold for the player) is this engine's reading of them (I).

**Fonts.** Every dialog font is Eurostile but `MP_NEW_GAME`'s `Courier
New`. `fonts.zrd` lists `lucida_console_8` and `_8b`, bitmap fonts in
`image.zbd` (a 670×12 strip of the 95 printable ASCII characters); no
dialog uses them. The fifth `FONTS` field is 0 only on the dark
input-field font. Confidence: verified for the faces, unknown for the flag.

**Cheats.** The words are in the string table (`MSG_CHEAT_AMMO`,
`_HEALTH`, `_INVINCIBLE`, `_TIMER`, `_FRAMERATE`, `_GOD`); the dialog is
`CHEAT_CODE_DIALOG`, Ctrl+X in play (manual p. 15). Confidence: verified.
