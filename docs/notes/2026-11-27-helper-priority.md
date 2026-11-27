# Our threads a frame waits for can run above normal (experimental, off by default)

Ported from [readmet3xt-lab/foxhound](https://github.com/readmet3xt-lab/foxhound), commits 002cf4a
(a priority for the threads a frame waits for) and 80c70b7 (the submitting thread's helpers at its
priority), read and written again in our own code. foxhound's `VICTORY_HELPER_PRIORITY` is
`LEONIDA_HELPER_PRIORITY` here.

## What it changes

`LEONIDA_HELPER_PRIORITY=1` raises these threads of ours to above normal priority, ahead of the
title's threads, as each one starts (`leonida_gpu::framelog::raise_helper`):

- `leonida-submit`, the thread that prepares and sends submissions (only with
  `LEONIDA_ASYNC_SUBMIT=1`);
- `leonida-relay` and `leonida-late-wait`, which land and settle submissions sent ahead;
- `leonida-wide-*`, the kept threads that share a submission's refreshes, snapshots and compares
  with the submitting thread (`shadow::wide`).

Any other value, or none, leaves every thread at the normal priority it was made with, as before.

The threads that translate lists (`LEONIDA_THIN=1`) stay at normal, as in 80c70b7. There are eight of
them by default, and above normal they could hold every core of an eight-core processor ahead of the
title's own recording threads. That is a guess; it was not measured here.

Some differences from upstream:

- foxhound's submit thread runs above normal in any case. Ours did not, and runs above normal only
  with the switch.
- foxhound's `VICTORY_AB` can flip the switch while the title runs. We have no such switch: the value
  is read once, and a thread is raised when it starts.
- foxhound's helpers from its landing pools, heap records thread and heap pass beside the lists have
  no counterpart in this tree, so there is nothing of ours to raise for them.
- The short-lived threads that split a single copy or scan of 16 MiB or more (`shadow::copy_wide`
  and the scan beside it) are not raised. They would be raised only once already running, which is
  when the priority helps least.
- 96f2f32 (the wide helpers asked onto the fast cores too) is not ported. `LEONIDA_FAST_CORES`
  still asks only for the submit, relay, late-wait and list threads.

## Why

foxhound's frame timeline of the first gameplay area had such threads woken and not running yet for
4 to 11 ms of the chain the present came out of (the listing threads up to 5.4 ms at a time), while
the title's own workers held every processor. On a six-core i5-12400F the title's threads and eight
list translators kept every core busy, and the submitting thread, above normal, stood waiting for
helpers at normal priority that were not running.

## Measured

RTX 3080 12 GB, driver 621.24, Ryzen 7 7800X3D, 2026-11-27. Diagnostics build, Series X, profile
`base` (so `LEONIDA_ASYNC_SUBMIT=1` and the submit thread is raised too), four unattended runs of
`scripts/measure-run.ps1` in one series, off and on taking turns (`-SkipScenes -Seconds 450
-From 320 -ShotAfter 1500`). Frame figures are in ms.

| run | switch | world at | median frame | usual frame | GPU | preparing | copying back |
|-----|--------|----------|--------------|-------------|-----|-----------|--------------|
| 01 | off | 79 s | 116.5 | 112 | 39.7 | 51.6 | 20.6 |
| 02 | on | 78 s | 114 | 109 | 37.7 | 49.5 | 20.2 |
| 03 | off | 75 s | 114 | 107 | 33.8 | 52.0 | 20.2 |
| 04 | on | 77 s | 112 | 107.5 | 38.5 | 51.2 | 20.7 |

Every run was closed at the time limit; none lost the device, crashes.log did not grow, and the
cutscene skip held Enter once in each. Not called faster: run 02 only ties run 03 (114 ms), so the
runs with the switch do not both beat both without it. The 2 to 4 ms between them is inside the
5 to 25 ms these runs vary by. Off by default stays.

Upstream's figures:

- 80c70b7, i5-12400F with an RTX 3060, Series S at 540p, off / on in one run (medians of 5-second
  windows in the world): 8.6 / 9.6 fps, refreshes 23.1 / 17.6 ms a frame, waiting for heap records
  6.2 / 2.5 ms, card idle behind the submitting thread 24.7 / 13.2 ms.
- 002cf4a, four bench runs with other builds running beside them: within the noise, which is why
  upstream first left it off.

On a CPU with more cores than the title keeps busy (an eight-core Ryzen 7 7800X3D, say) the gain may
be smaller: the helpers are more often free to run anyway.

## What it risks

- Above normal threads of ours can take a core from a title thread at the moment it needs one. A
  title thread the frame also waits for (its render or recording threads) could then run later.

## How to try it

Put the switch in an opt-in profile in `gpu-profiles.local.json` (git ignores it), for example
`helper-priority` with `LEONIDA_HELPER_PRIORITY=1`, and compare `scripts/measure-run.ps1` runs with
and without `-Profile helper-priority` in the same series folder, twice each. Look at the frame time,
"preparing" and "copying back" in `summary.txt`.
