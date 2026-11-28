# Exact integer values in owned volume views

The ordinary image path declares float resources even when the native view is an integer
texture. An original, asset-free program reproduced a lost high bit on an RTX 5090: a
literal `0x80000000` stored through the float path into `R32_UINT` became zero. A proper
integer store preserved it. Runtime values and the other tested special bit patterns did
not reproduce that loss on this driver; the test keeps those as independent controls.

`LEONIDA_UINT_IMAGES=1` adds a bounded, opt-in correction. It is off by default. A static
ordinary Texture3D load of R, or a full-mask store, gets mutually exclusive float and
integer paths. The host selects integer access only from the final explicit native
`R32_UINT` view in a private descriptor allocation owned by the recording. The allocation
and its heap survive repeated submissions until that recording is reset or dropped.
Descriptor rewrites, null/fallback creation, release and reuse invalidate its class.
The store retains the original EXEC guard. It supplies four defined raw operands with
DXIL's full typed-store mask; the admitted scalar `R32_UINT` view consumes X.

Fixed descriptor-address selection ignores declared stride metadata: the actual static
address, root/table offset, category and eight-word descriptor determine the binding.
The nonzero-stride correction does not change native-view validation, invalidation or
recording retention. Dynamically selected descriptors still retain the legacy path.

The admission rules require one mip, with default SRV component mapping for loads or a
complete UAV depth range for stores, and no 16-bit coordinates/data or dynamic descriptor
selection. Ordinary mirrored tables, typeless resources, inferred descriptions and other
view shapes retain the legacy path. Root class words share the existing allocator and cache
encoding; malformed manifests and a root signature exceeding 64 DWORDs are refused.
No program hashes or game-specific bindings select the correction.

The new source and cache encoding invalidate older translations even with the switch off;
the two authored disabled emissions remained byte-identical to the base. The explicit
64-DWORD check also applies with the switch off and rejects root signatures that already
exceed the native limit. No descriptor allocation is pinned by this feature when it is off.

## Measurements

Core i9-14900KS, 64 GB RAM, RTX 5090, NVIDIA 580.173.02, Proton 11.0. The actual Vulkan
device was checked separately from DXGI's reported adapter identity. The device test used
an isolated hidden display without sound or network access.

| Authored 4 by 2 by 13 volume | Legacy float path | Integer path |
|---|---:|---:|
| Literal high-bit stores, SRV-input variant | 104/104 wrong words | 0/104 wrong words |
| Literal high-bit stores, UAV-input variant | 104/104 wrong words | 0/104 wrong words |

The final fixture first validated all 16 authored shader variants with the Windows SDK,
before creating a device. With nonzero image stride metadata, it then ran 32 cases:
separate uploaded image loads and computed, runtime-buffer or literal stores, each with
SRV/UAV inputs and float/integer controls. All 3,328 checked
integer-path words matched. The float controls reproduced the two failures above; their
other checked words matched. Post-fence checks exercised real native view creation,
category mismatch, null replacement, failed creation, recreation and allocation release.
No GPU fault occurred; the only validation identifier was the deliberate canary. The Rust
test reported 4.09 seconds, which is not a gameplay performance measurement.

The GPU fixture uses the real descriptor creators and numeric-class query, then supplies
the root constants directly. It does not exercise `State::bind`, asynchronous retirement
or repeated submission end to end. A separate recording-retention CPU regression passed
under Windows/Proton, alongside source review. Keeping eligible tables until reset can
increase descriptor pressure while enabled; that cost has not been measured in gameplay.

The corrected shader source passed 221 native tests, with two existing ignores, and
eight production cache-codec tests passed. A new regression failed before the stride
admission correction and passed afterward; fixed descriptor addresses with zero and
nonzero declared stride produce identical bindings and emission. The typed-store mask
regression also failed before the correction and passed afterward.

Windows workspace checking, strict Clippy with default and all features, and all
workspace test compilation passed using `cargo xwin` on Linux, with
`blake3/prefer_intrinsics` for the cross-build. Asset policy passed for 306 files.
Two exact SDK-only Windows/Proton tests passed without skips in 1.841 seconds, with
devices, display, audio and network disabled: all 16 variants validate, and the SDK
rejects an authored incomplete-mask negative control with the expected diagnostic.
The explicit `LEONIDA_DXIL_VALIDATOR` path was pinned and its native load verified.
The hardware test now requires SDK validation rather than relying on driver acceptance.

The prior revision passed thirteen Windows/Proton host, cache and authored-emission
tests; those are not repeated-test claims for the final executable. Its unchanged host
contracts and cache handling also received independent source review. The earlier two
authored disabled programs matched the base bytecode; four enabled/disabled conversions
passed SPIR-V validation. The final disabled corpus completed in both cache scopes:
all 25,754 emitted blobs were byte-identical to the baseline, and all 25,953 explicit
outcomes matched. The enabled corpus was stopped at the user's request after processing
more than 23,000 containers in its first scope; its final result and second scope are
unverified. Recovery of every rejection found in the preceding revision is not claimed.
Linux-native workspace/GPU tests encounter the same Windows-only reference errors as the
exact base; compilation and focused tests are not full workspace test execution.

## Gameplay boundary

This is a reproduced integer-storage correction, not an artifact-free gameplay result.
The final enabled run reached the opening courtyard and stopped after the completed
world+16 image, about nine minutes including loading. Host binding diagnostics recorded
at least 8,192 integer-class selections and 4,096 legacy selections; these are host
decisions before root constants, not proof of GPU pixel ownership. The final image
remained washed out and blurred. The preceding revision's disabled short run also showed
corrupt lighting. These are not same-binary ON/OFF gameplay measurements or a warm-cache
performance comparison.

Both short runs completed without a GPU fault and their test resources were retired.
Pre-existing game validation findings remained. A longer unmodified-main run previously
hit a GPU timeout, so the repository's full world-600/world-1000 protocol and long-session
stability remain unverified. The user requested all further tests stop. Dynamically
selected descriptors and mirrored tables remain outside this correction's scope.
No gameplay frame-rate gain or fix for the visible artifacts is claimed. R-9 and the
other rendering TODO rows remain open.

No existing TODO row is closed by this narrowly scoped fix. All programs and values in
the committed regression tests are original; game-derived captures and shader output
remain outside the repository.
