# Title bytes under flights' writes compared with AVX2, on threads of their own (LEONIDA_AVX2_COMPARE)

`LEONIDA_AVX2_COMPARE=1` (off by default; after upstream b620284, its twin a69eb0c, in
trustno1-coder/VIctory-NVIDIA, which carries readmet3xt-lab/foxhound's history): with
`LEONIDA_LATE_WAIT` at stage 2 or more (`late_exact`, also on with the relay),
`Shadow::merge_under_writes` compares the pages the title wrote under
flights' shader writes with AVX2, 64 bytes a step, and the pages of one range on threads kept for
it (`leonida-under-*`). Before, every page was read and compared one after the other on the
submitting thread. `LEONIDA_UNDER_THREADS` (1 to 16, 6 by default, the submitting thread among
them) says how many threads compare; 1 keeps the AVX2 compare on the submitting thread. A range of
two pages or fewer stays on the submitting thread.

What is the same: the compare finds exactly the runs the byte by byte one finds (a unit test
checks it on synthetic data), the runs are joined over `LEONIDA_MERGE_GAP` and counted against
`LEONIDA_MERGE_RUNS` as before, and the runs are noted for the card and the pages marked as taken
on the submitting thread, in page order. What differs: the lock of the copy's reference is held
for all the pages of a range at once, not page by page, so the copy-back thread waits for the
whole range; and when the run cap is reached, pages that other threads finished meanwhile stay
done (as pages done before the cap always did).

The threads are not those of `wide`: the thread that waits there takes whatever job comes, and a
job of the copy-back's may take the lock the merge holds.

## What was not ported

- Upstream compares whole words, for the words it sends up to the card as (offset, word) pairs
  made on these threads (its `host/send.rs`). This tree has no such pairs: the changed bytes go up
  as copies of their own, so the compare here stays byte-exact and there are no pairs to make.
- Upstream's `VICTORY_WIDE_SPIN` (off there by default), its fast-core stage 3 for helpers that work
  ahead (this tree has no `cores.rs`) and its traffic statistics change.

## Measured

RTX 3080 12 GB, driver 621.24, Ryzen 7 7800X3D, 2026-11-27. Diagnostics build, Series X, profile
`base`, four unattended runs of `scripts/measure-run.ps1` in one series, off and on taking turns
(`-SkipScenes -Seconds 450 -From 320 -ShotAfter 1500`). `LEONIDA_LATE_WAIT` was not set: the relay
is on by default here, which puts late waits at stage 3, and every run logged the stage 2 report
of title bytes put in under flights' writes. Frame figures are in ms.

| run | switch | world at | median frame | usual frame | GPU | preparing | copying back |
|-----|--------|----------|--------------|-------------|-----|-----------|--------------|
| 01 | off | 82 s | 134 | 131.5 | 48.7 | 62.6 | 22.0 |
| 02 | on | 78 s | 145 | 140 | 53.6 | 62.4 | 23.8 |
| 03 | off | 76 s | 131 | 126 | 50.8 | 55.4 | 21.8 |
| 04 | on | 76 s | 137 | 132 | 59.8 | 55.0 | 21.1 |

Every run was closed at the time limit; none lost the device, crashes.log did not grow, and the
cutscene skip held Enter once (twice in run 04). Not faster: both runs with the switch were slower
than both without it, by 3 to 14 ms of median frame, which is inside the 5 to 25 ms these runs vary
by, so it is not called slower either. Run 02 drew more (4,318 draws a frame against about 3,900),
so its scene differed a little. Off by default stays.

On a Ryzen 7 7800X3D, synthetic 64 KiB pages (best of 200, release
build): the compare took 5.5 us byte by byte and 1.2 us with AVX2 for a page that did not change,
4.6 and 1.8 us with one byte in 256 changed, and 19.2 and 11.7 us with one byte in 16 changed.
Upstream counted 12 MB compared a frame, in 80,000 runs; run 219 here had some hundred pages a
frame in 38,000 runs. At that size the compare alone is about 1 ms of the submitting thread a
frame; reading title memory (the kernel's copy, about 3 GB a second in run 225) is likely more,
and that is what the threads share. The guess before the runs above was a few ms a frame at most;
the runs could not show a gain of that size.
