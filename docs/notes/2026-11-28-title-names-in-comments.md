# C-7: names of the title's shader programs out of two `leonida-gpu` files

Four doc comments in `leonida-gpu` named shader programs of the title, against the first hard rule in
`CLAUDE.md`: two in `pipeline.rs` (on `LEONIDA_BVH_HOLD`, one with the program's hash beside its name)
and two in `draw.rs` (on the decoration-bake table and on `LEONIDA_LIST_SLOTS`). They are gone, and the
hash with them; the sentences around them say the same without them. No code changed, so nothing was
measured. They came in with three older commits and are still in `main`'s history; the history was not
rewritten.

## What is left

Found by shape only: words of the form `CS_`, `PS_`, `VS_` or `GS_` followed by mixed case, and 16-digit
hex runs in quotes or backticks. A shape does not say whose a word is (some may be our own entry points or
format names), so each needs a look before it goes.

- **Name-shaped words in code:** `leonida-game` (`sgd5.rs`, 5), `leonida-gpu` (`cardmeter.rs`, 3;
  `descriptor/host.rs`, 1) and `leonida-shader` (`emit/container.rs`, `emit/control.rs`,
  `emit/lower.rs`, `emit/own_bits.rs` and `emit/wave.rs`, 1 each; `emit/wave64_tests.rs` and
  `resource.rs`, 2 each).
- **In docs:** `docs/fork-notes.md` (18), `docs/notes/2026-11-27-wave32-far-early-outs.md` (5) and the R-7
  row of `TODO.md` (2).
- **Hashes of the title's programs in code:** in `leonida-gpu/src/draw.rs`, the decoration-bake table (4;
  it picks programs by hash in every build) and the walls trace's culling table (9, each beside a label
  whose origin a shape check cannot tell), and one in a comment in `pipeline.rs` (on
  `LEONIDA_SKIP_PIPELINES`). Quoted hex runs of the same shape are also in
  `docs/notes/2026-11-27-wave32-far-early-outs.md` (1) and `docs/notes/2026-11-27-wave64-lds-single-wave.md`
  (4).

Comments and docs can simply lose them. Code that finds a program by name or hash should read it at run
time from a `LEONIDA_*` switch, the way UP-5's `LEONIDA_AMBIENT_PROGRAMS` takes a 16-digit hash or a name,
with made-up values in its tests. The decoration-bake table changes what every run draws, so moving it
needs a run before and after.
