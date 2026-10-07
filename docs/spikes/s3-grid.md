# S3: egui grid performance

Probe: `spikes/s3-grid` (eframe/egui 0.36.1 from the crmne fork, `glow`, release build). Raw output of both runs: `spikes/s3-grid/run1.txt`, `run2.txt`.

## Setup

- Machine: AMD Ryzen 7 7800X3D (8C/16T), AMD Radeon RX 7900 XTX (driver 32.0.31041.1004), Windows 11 Pro 26200. Two 2560x1440 displays.
- Refresh rate: Windows reports 144 Hz, so the budget interval is 6.94 ms.
- Grid: 10 000 cells, 200 px wide cells (200x228 px rows), `ScrollArea::show_rows`. The window was 2560 px wide, so 12 columns and 834 rows (191 297 px scrollable).
- Images: 320x320 JPEG q85 generated per index on a 4-thread worker pool (encode, then decode), only for the visible rows plus 1 row ahead. Decoded to RGBA and uploaded as textures on the UI thread. LRU cap 300 textures.
- Auto-run, no input: load first screen, 2 s settle, 10 s idle, scroll end-to-end at 3 000 px/s, then at 12 000 px/s, print stats, exit. The first 500 ms of each scroll phase is excluded.
- Frame time: `ctx.input(|i| i.unstable_dt)` (the budget check uses this, it is the real frame-to-frame interval). Also measured: CPU time inside `ui()` (does not include GL paint or buffer swap, so it understates the cost).

## Results (2 runs)

| Metric | Run 1 | Run 2 |
| --- | --- | --- |
| Idle repaints in 10 s | 2 | 2 |
| Idle process CPU time in 10 s | 0.000 s (0.00%) | 0.016 s (0.02%) |
| Working set, first screen | 111.0 MB | 99.8 MB |
| Private bytes, first screen | 273.7 MB | 277.8 MB |
| Working set after 3 000 px/s scroll | 125.7 MB | 103.5 MB |
| Private bytes after 3 000 px/s scroll | 451.4 MB | 441.3 MB |
| Working set after 12 000 px/s scroll | 127.4 MB | 105.0 MB |
| Private bytes after 12 000 px/s scroll | 457.6 MB | 445.9 MB |
| Peak working set (OS) | 142.9 MB | 106.8 MB |
| Peak private bytes (sampled per frame) | 465.5 MB | 447.8 MB |

| Scroll speed | Run | Frames | dt p50 | dt p99 | dt max | Frames > 1.5x interval (10.4 ms) | ui() CPU p50 / p99 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 3 000 px/s | 1 | 18 087 | 3.49 ms | 6.53 ms | 10.06 ms | 0 | 0.21 / 0.41 ms |
| 3 000 px/s | 2 | 14 731 | 3.77 ms | 13.56 ms | 43.42 ms | 1 302 (65 over 2x) | 0.21 / 0.42 ms |
| 12 000 px/s | 1 | 3 510 | 4.10 ms | 13.72 ms | 43.94 ms | 329 (28 over 2x) | 0.26 / 0.47 ms |
| 12 000 px/s | 2 | 4 236 | 3.65 ms | 7.08 ms | 8.97 ms | 0 | 0.25 / 0.45 ms |

Frames above the 6.94 ms interval itself (>1.1x): 17 and 1 352 at 3 000 px/s, 367 and 10 at 12 000 px/s (run 1, run 2).

## Observations

- Frames ran at 3.5 to 4 ms p50, i.e. about 250+ fps, faster than 144 Hz. The swap was not pacing to the display, so dt is raw frame cost and not a vsync-quantised interval. Treat the p99 as "time between frames when rendering flat out".
- Idle is clean: no repaint requests, 0.00 to 0.02% of a core. The 2 repaints in idle are not from the app (nothing requests them); likely window or cursor events on the machine.
- The slow frames are not on the CPU side of `ui()` (p99 under 0.5 ms). One run had a bad phase in each scroll speed (p99 13.6 ms at 3 000, 13.7 ms at 12 000) while the other was clean. The stalls are most likely the GL texture uploads and driver work in paint (up to about 600 textures/s at 12 000 px/s), or contention from other processes on the machine. Not isolated further in this spike.
- Private bytes are much higher than working set (about 275 MB at first screen, about 450 MB after scrolling). The growth of about 170 MB tracks the 300 textures of 320x320 RGBA (about 120 MB), so the GL driver appears to commit a copy per texture. The working set stays near 100 to 143 MB.

## Verdict

Idle CPU and repaints meet the budget (0% CPU, no repaint requests). Working set meets idle RSS < 150 MB in both runs (100 to 111 MB at first screen, peak 107 to 143 MB), but private bytes (270 to 460 MB) do not, so the result depends on which counter the budget means; if it is private/commit, textures must be smaller (upload at about 200 px or lower the LRU cap, since a 200 px cell only needs 200x200). The p99 ≤ 6.94 ms check is passed in the clean phases (6.5 ms at 3 000, 7.1 ms at 12 000, marginal) but failed in the noisy phases (13.6 and 13.7 ms, up to 44 ms stalls), so egui plus glow does not reliably hold 144 Hz while fling-scrolling a 12-column grid of freshly uploaded textures. At a 60 Hz interval (16.7 ms) every phase passes. Recommendation: egui is viable for the grid, but the real build must downscale on the worker (200 px textures, about 3x less upload and memory), cap uploads per frame, and re-measure on a quiet machine with vsync confirmed; this spike should be treated as "passes at 60 Hz, borderline at 144 Hz".
