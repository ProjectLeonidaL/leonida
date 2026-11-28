# UP-5: an estimate of the indirect light where the title's rays find nothing (experimental, off by default)

Ported from [trustno1-coder/VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA), commits
1316d08 (the stand-in) and 826ba15 (violet garbage of the probes' memory dropped), applied with
`git cherry-pick` and `scripts/rename-from-upstream.ps1`. Upstream's `VICTORY_AMBIENT` and
`VICTORY_AMBIENT_SHADE` are `LEONIDA_AMBIENT` and `LEONIDA_AMBIENT_SHADE` here. That a stand-in can go
where it goes, the shade in creases and the constant are beloooo's (Belobug/VIctory-Belobug-AMD
06031237, 4f45ab88, cd9f78ce), as upstream credits them.

## What it changes

With the ray-tracing structure builder off (the default; `LEONIDA_BVH_BUILDER=1` asks for it) the
title's rays find nothing. Its program that resolves the diffuse light of its screen probes then
leaves the sky's light outdoors, nothing indoors (walls stay black where no lamp or sun reaches) and,
in blocks of tiles, whatever the probes' memory held. With `LEONIDA_AMBIENT` set
(`crates/leonida-gpu/src/ambient.rs`):

- after the title's program that keeps the light visible surfaces send out (for its own screen
  traces, the frame before), that light is averaged in 64x36 cells, with a slow mean of the last
  seconds beside it;
- right after the resolve, the cells are gathered at 160x90 points as a directional value, each
  pixel takes it between the points around it by depth, with its normal and a shade in creases, and
  the resolve's output is raised to it. What the program left counts up to four times the estimate.

| value | does |
|---|---|
| unset, `0`, `off` | the title's own output, as before |
| `<share>` | that share of the estimate (`1`: all of it) |
| `constant:<level>` | the same light everywhere, as beloooo's fork has it |

`LEONIDA_AMBIENT_SHADE=<strength>,<metres>` sets the creases (`1,0.6`). The stand-in is off while the
builder runs.

Which two programs of the title it follows is not in the code (upstream finds them by their names,
which are the title's): they are named at run time, the way sanchaton's branch of the same port
does it (sanchaton/leonida-legacy, R-11):

`LEONIDA_AMBIENT_PROGRAMS=resolve:<program>;radiance:<program>` (`,` also separates), where a
program is the 16-digit shader hash the log's "first bind" lines print, or a name its container
holds (8 characters or more). A role may be named more than once. An entry it cannot read is left
out with a warning; with `LEONIDA_AMBIENT` on and no program named it warns once and does nothing.

The images are taken from fixed places in their tables (upstream's). Since the programs are named by
hand, what is bound there is checked before anything is recorded: an unordered view of a 2D image in
the expected format, and for the depth a shader view that is not a stencil view, of the image's
size. Where the title binds only the stencil at the depth's place, the depth target of the image's
size that the program also reads is taken instead. Both are sanchaton's. When a table or an image is
not what is expected, it logs that once and records nothing.

## Not ported

Upstream's later work on the same module, each a pull request of its own:
`VICTORY_AMBIENT_AIR` (6c105a5), `VICTORY_AMBIENT_ONCE` (8cf0a75) and the stand-in for reflections,
`VICTORY_AMBIENT_SPECULAR` (2db59d4, 9f6fb77). There is no profile for it yet: one goes in with
figures from runs.

## Measured

Not in a run yet. `cargo test -p leonida-gpu ambient` on an RTX 3060 Ti 8 GB, driver 621.24: the five
passes compile with the system's compiler and the host takes their pipelines.

Upstream's ray-tracing document puts the estimate at next to nothing a frame, against some 40 ms
more of the card's time a frame for the builder. That was not measured here.

## What it risks

- Wrong light where its assumption fails: what is off screen is taken to send out what is on
  screen. The slow mean keeps a turn into a dark corner from going black at once.
- A title build that moves the images in those tables leaves it doing nothing (logged once).
- It records on the title's list right after the program's dispatch and replaces the list's compute
  root signature and pipeline; the next dispatch binds the title's again.

## How to try it

Put the switches in an opt-in profile in `gpu-profiles.local.json` (git ignores it), for example
`ambient` with `LEONIDA_AMBIENT=1` and `LEONIDA_AMBIENT_PROGRAMS`. The programs' hashes and names come
from your game: keep them in that file, never in a commit, an issue or a post. Compare `scripts/measure-run.ps1` runs with and without it in
the same series folder, twice each. `-Profile` replaces the configured list, so name every profile
in both runs. Look at rooms lit only by lamps in the shots, and at the frame time in `summary.txt`.
