# S3: egui grid performance

Probe: `spikes/s3-grid` (eframe/egui 0.36.1 from the crmne fork, `glow`, release build). Run: `s3-grid.exe` (baseline) or `s3-grid.exe --mitigated`. The probe runs by itself (load, 2 s settle, 10 s idle, scroll end-to-end at 3 000 px/s, then 12 000 px/s) and prints stats.

Raw outputs:
- `spikes/s3-grid/results/{baseline,mitigated}-{1,2}.txt`: the four runs reported below (vsync requested explicitly, monitor refresh queried, worst-5 frames).
- `spikes/s3-grid/run1.txt`, `run2.txt`: first-round baseline runs, kept for history. They predate the explicit vsync setting, the per-frame upload log and the monitor query, and used a hard-coded 144 Hz.

## Setup

- Machine: AMD Ryzen 7 7800X3D, AMD Radeon RX 7900 XTX (driver 32.0.31041.1004), Windows 11 Pro 26200. Two 2560x1440 displays: DISPLAY1 reports 144 Hz, DISPLAY2 reports 60 Hz (queried per run with `EnumDisplaySettingsW` for the monitor under the window centre, printed in the MODE line).
- The window opened at 2538x1356 (the 1280x800 request and the requested position are not honoured; the OS places it on either display, so the monitor differs between runs, see below). That gives 12 columns, 834 rows, 191 297 px scrollable.
- Grid: 10 000 cells, 200 px cells (228 px rows), `ScrollArea::show_rows`. Images: 320x320 JPEG q85 generated per index on a 4-thread worker pool (encode then decode), only visible rows plus 1 row ahead, uploaded as textures on the UI thread, LRU cap 300.
- Baseline: textures uploaded at 320x320, unlimited uploads per frame. Mitigated (`--mitigated`): the worker downscales to 200x200 (Triangle filter) before upload, and at most 6 uploads per frame (N = 6; leftovers wait for the next frame, which is requested immediately).
- Vsync: requested explicitly (`NativeOptions.glow_options.vsync = true`, also the default).
- Frame time: `ctx.input(|i| i.unstable_dt)` for the budget check; plus `ui()` CPU time (excludes GL paint and swap). The first 500 ms of each scroll phase is excluded. Worst-5 frames are logged with phase time, uploads, frees and ui() CPU time.
- "Exceeded interval" is `dt > interval`; "missed a refresh" is `dt > 1.5x interval` (a frame that cannot fit in one vsync slot plus jitter). Thresholds are for the monitor the window was actually on (144 Hz: 6.94 ms / 10.4 ms; 60 Hz: 16.7 ms / 25 ms), and for a fixed 60 Hz.
- The four result files were produced by the committed code minus a few idle-diagnostic `println!` lines added afterwards.

## Memory (all four runs)

| Run | WS first screen | Private first screen | WS after 3 000 | WS after 12 000 | Private after 12 000 | OS peak WS | Peak private |
| --- | --- | --- | --- | --- | --- | --- | --- |
| baseline-1 | 103.2 | 276.4 | 103.0 | 106.7 | 443.8 | 108.3 | ~445 |
| baseline-2 | 97.4 | 274.8 | 102.9 | 104.7 | 446.6 | 114.4 | ~447 |
| mitigated-1 | 99.9 | 259.5 | 99.4 | 108.5 | 388.6 | 109.5 | ~389 |
| mitigated-2 | 90.1 | 248.2 | 107.8 | 108.8 | 385.0 | 114.4 | ~386 |

MB. Working set stays 90 to 114 MB for both variants (idle WS under 150 MB passes). Private bytes are 250 to 275 MB right after the first screen and 385 to 447 MB after scrolling. The first-screen private bytes (about 250 to 275 MB before any texture churn) are unexplained (GL driver and runtime commit). The growth during scrolling is about 170 MB for baseline and about 130 MB for mitigated; that is consistent with texture memory (300 textures, 120 MB at 320x320 RGBA versus 48 MB at 200x200), but it was not isolated further. Mitigated lowers private bytes by about 60 MB.

## Idle

- Idle CPU: 0.016 s to 0.42 s over 10 s (0.03% to 2.3% of a core). Earlier runs measured 2 repaints and 0.00 to 0.02%.
- Idle repaints varied by run: 2 (earlier runs), 94, 124, 186, 347 in the four final runs. A diagnostic run (`ctx.repaint_causes()`) showed the cause: egui's floating scroll bar fade animation (`egui/src/containers/scroll_area.rs:1473`, `animate_bool_responsive`), no input events, no app-requested repaint. It animates while the pointer is over or near the window, so the idle result depends on where the mouse is. With the pointer away the idle cost is about 2 update calls and about 0% CPU. Any real build should disable the floating scroll bar fade (or accept a few frames per pointer move); this was not tested.

## Scroll results (4 runs)

| Run | Monitor / measured fps | Speed | dt p50 / p99 / max (ms) | dt > interval | missed a refresh (>1.5x) | > 60 Hz interval (16.7 ms) | uploads/s | cells loaded |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| baseline-1 | DISPLAY1 144 Hz / 144 (paced) | 3 000 | 6.94 / 8.36 / 17.18 | 4 552 | 4 | 1 | 155 | 100.0% |
| baseline-1 | same | 12 000 | 6.95 / 8.11 / 15.46 | 1 124 | 2 | 0 | 621 | 100.0% |
| baseline-2 | DISPLAY1 144 Hz / 276-281 (unpaced) | 3 000 | 3.55 / 7.10 / 11.78 | 269 | 2 | 0 | 155 | 100.0% |
| baseline-2 | same | 12 000 | 3.51 / 6.81 / 12.80 | 33 | 2 | 0 | 621 | 100.0% |
| mitigated-1 | DISPLAY2 60 Hz / 180-254 (unpaced) | 3 000 | 6.46 / 10.77 / 26.51 | 15 (60 Hz) | 1 (60 Hz) | 1 | 155 | 100.0% |
| mitigated-1 | same | 12 000 | 3.90 / 7.60 / 10.01 | 0 (60 Hz) | 0 (60 Hz) | 0 | 621 | 100.0% |
| mitigated-2 | DISPLAY1 144 Hz / 232-264 (unpaced) | 3 000 | 3.83 / 13.68 / 29.79 | 1 280 | 1 004 | 2 | 155 | 100.0% |
| mitigated-2 | same | 12 000 | 3.73 / 7.92 / 58.75 | 121 | 26 | 3 | 622 | 99.7% |

`ui()` CPU is 0.2 to 0.3 ms p50 and 0.4 to 0.5 ms p99 in every run (max 1.35 ms), i.e. the app's own frame work is about 4% of a 144 Hz budget. In the "dt > interval" column for paced runs the count is dominated by vsync jitter (p50 equals the interval), so "missed a refresh" is the meaningful column there.

Did any frame exceed the refresh interval? At 144 Hz: yes, in every run on a 144 Hz monitor (max dt 11.8 to 58.8 ms; 2 to 1 004 frames missed a whole refresh). At 60 Hz (16.7 ms): no in baseline-2 (max 12.8 ms) and in mitigated-1 at 12 000 px/s; yes, but rarely, in the rest (baseline-1 1 frame at 3 000 px/s, mitigated-1 15 frames at 3 000 px/s, mitigated-2 2 and 3 frames; max 17.2 to 58.8 ms).

## Pacing

Vsync is requested, but pacing is not reliable on this machine: measured fps was 144 in baseline-1, but 180 to 281 in the other runs, whether the window was on DISPLAY1 or DISPLAY2. glutin's swap interval is set (egui logs a failure through `log` only, and the probe has no logger), so the cause is the driver or DWM, not isolated. Consequences: dt in unpaced runs is raw frame-to-frame time, not a vsync slot, and "fits in a refresh" is then judged by dt <= interval, which is the stricter reading; in paced runs dt is quantised to the interval and a miss shows as dt >= 1.5x.

## Stall attribution

Worst frames are not caused by the app's own work. In baseline the 5 worst frames at 3 000 px/s mostly have 0 uploads and 0 frees (`t=44.04s dt=17.2 ms ui-cpu=0.31 ms uploads=0`), and ui() CPU never exceeds 1.4 ms. They cluster in time (mitigated-2: five of the 5 worst frames at 12 000 px/s lie within 12.47 to 12.88 s, up to 58.8 ms; mitigated-1 at 3 000 px/s around 8.9 to 11 s), which points at an external or driver stall (OS scheduling, DWM, other processes, GL driver), not at upload bursts. At 12 000 px/s, frames with 11 to 12 uploads (baseline) are slow (up to 15 ms) but not the worst. The mitigated variant (at most 6 uploads) has the same kind of stalls, so the upload cap does not remove them. Uploaded-per-second load: 155/s at 3 000 px/s and 621/s at 12 000 px/s (311/s with 6 columns in an earlier window layout); the loaders kept up (99.7 to 100% of visible cells showed a texture, so the 12 000 px/s result is a realistic worst case for the data path).

## Verdict (budget: idle working set < 150 MB; no frame misses the refresh interval at 144 Hz and at 60 Hz)

Idle memory: passes for both variants by working set (90 to 114 MB, peak 114 MB). Private bytes (250 to 275 MB at first screen, 385 to 447 MB after scrolling) would fail a 150 MB limit if the budget meant private/commit, and the first-screen part is unexplained; the mitigated variant cuts private bytes by about 60 MB but does not change that outcome. Idle CPU: 0% is achievable (2 repaints, about 0% CPU when the pointer is away) but not guaranteed as shipped: the floating scroll bar fade keeps egui repainting when the pointer is over the window (up to 2.3% of a core in these runs). Frame pacing: the "no frame misses the refresh" budget is not met at 144 Hz by either variant in this environment (every run had frames of 11.8 to 58.8 ms, and 2 to 1 004 frames per phase over 1.5x the interval); p99 passes in 1 of 4 baseline phases and in 0 of 4 mitigated phases at 6.94 ms (p99 6.81 to 13.68 ms), and at 60 Hz "no frame over 16.7 ms" holds in only 1 of 4 runs for both phases (baseline-2) plus mitigated-1 at 12 000 px/s (max dt elsewhere 17.2 to 58.8 ms), although p99 is under 16.7 ms in every phase. The mitigation (200 px downscale, 6 uploads per frame) gave no consistent frame-time improvement: its best phase is no better than baseline and its worst phase (mitigated-2) is the worst of all. The stalls have 0.2 to 0.5 ms of app CPU and often no upload in the frame, so they are not an egui-immediate-mode CPU cost. Whether they are driver, DWM or machine noise was not isolated; vsync pacing itself is inconsistent here. For the M0 gate: egui plus glow meets the working-set budget and has ample CPU headroom (ui() about 0.3 ms), but the strict "no missed frame at 144 Hz" budget is not demonstrated on this machine; before deciding, repeat on a quiet machine with a pacing check (for example a presentation-timing trace with PresentMon), and try wgpu or a different swap setup only if the stalls reproduce there. Adopt the 200 px downscale in the real build for the memory saving (about 60 MB private) regardless.
