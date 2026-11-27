# R-9: unattended runs can hold Enter through the opening's cutscenes

Unattended runs (`scripts/measure-run.ps1`) never got past the opening: it is several
cutscenes, each skipped by holding Enter, and the run only tapped Enter for 0.2 s every 8 s. At
5 to 7 fps how long Enter has to stay down in real time changes from run to run, so no fixed tap
length works.

`LEONIDA_SKIP_SCENES=1` (every build, off unless set) looks at the presented picture
instead. From the world's first frame on (the first that ran 500 draws or more), every 8th presented
frame has eight full-width rows copied into a small readback buffer by the present's own list: three
in the top bar (1%, 2.5% and 4% of the height), two in the bottom bar (98% and 99%) and three in the
picture (8%, 50% and 92%). The rows are judged at a later present, once that list has finished, so
nothing waits for the card. The copy reads the title's frame, not the back buffer, so the frame rate
box is not in it. Only 8-bit color frames are read; any other format logs one warning and skips
nothing.

A row is a bar when no color channel of every 4th pixel is above 12. A sample is:

- **letterboxed** (a cutscene): all five bar rows are bars and a picture row goes above 24;
- **open**: the 1% or the 2.5% row is not a bar;
- **black** (a fade or a loading screen): every row is a bar;
- **unsure**: anything else.

Enter is held after two letterboxed samples in a row and let go after three open ones. Black and
unsure samples change nothing, so a hold carries across the fade between two cutscenes. While
holding, Enter is down and then up for 3 presented frames (and at least 0.5 s), so a skip that
wants a fresh press gets one. The first press lasts 48 frames and every next one of the same hold
twice as long, up to 384: how long a skip wants Enter down is not known, and one length that is
too short every time would never skip. A hold that goes 1200 frames without a black sample stops,
and is not taken up again before an open or a black sample: a letterbox Enter cannot skip, and
the cutscene after its fade is held again. The held Enter joins the scripted keys
(with `LEONIDA_KEYS` in a diagnostics build), so it reaches both the keyboard and the pad button
the stand-in pad maps Enter to.
The log has one line when a hold starts and one when it ends, with the seconds held; no picture
data.

`measure-run.ps1 -SkipScenes` turns it on. Without it a run is as before, so measurements of the
opening cutscene (R-9, P-9) don't change. The taps stay as they were (the start screen needs them),
and `summary.txt` has a new `cutscenes_held` line.

## In every build

The skipper was first only in the diagnostics build. It is now in the plain release build too
(`run-gta6.ps1`, the launcher, `leonida run`), still off unless `LEONIDA_SKIP_SCENES=1`. Only what
it needs is built in: the sampling in `leonida-gpu/src/skip.rs`, its calls at the present, and the
held Enter in the input code. `LEONIDA_KEYS` and the other debugging aids stay diagnostics-only.
The held Enter goes the same way in both builds: into the keys the title reads and onto the first
pad's button for Enter.

Switched off, each present checks the switch (read from the environment once) and skips the rest;
the input code reads one flag that stays false. Nothing is copied, allocated or logged.

A player turns it on without setting anything by hand: a file `gpu-profiles.local.json` beside
`leonida.toml` (normally the repository's folder) with

```json
{
  "profiles": [
    { "name": "skip-scenes", "env": { "LEONIDA_SKIP_SCENES": "1" } }
  ]
}
```

A local profile with no `match` applies on every card while profiles are picked automatically, and
the program sets its switches before the graphics start. If the file is already there, the object
goes into its `profiles` list. Deleting the object turns skipping off again. An explicit
`LEONIDA_SKIP_SCENES` in the environment still wins over the profile.

Profiles are not picked automatically when `[graphics]` in `leonida.toml` has a `profiles` line or
`LEONIDA_PROFILE` names profiles: then `skip-scenes` has to be in that list. With
`LEONIDA_GRAPHICS_OFF=1` (a measurement run with the speed switches off) no profile applies, this
one included. The line `graphics card: ...; profiles: ...` the program prints at start lists
`skip-scenes` when it is on.

## Where the numbers come from

The bars were measured on saved half-size world pictures of 25 earlier runs on one PC: of 115
pictures, 68 had bars, and 62 of those were exactly 60 rows top and bottom at 1080 rows
(5.6% each, a 2:1 picture). Some had subtitles reaching 32 rows into the bottom bar, hence the
bottom rows at 98% and 99%; one pair of 48-row bars is why the lowest top row is at 4%, not 5%. Of
304 pictures before the world (menus, loading screens, title cards), none had bars. The labels come
from the order of the pictures in each run and from the statistics, not from someone looking at
them.

## Measured

RTX 3080 12 GB, driver 621.24, Ryzen 7 7800X3D, 2026-11-27: two runs of `measure-run.ps1` on the
diagnostics build, profile `base`, 900 s each.

| run | hold | let go after start | median frame after | card | preparing |
|---|---|---|---|---|---|
| 1 | 928 frames, 188.6 s | about 277 s | 112 ms | 36.6 ms | 50.1 ms |
| 2 | 960 frames, 237.7 s | about 315 s | 121 ms | 40.3 ms | 53.2 ms |

Each run held once and let go on an open picture; no hold reached the cap, no run lost the device.
Both ended in the same place, where the player can move and drive (checked by eye on a shot). The
game ran at about 4 fps while Enter was held.

## Not measured

- Black samples are not logged, so whether one hold skipped one cutscene or several is not known.
- A cutscene without bars is not seen; the taps are the only presses then.
- Enter stays held for up to three samples (about 24 frames) after a cutscene ends.
- The cost of the row copies was not measured. They are about 8 rows of 15 KB at 3840 wide every
  8th frame, recorded into a list that is sent anyway, and never waited for.
- The release build with the switch on has not been run with the game yet; the figures above are
  from the diagnostics build. The code it runs is the same.
