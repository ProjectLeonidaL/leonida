# Guide for coding agents

Read this before changing anything. [TODO.md](TODO.md) is the work list, [docs/asset-policy.md](docs/asset-policy.md)
the rule that matters most, and [docs/architecture.md](docs/architecture.md) how the crates fit together.

## What this is

Leonida Legacy runs the Xbox Series build of Grand Theft Auto VI natively on Windows, from the user's own copy. The
game's x86-64 code runs unchanged inside our process; we supply the console platform under it: the loader, the
runtime services, a Direct3D 12 back end and a shader recompiler (console GPU code to DXIL, on first use, cached).

The goal is a console-like experience on PC: straight into play, a steady 30 fps (33 ms a frame) or better, the
console's picture, shaders and textures that load in as smoothly as on the retail console (no hitch the first time
something is seen, no late or blurry textures), no stutter, no crashes.

**For everyone's PC, not just yours.** Many projects like this one are tuned on the developer's own machine and
run badly everywhere else. This one aims to run as well as it can on whatever hardware the user has, and to be usable
by people who can't code and have no AI assistant. In practice:

- A switch that helped on your card goes into `gpu-profiles.json` as a profile that matches your card (or opt-in),
  with its figures, not into `base` and not into the run scripts. `base` holds only what helps on every card measured.
- Never make another card slower or less stable to make yours faster. If you could only measure on one card, say so.
- Whatever a user has to do goes in the README in plain steps they can copy: one script to run, no environment
  variables to set by hand, no knowledge of Rust, git internals or the code assumed. Error messages say what to do.

It is a fork of [fxceless/victory-gta6](https://github.com/fxceless/victory-gta6), itself a fork of
[Marvisak/VIctory](https://github.com/Marvisak/VIctory), renamed to Leonida Legacy. GPL-3.0-or-later.

## Hard rules

- **No game data, ever.** No assets, executables or parts of them, shaders in any form, disassembly, dumps,
  screenshots, keys, or logs that quote any of it, in commits, issues, pull requests or test fixtures. Tests build
  their input with our own code, or read real files from a path in an environment variable and skip when it is unset.
  The one exception is `crates/leonida-formats/src/bcpack-tables.bin`; no new ones.
- **Derived data stays outside the repository.** Anything that writes caches, converted data or logs goes through
  `leonida_core::ensure_outside_repo` (or `Config::cache_subdir` / `data_subdir`, which call it).
- **The repository is text only.** `cargo xtask policy` allows these extensions: `rs toml md txt lock yml yaml json
  hlsl hlsli ps1 cmd sh`, files under 512 KiB, no long hex or base64 runs. No Python files: write helpers in Rust
  (`xtask`) or PowerShell.
- **Keep the credits.** Links, upstream commit and issue references (`upstream VIctory 86a5ed6`, `VIctory #1122`) and
  the README's credits name VIctory and victory-gta6 on purpose. Don't rename them, and don't remove the license or
  copyright notices.
- **Offline by design.** The title must not reach the network.
- **Never touch `leonida.toml`** in a commit. It is the user's own config and git ignores it.

## Before you start

- Pick an item from [TODO.md](TODO.md). Search the open pull requests on
  [ProjectLeonidaL/leonida](https://github.com/ProjectLeonidaL/leonida) for its ID; if someone has it, pick another.
- Claim it by opening a draft pull request with the ID in the title (`P-3: ...`) as soon as you start.
- One item, one pull request. Touch only the files the item needs. The pull request that finishes an item deletes
  its row in `TODO.md` and edits no other row. New work goes in as a new row with the next free ID; never renumber.
- A pull request that moves an item forward without finishing it may edit that item's row only: new figures and a
  link to its note. The findings themselves go in the note under [docs/notes/](docs/notes/), never in the row.
  Keeping each pull request to its own row is what keeps `TODO.md` from conflicting.
- Read the code, [docs/notes/](docs/notes/) and the older [docs/fork-notes.md](docs/fork-notes.md) before guessing:
  many things were already measured there. Quote where a number comes from, or say it is a guess.

## Where things are

| Path | What |
|------|------|
| `apps/leonida` | the `leonida` command: `run`, `config`, `translate`, `inspect`, `census`, `shader-info` |
| `crates/leonida-core` | config (`leonida.toml`, `LEONIDA_*` env), the asset-policy guard, logging |
| `crates/leonida-loader` | prepares, maps, binds and starts the title image; crash reports |
| `crates/leonida-hle` | console APIs: files, memory, audio, input, saves, DirectStorage |
| `crates/leonida-gpu` | D3D12.X objects, command lists and title memory on host D3D12 |
| `crates/leonida-shader` | console shader containers to DXIL |
| `crates/leonida-formats` | texture layouts and detiling, audio codecs, BCPack |
| `crates/leonida-pe`, `-pipeline`, `-game` | PE reading, the converter cache, game-specific formats and build ids |
| `xtask` | `cargo xtask policy` and `cargo xtask install-hooks` |
| `scripts/rename-from-upstream.ps1` | renames code taken from upstream (`victory` to `leonida`) |
| `run-gta6.ps1` | builds and starts the game with the frame-rate switches for this PC's graphics card |
| `scripts/measure-gta6.ps1`, `start-measure.cmd` | the same with the diagnostics build and logs; a double-click start for testers |
| `gpu-profiles.json`, `leonida-core::graphics` | measured profiles embedded in the executable, and native settings resolution |
| `scripts/leonida-settings.ps1` | launcher queries and reports for the native graphics settings |
| `scripts/measure-run.ps1`, `scripts/make-view.ps1` | one unattended diagnostics run with frame shots and a summary of its figures; small JPEG copies of shots |
| `docs/` | architecture, asset policy, ray tracing, the fork's measurement log (`docs/notes/`, one file per change; `docs/fork-notes.md` up to 2026-11-27) |
| `.claude/skills/` | agent skills, copied in `.agents/skills/` for other agents; [AGENTS.md](AGENTS.md) lists them. Edit both copies |

## Commands

Windows only; the toolchain is pinned in `rust-toolchain.toml`. None of these need the game except the last two.

```powershell
cargo xtask install-hooks                                   # once: asset-policy check on every commit
cargo check --workspace --all-targets --locked
cargo test -p <crate> --locked                              # while you iterate
cargo test --workspace --locked                             # before the pull request
cargo clippy --workspace --all-targets --locked -- -D warnings                 # what CI runs
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings  # CI, diagnostics build
cargo xtask policy
cargo build --release -p leonida; target\release\leonida.exe run --execute    # asks for the game folder once
cargo build --profile diag -p leonida --features diagnostics                   # debugging aids, target\diag
```

If `cargo xtask` fails with "The directory name is invalid" after the folder was moved, run `cargo clean -p xtask`.

Each worktree builds into its own `target`: the run and measure scripts look for `target\release` and `target\diag`
in their own checkout, so don't point `CARGO_TARGET_DIR` elsewhere.

## Config

- One file is read: the one `LEONIDA_CONFIG` names, else `leonida.toml` in the current folder. Nothing is merged, so
  each worktree reads its own copy unless `LEONIDA_CONFIG` is set. `gpu-profiles.local.json` is read from the folder
  of that file.
- A `LEONIDA_*` variable beats the file's `[graphics]` value, which beats the profiles (`leonida-core::graphics`).
  `leonida config --graphics` prints what a run would use.

## Test failures

Clippy passes on `main` since #20. The Windows runner's depth-bounds failure was fixed in #41.
The C-2 indirect-budget test now checks lane order within each group without assuming which group
runs first; controlled submissions cover both possible winners.
Compare against `main` before and after your change; a pull request must not add a new failure or
warning. Fixing an unrelated failure is its own item, not something to fold into another pull request.

## Conventions

- **Formatting:** `rustfmt.toml` sets a width of 100, but the tree isn't formatted yet (TODO C-3). Don't run
  `cargo fmt` on the whole tree; it rewrites almost every file and conflicts with every open pull request. Keep the
  lines you write within 100 columns and formatted the way `rustfmt` would.
- **Match the surrounding code:** its naming, its comment density and its plain-prose comments that say why.
- **Switches:** a new behaviour that might break a picture or lose the device goes behind a `LEONIDA_*` environment
  variable, off by default, described in a comment where it is read and in a note under `docs/notes/`.
- **`shortcut:`** marks code that approximates what the console does. Add the marker when you approximate; remove it
  when you replace one with the real behaviour.
- **Unsafe:** `unsafe_op_in_unsafe_fn` is denied. Give every new `unsafe` block a `// SAFETY:` comment.
- **Line endings:** `.ps1` and `.cmd` files are CRLF, everything else LF (`.gitattributes`).

## Measuring

- Measure on your own machine before and after, in the same scene, and put both figures in the pull request with
  your CPU, GPU and driver. Most numbers in the docs come from one RTX 4070.
- Every figure you write, in `TODO.md`, a note or a pull request, names the card, the resolution, the scene and
  where it came from: a frame time measured on one card says little about another. If the source doesn't name the
  card, say so. Shares of a frame ("about half the card's time") carry over better than ms.
- Post figures, never log files: logs can quote game data.
- Write what you measured or changed in a new file under `docs/notes/` ([how](docs/notes/README.md)), not in
  `docs/fork-notes.md`, which is closed to new entries. One file per pull request keeps notes from conflicting.
- The first run after a change to `leonida-shader` re-translates every shader (the cache keys on the translator's
  source), so it stutters. Don't measure that run.
- `-Profile` (`LEONIDA_PROFILE`) replaces the profile list in `leonida.toml` instead of adding to it (`base` always
  stays), so name every profile you use: compare `-Profile nvidia-8gb` with `-Profile nvidia-8gb,mine`, not with
  `-Profile mine`. Shader switches are part of the shader cache key too: warm the cache again after changing one.

## Picture and feel: unattended runs

For work on shaders, textures, a wrong or black picture, or anything that brings the game closer to how it looks
and plays on the console, test it with unattended runs before and after. No one has to play: an agent can run,
measure and look at the result itself. This is how levels on demand was found to draw a wall that stayed black
on an RTX 3080.

1. Build the diagnostics build (`cargo build --profile diag -p leonida --features diagnostics`). After a change to
   `leonida-shader`, warm the cache first (`target\diag\leonida.exe translate --warm all`, about 3 minutes).
2. A run of `main` and a run of your change, in the same series folder (about 6 minutes each):

   ```powershell
   $root = Join-Path $env:LOCALAPPDATA 'leonida\runs\my-change'
   powershell -ExecutionPolicy Bypass -File scripts\measure-run.ps1 -Root $root -Label 01-main
   powershell -ExecutionPolicy Bypass -File scripts\measure-run.ps1 -Root $root -Label 02-mine
   ```

   A change behind a switch needs no second build: put the switch in an opt-in profile in
   `gpu-profiles.local.json` (git ignores it) and pass `-Profile <name>`.
3. Compare the figures in `$root\summary.txt`, and look at the shots side by side: `cap\world-600.view.jpg` and
   `cap\world-1000.view.jpg` of each run, taken that many frames after the world's first frame. The scene can still
   differ a little between runs, and one run each varies by about 5 ms a frame: run twice before calling a
   difference real.
4. If it helps (a better picture, or a shorter frame) and no run lost the device or wrote a crash report, open a
   pull request with the figures from `summary.txt` and what the shots showed, in words. Never attach the shots
   or logs: they are game data.

## Upstream

"VIctory" means [Marvisak/VIctory](https://github.com/Marvisak/VIctory), the original project. Many forks on GitHub
share the name; the other forks worth porting from are listed by owner in the Upstream section of `TODO.md`.

A fresh clone has neither upstream as a remote. Add them once:

```powershell
git remote add upstream https://github.com/fxceless/victory-gta6.git
git remote add victory https://github.com/Marvisak/VIctory.git
```

To bring a fix in:

```powershell
git fetch victory
git cherry-pick <hash>
powershell -ExecutionPolicy Bypass -File scripts\rename-from-upstream.ps1
cargo check --workspace --all-targets --locked
```

One upstream commit (or a few related ones) per pull request, with the upstream hash in the description.
The forks in `TODO.md` work the same way: add the fork as a remote, cherry-pick (or apply its patch), rename.

A commit is worth porting when it does at least one of these, and the pull request shows it:

- fixes a crash, a hang or a device loss;
- brings the picture closer to the console (a missing effect, a black or wrong surface);
- shortens the frame or removes stutter, measured before and after on your card;
- makes the game easier to start for someone who can't code (a clearer error, a check at start).

And it follows these rules:

- It makes no card slower or less stable. A switch that helps one vendor or card goes into a matching profile in
  `gpu-profiles.json`, not `base`. If you could test on one card only, say which, and ask someone with another
  vendor's card to check before it is turned on by default.
- Anything that leaves console work out to gain speed (ray tracing off, effects dropped, the 30 fps interval
  lifted) goes behind a `LEONIDA_*` switch, off by default, in an opt-in profile.
- Leave out downloaders, launchers, overlays, telemetry, anything that reaches the network, and Python files.
- Skip pure refactors and renames in code this fork has changed: they cost conflicts and change nothing a player
  sees.

victory-gta6 replaced its tree with VIctory-NVIDIA's (`5dbf5ce`) after this fork branched off, so its launcher
switches, ray tracing among them, belong to that tree (UP-5). In ours, `LEONIDA_BVH_BUILDER=1` lights nothing yet
(R-1, [docs/fork-ray-tracing.md](docs/fork-ray-tracing.md)).

## Commits and pull requests

- Conventional Commits, short and specific: `fix(gpu): ...`, `feat: ...`, `docs: ...`.
- Pull requests go to `ProjectLeonidaL/leonida`, branch `main`. Put the TODO ID in the title, say what you tested
  and on which GPU and driver, and say if it was AI-assisted.
- `cargo xtask policy` checks the files you commit, not commit messages, pull request text or comments. Those hold
  figures and your own words only: no log lines, and no paths or names from the game.
