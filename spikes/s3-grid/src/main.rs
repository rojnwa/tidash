//! S3 spike: egui virtualized grid, 10k lazily generated JPEGs, auto-scroll benchmark.
#![allow(unsafe_code)]
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

const N: usize = 10_000;
const CELL: f32 = 200.0;
const GAP: f32 = 8.0;
const LABEL: f32 = 20.0;
const LRU_CAP: usize = 300;
const SPEEDS: [f32; 2] = [3000.0, 12000.0];
const REFRESH_HZ: f32 = 144.0; // as reported by Windows (Win32_VideoController)

// ---- process stats (Windows) ----
fn mem() -> (usize, usize, usize) {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb);
        (c.WorkingSetSize, c.PrivateUsage, c.PeakWorkingSetSize)
    }
}
fn cpu_secs() -> f64 {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    unsafe {
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let f = |t: FILETIME| (((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64) as f64 / 1e7;
        f(k) + f(u)
    }
}
fn mb(b: usize) -> f64 {
    b as f64 / 1048576.0
}

// ---- image generation + decode ----
fn gen_jpeg(i: usize) -> Vec<u8> {
    let h = (i as u32).wrapping_mul(2654435761);
    let (r, g, b) = ((h >> 24) as u8, (h >> 16) as u8, (h >> 8) as u8);
    let mut px = vec![0u8; 320 * 320 * 3];
    for y in 0..320u32 {
        for x in 0..320u32 {
            let o = ((y * 320 + x) * 3) as usize;
            let s = ((x ^ y) & 63) as u8 * 2;
            px[o] = r.wrapping_add(s).wrapping_add((x / 2) as u8);
            px[o + 1] = g.wrapping_add(s).wrapping_add((y / 2) as u8);
            px[o + 2] = b.wrapping_add(s);
        }
    }
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
    image::ImageEncoder::write_image(enc, &px, 320, 320, image::ExtendedColorType::Rgb8).unwrap();
    out
}

#[derive(Default)]
struct Queue {
    wanted: Vec<usize>,
    inflight: HashSet<usize>,
}
type Shared = Arc<(Mutex<Queue>, Condvar)>;

fn spawn_workers(ctx: egui::Context, q: Shared, tx: mpsc::Sender<(usize, egui::ColorImage)>) {
    for _ in 0..4 {
        let (ctx, q, tx) = (ctx.clone(), q.clone(), tx.clone());
        std::thread::spawn(move || {
            loop {
                let i = {
                    let (m, cv) = &*q;
                    let mut g = m.lock().unwrap();
                    loop {
                        if let Some(i) = g.wanted.pop() {
                            g.inflight.insert(i);
                            break i;
                        }
                        g = cv.wait(g).unwrap();
                    }
                };
                // generating (encoding) the JPEG stands in for the network fetch; only decode is the real cost
                let bytes = gen_jpeg(i);
                let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
                    .unwrap()
                    .to_rgba8();
                let ci = egui::ColorImage::from_rgba_unmultiplied(
                    [img.width() as usize, img.height() as usize],
                    img.as_raw(),
                );
                if tx.send((i, ci)).is_ok() {
                    ctx.request_repaint();
                }
            }
        });
    }
}

// ---- app ----
#[derive(PartialEq, Clone, Copy, Debug)]
enum Phase {
    Load,
    Settle,
    Idle,
    Scroll(usize),
    Done,
}

struct Stats {
    dt: Vec<f32>,
    cpu: Vec<f32>,
}

struct App {
    q: Shared,
    rx: mpsc::Receiver<(usize, egui::ColorImage)>,
    tex: HashMap<usize, (egui::TextureHandle, u64)>,
    tick: u64,
    phase: Phase,
    t0: Instant,
    scroll_y: f32,
    max_y: f32,
    updates_in_idle: u32,
    idle_cpu0: f64,
    peak_ws: usize,
    peak_priv: usize,
    recent: Vec<f32>,
    stats: [Stats; 2],
    report: Vec<String>,
    last_cpu_frame: f32,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = mpsc::channel();
        let q: Shared = Arc::default();
        spawn_workers(cc.egui_ctx.clone(), q.clone(), tx);
        let s = || Stats { dt: vec![], cpu: vec![] };
        Self {
            q,
            rx,
            tex: HashMap::new(),
            tick: 0,
            phase: Phase::Load,
            t0: Instant::now(),
            scroll_y: 0.0,
            max_y: 1.0,
            updates_in_idle: 0,
            idle_cpu0: 0.0,
            peak_ws: 0,
            peak_priv: 0,
            recent: vec![],
            stats: [s(), s()],
            report: vec![],
            last_cpu_frame: 0.0,
        }
    }
    fn snap(&mut self, label: &str) {
        let (ws, pr, pk) = mem();
        self.report.push(format!(
            "MEM {label}: working_set={:.1} MB private={:.1} MB os_peak_ws={:.1} MB",
            mb(ws),
            mb(pr),
            mb(pk)
        ));
    }
}

fn pct(v: &[f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[((s.len() - 1) as f32 * p).round() as usize]
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let t_start = Instant::now();
        let dt = ctx.input(|i| i.unstable_dt);
        self.tick += 1;

        // drain finished decodes -> textures
        while let Ok((i, ci)) = self.rx.try_recv() {
            let t = ctx.load_texture(format!("t{i}"), ci, egui::TextureOptions::LINEAR);
            self.tex.insert(i, (t, self.tick));
            self.q.0.lock().unwrap().inflight.remove(&i);
        }

        let cols = (((ui.available_width() - GAP) / (CELL + GAP)).floor() as usize).max(1);
        let rows = N.div_ceil(cols);
        let row_h = CELL + LABEL + GAP;
        let mut visible = 0..0;

        let mut sa = egui::ScrollArea::vertical().auto_shrink(false);
        if matches!(self.phase, Phase::Scroll(_)) {
            sa = sa.vertical_scroll_offset(self.scroll_y);
        }
        let tick = self.tick;
        let tex = &mut self.tex;
        let out = sa.show_rows(ui, row_h, rows, |ui, range| {
            visible = range.clone();
            ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
            for r in range {
                ui.horizontal(|ui| {
                    for c in 0..cols {
                        let i = r * cols + c;
                        if i >= N {
                            break;
                        }
                        ui.vertical(|ui| {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(CELL, CELL), egui::Sense::hover());
                            if let Some((t, last)) = tex.get_mut(&i) {
                                *last = tick;
                                egui::Image::new(&*t).fit_to_exact_size(egui::vec2(CELL, CELL)).paint_at(ui, rect);
                            } else {
                                ui.painter().rect_filled(rect, 4.0, egui::Color32::from_gray(40));
                            }
                            ui.add_sized([CELL, LABEL - 4.0], egui::Label::new(format!("Album {i}")).truncate());
                        });
                    }
                });
            }
        });
        self.max_y = (out.content_size.y - out.inner_rect.height()).max(1.0);

        // request visible + 1 row ahead, not loaded / in flight; reversed so pop() serves top-left first
        let ahead = visible.start..(visible.end + 1).min(rows);
        {
            let (m, cv) = &*self.q;
            let mut g = m.lock().unwrap();
            let mut want: Vec<usize> = ahead
                .flat_map(|r| (0..cols).map(move |c| r * cols + c))
                .filter(|i| *i < N && !self.tex.contains_key(i) && !g.inflight.contains(i))
                .collect();
            want.reverse();
            let any = !want.is_empty();
            g.wanted = want;
            if any {
                cv.notify_all();
            }
        }
        while self.tex.len() > LRU_CAP {
            let k = *self.tex.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k).unwrap();
            self.tex.remove(&k);
        }
        let vis_loaded = (visible.start * cols..(visible.end * cols).min(N)).all(|i| self.tex.contains_key(&i));

        // phase machine
        let now = Instant::now();
        match self.phase {
            Phase::Load => {
                if vis_loaded && !visible.is_empty() {
                    self.phase = Phase::Settle;
                    self.t0 = now;
                    ctx.request_repaint_after(Duration::from_secs(2));
                }
            }
            Phase::Settle => {
                if now.duration_since(self.t0) >= Duration::from_secs(2) {
                    self.snap("first screen");
                    self.phase = Phase::Idle;
                    self.t0 = now;
                    self.updates_in_idle = 0;
                    self.idle_cpu0 = cpu_secs();
                    ctx.request_repaint_after(Duration::from_secs(10));
                }
            }
            Phase::Idle => {
                let el = now.duration_since(self.t0);
                if el >= Duration::from_secs(10) {
                    let cpu = cpu_secs() - self.idle_cpu0;
                    self.report.push(format!(
                        "IDLE 10s: repaints(update calls, excluding the wake-up frame)={} cpu_time={:.3}s ({:.2}% of one core)",
                        self.updates_in_idle,
                        cpu,
                        cpu / el.as_secs_f64() * 100.0
                    ));
                    self.phase = Phase::Scroll(0);
                    self.t0 = now;
                    self.scroll_y = 0.0;
                } else {
                    self.updates_in_idle += 1;
                }
            }
            Phase::Scroll(k) => {
                self.scroll_y = (now.duration_since(self.t0).as_secs_f32() * SPEEDS[k]).min(self.max_y);
                // skip the first 500 ms of each phase (warmup)
                if now.duration_since(self.t0).as_millis() > 500 {
                    self.stats[k].dt.push(dt);
                    self.stats[k].cpu.push(self.last_cpu_frame);
                    self.recent.push(dt);
                    if self.recent.len() > 600 {
                        self.recent.remove(0);
                    }
                }
                if self.scroll_y >= self.max_y {
                    self.snap(&format!("after scroll {} px/s", SPEEDS[k]));
                    if k + 1 < SPEEDS.len() {
                        self.phase = Phase::Scroll(k + 1);
                        self.t0 = now;
                        self.scroll_y = 0.0;
                    } else {
                        self.phase = Phase::Done;
                    }
                }
                ctx.request_repaint();
            }
            Phase::Done => {
                let (ws, pr, pk) = mem();
                println!("S3 RESULTS");
                println!("rows={rows} cols={cols} row_h={row_h} scrollable_h={:.0}", self.max_y);
                for l in &self.report {
                    println!("{l}");
                }
                println!(
                    "PEAK sampled per frame: working_set={:.1} MB private={:.1} MB; OS peak working set={:.1} MB (final ws={:.1}, priv={:.1})",
                    mb(self.peak_ws),
                    mb(self.peak_priv),
                    mb(pk),
                    mb(ws),
                    mb(pr)
                );
                let iv = 1.0 / REFRESH_HZ;
                for (k, s) in self.stats.iter().enumerate() {
                    let ms = |v: f32| v * 1000.0;
                    let over = |f: f32| s.dt.iter().filter(|d| **d > f).count();
                    println!(
                        "SCROLL {} px/s: frames={} dt p50={:.2} ms p99={:.2} ms max={:.2} ms | cpu-frame p50={:.2} ms p99={:.2} ms max={:.2} ms | dt>1.1x interval={} >1.5x={} >2x={} (interval {:.2} ms)",
                        SPEEDS[k],
                        s.dt.len(),
                        ms(pct(&s.dt, 0.5)),
                        ms(pct(&s.dt, 0.99)),
                        ms(pct(&s.dt, 1.0)),
                        ms(pct(&s.cpu, 0.5)),
                        ms(pct(&s.cpu, 0.99)),
                        ms(pct(&s.cpu, 1.0)),
                        over(1.1 * iv),
                        over(1.5 * iv),
                        over(2.0 * iv),
                        ms(iv)
                    );
                }
                std::process::exit(0);
            }
        }

        let (ws, pr, _) = mem();
        self.peak_ws = self.peak_ws.max(ws);
        self.peak_priv = self.peak_priv.max(pr);
        egui::Area::new("overlay".into()).fixed_pos(egui::pos2(10.0, 10.0)).show(&ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.label(format!(
                    "{:?} | dt p50 {:.2} p99 {:.2} ms | ws {:.0} MB priv {:.0} MB | tex {}",
                    self.phase,
                    pct(&self.recent, 0.5) * 1000.0,
                    pct(&self.recent, 0.99) * 1000.0,
                    mb(ws),
                    mb(pr),
                    self.tex.len()
                ));
            });
        });
        self.last_cpu_frame = t_start.elapsed().as_secs_f32();
    }
}

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native("s3-grid", opts, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
