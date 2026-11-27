# R-1: Read view addresses from the existing slot table

Heap-picked descriptor address reads are behind `LEONIDA_VIEW_ADDRS=1` and off by default.
They use the address words already present after the strides in each heap's `SLOT_STRIDES`
upload buffer; no additional 16 MiB per-heap buffer is allocated. Ray-tracing acceleration
structure views use their console `Location` in 256-byte units (`Location >> 8`) in both the
heap-slot address table and descriptor-table reads. The shader-cache key includes the switch.

Paired diagnostics runs used `scripts\measure-run.ps1 -SkipScenes -Seconds 330 -From 170`,
Series X, the same opening path and fixed frame captures. Machine: RTX 5060 Ti 8 GB, Ryzen 5
9600X, 32 GB RAM, NVIDIA driver 617.42, Windows 10 IoT Enterprise LTSC 2024 (build 26200).
Both settings selected `nvidia-8gb`; the switch was unset for the first two runs and set to 1
for the latter two:

| Setting | Run | World at | FPS | ms/frame | Usual / median frame ms | GPU / preparing / copy ms | Draws | Outcome |
|---|---:|---:|---:|---:|---:|---:|---:|---|
| off | 1 | 79 s | 7.0 | 141.8 | 61 / 124 | 45.6 / 54.7 / 19.6 | 4,244 | Full window; no device loss |
| off | 2 | 54 s | 9.0 | 111.3 | 45 / 105 | 44.8 / 44.6 / 18.7 | 3,681 | Device removed; short sample |
| on | 1 | 81 s | 7.6 | 131.2 | 73 / 122 | 52.9 / 48.4 / 17.7 | 4,208 | Full window; no device loss |
| on | 2 | 54 s | 8.7 | 115.5 | 90 / 115 | 49.4 / 38.4 / 14.2 | 4,178 | Device removed; short sample |

These attempts do not establish a stable before/after FPS change: each setting has one full run
and one early device loss, and the short-run medians are not comparable to a complete window.
Device loss occurred with the switch off and on, consistent with the 8 GB pattern discussed in
issues #93 and #106 and PR #106; these runs do not attribute it to this switch.

The 600-frame captures show the same hazy, warm-lit close-up at different poses: the switch-off
shots include a strong eye/cheek shadow, and the first switch-on shot shows the face more fully
lit with a diagonal band of light. The second switch-on shot has a conspicuous red-white blocky
artifact around the chin. The 1000-frame captures are black with a loading spinner. The poses
and lighting vary between captures, so this is a description of the shots, not a claim that the
switch corrected lighting; shadows still need a stationary in-game comparison.
