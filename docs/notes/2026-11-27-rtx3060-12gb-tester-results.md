# Cards measured: RTX 3060 12 GB

Reports from two testers from the report form, as they gave them. Nothing here quotes game data.
axel_cf attached a file to their first report; it is not added here (logs can quote game data).

- axel_cf: RTX 3060 12 GB (12,115 MB), Core i5-10400F, 16 GB RAM, Windows 11 Pro 26H2 build
  26300 (given with run 3). Driver 621.01 for runs 1 and 2, 621.40 for runs 3 to 5. Build unknown
  for runs 1 to 3 (no git); runs 4 and 5 used a build with the S-6 fix
  ([S-6 note](2026-11-27-cbv-64k-limit.md)).
- Lepsy06: RTX 3060 12 GB (12,108 MB), Core i5-13600KF, 32 GB RAM, driver 621.01, Windows 11 Pro
  for Workstations 25H2 build 26200. Build unknown (no git). `base` applied (the eight speed
  switches on), shader warm-up done, prison hall.

Driver 621.01 is older than the 621.14 to 621.40 on the other cards reported so far.

| Tester | Run | Span | Frames | fps | Median frame | Longest frame |
|---|---|---|---|---|---|---|
| axel_cf | 1 | last minute before closing | 508 in 60.8 s | 8.4 | 58 ms | 1,028 ms |
| axel_cf | 1 | whole run, menus and loading included | 2,863 in 696.3 s | 4.1 | 36 ms | 11,204 ms |
| axel_cf | 2 | last minute before closing | 200 in 64.9 s | 3.1 | 248 ms | 2,032 ms |
| axel_cf | 2 | whole run, menus and loading included | 6,558 in 1,206.3 s | 5.4 | 149 ms | 10,532 ms |
| axel_cf | 3 | last minute before closing | 115 in 61.2 s | 1.9 | 282 ms | 2,590 ms |
| axel_cf | 3 | whole run, menus and loading included | 4,002 in 816.2 s | 4.9 | 135 ms | 9,788 ms |
| Lepsy06 | 1 | last minute before closing | 264 in 62.1 s | 4.3 | 238 ms | 754 ms |
| Lepsy06 | 1 | whole run, menus and loading included | 6,687 in 1,188.7 s | 5.6 | 33 ms | 23,369 ms |

- axel_cf run 1: switches, warm-up and scene not given. Its 58 ms last-minute median may not be
  world frames (the menus sit at about 33 ms), so it can't be compared with run 2.
- axel_cf run 2: `base` applied, shader warm-up done, first room and the flashback. No crash
  report (exit code 0). The tester saw textures missing and flashing, and a freeze.
- axel_cf run 3: the same as run 2 (`base`, warm-up done, the prison then the flashback, ending
  in the flashback) after updating the driver to 621.40. It ended with a crash report and exit
  code -2005270523 (`0x887A0005`, the device removed), in the flashback; the tester says runs on
  621.01 never crashed and got further into the game. They also saw textures load more slowly
  and flash. The median went 248 -> 282 ms.
- Lepsy06's run ended with a crash report and the same exit code, `0x887A0005`: the card dropped
  out of the run. It held about 4 fps before that, and the other characters' textures were still
  loading. Another device removal in the notes is a laptop with `LEONIDA_TEXTURE_RANGE_LOCK=1`
  ([P-4 note](2026-11-27-p4-texture-range-lock.md)), a switch not on here.
- axel_cf's crash report has no removal reason. Its last error is a host command list that did
  not close, with `0x8007000E`. That code is `E_OUTOFMEMORY`: D3D12 returns it from `Close` when
  memory ran out while the list was recorded, and an invalid command gives `E_INVALIDARG`. Our
  message (`crates/leonida-gpu/src/host.rs`) says "a command recorded into it was invalid" for
  every code, so here it points the wrong way. This PC has 16 GB of RAM, and the RTX 3060 Ti
  8 GB with 16 GB in [../fork-notes.md](../fork-notes.md) used 22 GB of private memory in the
  first room, so running out of memory fits. The tester later gave RAM at 97% for run 3.
- So this card has lost the device on both drivers, 621.01 and 621.40, with `base` and after the
  warm-up. Lepsy06's report gives no reason, and axel_cf's points to memory, not to the constant
  buffer view size that an RTX 5080 run hit (reason invalid call). The two crashes may not share
  a cause. One crash after one driver update doesn't show the driver caused it.
- Lepsy06's 238 ms on an i5-13600KF is close to axel_cf's 248 ms on an i5-10400F, in different
  scenes. A much faster CPU gave about the same frame time, so the CPU looks less likely to be
  what holds this card back (a guess). axel_cf's 248 -> 282 ms after the driver update is one
  run each, and run 3 crashed, so it doesn't show the newer driver is slower.
- Nothing here says what any switch does on this card: every run with known switches had the same
  ones.

## With the S-6 fix (axel_cf, runs 4 and 5)

Both runs on driver 621.40, with a build that caps constant buffer views at 64 KiB. Only the
figures below were given: no frame counts, spans or longest frames.

- Run 4 ended with an access violation (`0xC0000005`) in the opening cutscene. The tester saw a
  white flash. Its crashes.log shows a write to address 0 in the title image, on a title thread,
  with no Leonida frames on the stack. That fits an allocation that failed and a null pointer the
  title went on to use (a guess: the log doesn't say which allocation).
- Run 5 lost the device (`0x887A0005`) in the flashback, the same place as run 3 before the fix,
  with a median frame of 311 ms (run 3: 282 ms; the span of the 311 ms was not given). Its last
  error is the same as run 3's: a host command list that did not close, with `0x8007000E`, and
  the localized system text says there is not enough memory.
- The page file is system managed, and the PC has 16 GB of RAM.
- So on this PC the crashes are memory, not the constant buffer view S-6 fixed: both runs with the
  fix failed, one of them with the same out-of-memory error as before it.
- Lepsy06 (32 GB of RAM, same card) has not run a build with S-6 yet, and is the useful S-6 test
  on this card: a run there would show whether S-6 stops their removal, and whether the removal
  follows the amount of RAM.
