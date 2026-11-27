# R-10: wave64 programs with LDS read and written in one lane branch on 32-lane cards (`LEONIDA_WAVE64_GROUP_WIDE=1`, off by default)

On an RTX 5060 Laptop (at most 32 lanes per wave) three translated compute programs were refused
with `E_INVALIDARG`; the debug layer said they ask for 64-lane waves. One of them dispatches
80x45 groups and writes a 1280x720 R16_UINT image, so leaving it out is a likely cause of a white
veil over a cutscene (not confirmed). `LEONIDA_WAVE_WHY=1` gave for them: LDS in a group of more
than 32 threads with an LDS read and write in one per-lane branch region, some with
`v_readlane`, `v_mbcnt` or masks read as data.

## Why they keep 64

A console wave of 64 lanes runs as two host waves of 32, which exchange cross-lane values
through LDS dwords and meet at a group barrier before every LDS access. A barrier must be reached
by every thread of the group, so it cannot stand inside a per-lane branch. Around such a branch
the waves meet where it starts and ends (`lds_meetings`), but a read and a write inside one
branch region cannot be ordered that way, and the program kept 64 lanes. The existing way out,
taking the branch wave-wide (`LEONIDA_WAVE_WIDE=1`), was limited to groups of one console wave:
with several, each console wave decides on its own and a barrier inside the branch waits for
threads that never come.

## What changed

- `LEONIDA_WAVE_WHY=1` now says which reason forces 64 (`forced-by=[...]`, with the offsets of
  the blocks), and `leonida-gpu` logs it with the program's hash on a `shader keeps 64-lane waves`
  line after `shader translated`. Programs taken from the disk cache are not translated again,
  so clear the cache or warm it with the switch set. It prints only; nothing else changes.
- `LEONIDA_WAVE64_GROUP_WIDE=1` (off by default) lets the wave-wide retry run in groups of any
  size. Each wave-wide branch is then taken by every thread of the group: a constant, never a
  decision that differs between threads, so the barriers inside it are reached by all. That is
  exact only where running the region in a console wave that would have skipped it changes
  nothing, and `forced_wide` checks this for each branch:
  - the branch skips a region: one way is its post-dominator, and there is no loop in the region;
  - every vector instruction in the region is off in every lane that does not want it (EXEC
    stays its guard; checked on the lane conditions with `Bools::meet`);
  - no scalar code in it writes memory or sends messages, and there is no `v_writelane`;
  - nothing after it reads a scalar, SCC or lane mask written in it, except an EXEC holding
    only lanes that wanted the region.
  If any wave-wide branch fails a check, the program keeps 64 lanes as before, and
  `LEONIDA_WAVE_WHY=1` names the reason (`every thread through it: ...`). Lane reads, `v_mbcnt`
  and masks still go through the exchange dwords.

`ir::without_skips` already removes plain `s_cbranch_execz` skips in the IR for the same reason.
The new mode also covers the other skip forms the compiler uses (`s_cbranch_scc0` on the SCC of
`s_and_saveexec`, `vccz`, `execnz`).

## What still keeps 64

- LDS read and written inside a loop whose exit is per lane (a wave-wide branch around a loop);
- a region that leaves a scalar, SCC or non-EXEC lane mask for the code after it, or writes
  memory from scalar code, or uses `v_writelane`;
- programs with an indirect exit.

## Measured

Authored tests only (`emit/group_wide_tests.rs`), on an RTX 5060 Laptop: a 128-thread wave64
program that writes LDS and reads the dword of a lane in the other half of its console wave inside
one `s_cbranch_scc0` branch keeps 64 lanes without the switch and gets 32 with it. Every
group barrier lies on every path through the lowered program. On the card, every thread read its
partner's write as a 64-lane wave would, with 128, 96, 64, 40 and 0 threads in the branch
(including the case where one console wave of the group skips it). Not run in the game: whether
the three programs now get 32 lanes, and whether the veil goes, is not measured.
