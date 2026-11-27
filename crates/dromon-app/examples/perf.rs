//! Headless performance gates (plan §9, §5 M5) on the real app: its view, handlers and `on_key`
//! in Rune's headless `Runtime`, fed listings the way the service thread feeds them
//! (`ServiceEvent::ListingPage`, 512 entries a page, `service::conn::PAGE`).
//!
//! - `scroll`: a 100,000-entry folder, wheel-scrolled down and back up one wheel event per frame,
//!   at a brisk trackpad speed (`scroll`, 24 pt a frame) and a hard flick (`fling`, 280 pt a frame:
//!   ten new rows every frame). Per frame: the UI thread's CPU time, wall time and allocations.
//!   Then the frame after each of: ↓ and End (the selection moves), a click on a row, a letter
//!   typed into the ⌘F filter (or deleted); and the worst frame of the glide to the last row.
//! - `first-rows`: a 10,000-entry folder on screen as Loading (skeleton rows), then from the first
//!   page's `apply` to the end of the first frame that has its rows mounted; then the rest of the
//!   folder, a page a frame.
//! - `stream`: a 100,000-entry folder streaming in, one page applied and one frame drawn per
//!   page: the worst page and the total.
//! - `refresh`: a poll's `Refreshed` of the 100,000-entry folder with one file added (the
//!   listing diff, the sort, the rows), as one frame.
//! - `preview`: the quick preview (Space) of text files whose reads bring a full
//!   `TEXT_LIMIT` head (256,000 bytes of JSON- or log-like lines of varying length), fed as the
//!   service feeds it (`ServiceEvent::Preview`). The frame the text arrives in (its first
//!   layout), in a new window (`arrive-cold`: its first text in the mono face) and in a window
//!   whose panel already showed another text and was closed (`arrive-warm`); steady frames with the panel open and nothing changing, and with the pointer
//!   moving over the text; the text wheel-scrolled 24 pt and 280 pt a frame, down and back; and
//!   ↓ to the next row (the key's frame, the frame its 256 KB arrives in while the cursor may
//!   still move, `held`, and the frame it shows in once the cursor rested, `switch`), then ↑
//!   back (`back`: a text the panel kept, shown again in the key's frame) and ↓ again (`again`:
//!   the text just left). Last, a minified file: one 256 KB line, shown to its first 2,000
//!   characters (`one-line`).
//!
//! Headless: no window, no GPU. A frame here is Rune's view, commit, layout, text and paint
//! (instances built), not encoding, uploading or presenting them; those are what the 120 Hz
//! budget's remaining headroom is for.
//!
//! ```sh
//! cargo run --release -p dromon-app --example perf [--features perf] -- [--gate scroll,first-rows,stream,refresh,preview] [--samples 5]
//! ```
//!
//! With `--features perf` (Rune's `frame-profile`) the scroll frames are also split into Rune's
//! phases (view, commit, layout, text, paint). Prints `METRIC gate=.. metric=.. value=..` lines,
//! each the median over the samples (and the worst sample), for `scripts/perf.sh`.

use std::time::Instant;

use dromon_app::state::preview::{Phase, TEXT_LIMIT};
use dromon_app::state::{
    Action, App, Backend, Command, HostDirectory, Listing, PreviewBytes, PreviewEvent, PreviewId, ServiceEvent,
};
use dromon_core::{ConnStatus, Entry, EntryKind, HostId, HostSpec, RemotePath};
use rune::{Env, Key, KeyEvent, Mods, Runtime};

/// Counts allocations (and reallocations) of the whole process. The example's frames run on
/// one thread and nothing else runs meanwhile, so the difference across a frame is that
/// frame's. Rune's own counter (`procwatch::alloc::Counting`) isn't a dependency of this crate.
#[expect(
    unsafe_code,
    reason = "`GlobalAlloc` is an unsafe trait; this one only counts and hands every call to `System`"
)]
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::Relaxed;

    static COUNT: AtomicU64 = AtomicU64::new(0);

    pub struct Counting;

    // SAFETY: every call goes to `System` unchanged; the counter is a relaxed atomic add.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            COUNT.fetch_add(1, Relaxed);
            // SAFETY: the caller's contract for `alloc` is passed on as is.
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            COUNT.fetch_add(1, Relaxed);
            // SAFETY: as above.
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: as above.
            unsafe { System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            COUNT.fetch_add(1, Relaxed);
            // SAFETY: as above.
            unsafe { System.realloc(ptr, layout, new_size) }
        }
    }

    pub fn allocations() -> u64 {
        COUNT.load(Relaxed)
    }
}

#[global_allocator]
static ALLOC: counting::Counting = counting::Counting;

/// The app's window size (`lib.rs`), on a 2× display like the M-series Macs the gate is for.
const SIZE: (f32, f32) = (1080.0, 680.0);
const SCALE: f32 = 2.0;
/// The service's page size (`service::conn::PAGE`).
const PAGE: usize = 512;
const DT: f64 = 1.0 / 120.0;
const HOST: &str = "perf";
const FOLDER: &str = "/srv/big";

/// Connects at once and opens in [`FOLDER`]; listings come from the bench, page by page.
struct Bench;

impl Backend for Bench {
    fn send(&mut self, cmd: Command) -> Vec<ServiceEvent> {
        match cmd {
            Command::Connect { host } => vec![
                ServiceEvent::Status { host: host.clone(), status: ConnStatus::Ready },
                ServiceEvent::Opened { host, start: RemotePath::new(FOLDER) },
            ],
            _ => Vec::new(),
        }
    }
}

/// `n` entries in server order (unsorted): a folder in 97, files with mixed stems, numbers and
/// extensions (photos, logs, sources, dotfiles), sizes and dates.
fn entries(n: usize) -> Vec<Entry> {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let stems = ["IMG_", "DSC", "report-", "build.", "access.log.", "Screenshot 2026-11-", "data_", ".cache-"];
    let exts = ["jpg", "png", "txt", "gz", "rs", "json", "mp4", "pdf"];
    let mut out: Vec<Entry> = (0..n)
        .map(|i| {
            let r = next();
            let folder = i % 97 == 0;
            let stem = stems[(r % stems.len() as u64) as usize];
            let name = if folder {
                format!("{stem}dir-{i}")
            } else {
                format!("{stem}{i:06}.{}", exts[((r >> 8) % exts.len() as u64) as usize])
            };
            Entry {
                name,
                kind: if folder { EntryKind::Dir } else { EntryKind::File },
                size: if folder { 0 } else { (r >> 16) % 50_000_000 },
                mtime: Some(1_700_000_000 + ((r >> 24) % 60_000_000) as i64),
                mode: Some(if folder { 0o40755 } else { 0o100644 }),
                owner: None,
                group: None,
            }
        })
        .collect();
    // A server lists in its own order (hash order on ext4): shuffled, so sorting does real work.
    for i in (1..out.len()).rev() {
        out.swap(i, (next() % (i as u64 + 1)) as usize);
    }
    out
}

fn host() -> HostId {
    HostId(HOST.into())
}

fn folder() -> RemotePath {
    RemotePath::new(FOLDER)
}

/// The app connected to the bench host, its folder asked for and showing skeleton rows.
fn runtime() -> Runtime<App> {
    let mut rt = dromon_app::app().headless_runtime(SIZE, Env::new(true, SCALE));
    rt.update(|s, _| {
        *s = App::new(HostDirectory::default(), 1_790_000_000, Box::new(Bench));
        s.apply(ServiceEvent::Hosts(vec![HostSpec::from_config_alias(HOST)]));
        s.act(Action::SelectHost(host()));
    });
    // The skeleton rows shimmer until the listing comes: a few frames of it, not a settle.
    for _ in 0..12 {
        rt.frame(DT);
    }
    assert!(rt.state.conn().is_some_and(|c| c.listing == Listing::Loading));
    rt
}

#[track_caller]
fn settle(rt: &mut Runtime<App>) {
    for _ in 0..3000 {
        if !rt.frame(DT) {
            return;
        }
    }
    eprintln!("perf: still animating after 3000 frames (line {})", std::panic::Location::caller().line());
}

fn apply(rt: &mut Runtime<App>, ev: ServiceEvent) {
    rt.update(move |s, _| s.apply(ev));
}

fn page(entries: Vec<Entry>, first: bool, done: bool) -> ServiceEvent {
    ServiceEvent::ListingPage { host: host(), path: folder(), entries, first, done }
}

/// Streams `all` in pages, one frame each; returns each page's (apply + frame, apply) in ms.
fn stream(rt: &mut Runtime<App>, all: &[Entry]) -> Vec<(f64, f64)> {
    let pages: Vec<Vec<Entry>> = all.chunks(PAGE).map(<[Entry]>::to_vec).collect();
    let last = pages.len() - 1;
    let mut took = Vec::with_capacity(pages.len());
    for (i, p) in pages.into_iter().enumerate() {
        let t = Instant::now();
        apply(rt, page(p, i == 0, i == last));
        let applied = ms(t);
        rt.frame(DT);
        took.push((ms(t), applied));
    }
    took
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn cpu_ms() -> f64 {
    rune::thread_cpu_ns().unwrap_or(0.0) / 1e6
}

/// A row's element key in the list (`views::ROWS`).
fn row_key(name: &str) -> String {
    format!("rows/{name}")
}

fn row_name(rt: &Runtime<App>, i: usize) -> String {
    rt.state.conn().map(|c| c.row(i).name.clone()).expect("rows")
}

/// One frame's cost.
#[derive(Clone, Copy, Default)]
struct Cost {
    cpu: f64,
    wall: f64,
    allocs: f64,
    /// CPU ms of the input itself (handlers that run as it's delivered), before the frame.
    input: f64,
    /// Rune's phases, CPU ms: view, commit, layout, text, paint (with `--features perf`).
    phases: [f64; 5],
    /// The frame asked for another (something still moves).
    more: bool,
}

/// Runs `input`, then one frame, and what the two cost.
fn measure(rt: &mut Runtime<App>, input: impl FnOnce(&mut Runtime<App>)) -> Cost {
    #[cfg(feature = "perf")]
    let _ = rune::take_frame_phases();
    let (a0, c0, t0) = (counting::allocations(), cpu_ms(), Instant::now());
    input(rt);
    let handled = cpu_ms() - c0;
    let more = rt.frame(DT);
    #[allow(unused_mut, reason = "only `--features perf` writes the frame phases into it")]
    let mut cost = Cost {
        cpu: cpu_ms() - c0,
        wall: ms(t0),
        allocs: (counting::allocations() - a0) as f64,
        input: handled,
        phases: [0.0; 5],
        more,
    };
    #[cfg(feature = "perf")]
    {
        let p = rune::take_frame_phases();
        // The CPU columns follow the wall ones; commit and layout include their nested phases.
        for (i, v) in cost.phases.iter_mut().enumerate() {
            *v = p[rune::PHASES + i];
        }
    }
    cost
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// `frames` frames scrolling down `dy` points a frame, then as many back up.
fn scroll_once(rt: &mut Runtime<App>, dy: f32, frames: usize) -> Vec<Cost> {
    let (x, y) = rt.find("list").expect("the list");
    let top = row_name(rt, 0);
    let mut costs = Vec::with_capacity(2 * frames);
    for i in 0..2 * frames {
        let d = if i < frames { -dy } else { dy };
        costs.push(measure(rt, |rt| rt.wheel(x, y, d)));
        if i + 1 == frames {
            assert!(rt.find(&row_key(&top)).is_none(), "the list scrolled away from its first row");
        }
    }
    settle(rt);
    costs
}

struct Metrics(Vec<(String, String, f64)>);

impl Metrics {
    fn add(&mut self, gate: &str, metric: &str, value: f64) {
        self.0.push((gate.to_owned(), metric.to_owned(), value));
    }

    fn costs(&mut self, gate: &str, costs: &[Cost]) {
        let mut cpu: Vec<f64> = costs.iter().map(|c| c.cpu).collect();
        let mut wall: Vec<f64> = costs.iter().map(|c| c.wall).collect();
        let allocs = costs.iter().map(|c| c.allocs).sum::<f64>() / costs.len() as f64;
        self.add(gate, "cpu_p50_ms", percentile(&mut cpu, 0.5));
        self.add(gate, "cpu_p99_ms", percentile(&mut cpu, 0.99));
        self.add(gate, "cpu_max_ms", percentile(&mut cpu, 1.0));
        self.add(gate, "wall_p99_ms", percentile(&mut wall, 0.99));
        self.add(gate, "allocs_per_frame", allocs);
        self.phases(gate, costs);
    }

    /// Means of Rune's phases over `costs` (with `--features perf`).
    fn phases(&mut self, gate: &str, costs: &[Cost]) {
        if cfg!(feature = "perf") {
            for (i, name) in ["view", "commit", "layout", "text", "paint"].iter().enumerate() {
                let mean = costs.iter().map(|c| c.phases[i]).sum::<f64>() / costs.len() as f64;
                self.add(gate, &format!("{name}_cpu_mean_ms"), mean);
            }
        }
    }

    /// One input's frame, a few times over: medians.
    fn events(&mut self, gate: &str, costs: &[Cost]) {
        self.add(gate, "cpu_ms", percentile(&mut costs.iter().map(|c| c.cpu).collect::<Vec<_>>(), 0.5));
        self.add(gate, "input_cpu_ms", percentile(&mut costs.iter().map(|c| c.input).collect::<Vec<_>>(), 0.5));
        self.add(gate, "allocs", percentile(&mut costs.iter().map(|c| c.allocs).collect::<Vec<_>>(), 0.5));
        self.phases(gate, costs);
    }
}

fn scroll_gates(m: &mut Metrics) {
    let mut rt = runtime();
    stream(&mut rt, &entries(100_000));
    settle(&mut rt);
    assert_eq!(rt.state.conn().map(|c| c.row_count()), Some(100_000));
    // One warm-up pass (fonts, glyph caches), then the measured ones.
    scroll_once(&mut rt, 24.0, 60);
    m.costs("scroll", &scroll_once(&mut rt, 24.0, 120));
    m.costs("fling", &scroll_once(&mut rt, 280.0, 120));

    // Keys and clicks, from the top of a fresh list (the wheel's momentum leaves it elsewhere).
    let mut rt = runtime();
    stream(&mut rt, &entries(100_000));
    settle(&mut rt);
    let n = 100_000;
    let (mut down, mut end, mut glide, mut click) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..5 {
        // An arrow key and End: the selection moves (the view runs again) and asks for a reveal.
        down.push(measure(&mut rt, |rt| rt.key(KeyEvent { key: Key::Down, mods: Mods::default() })));
        settle(&mut rt);
        end.push(measure(&mut rt, |rt| rt.key(KeyEvent { key: Key::End, mods: Mods::default() })));
        settle(&mut rt);
        // The jump to the last row itself, every frame of it, as a path reveal (`rows/<name>`).
        // (The app's own reveal, `scroll_into_view_in(ROWS, name)`, doesn't scroll the list
        // headless: see the report.)
        let last = row_name(&rt, n - 1);
        let key = row_key(&last);
        let mut frames = vec![measure(&mut rt, |rt| rt.update(move |_, tx| tx.scroll_into_view(key)))];
        while frames.last().is_some_and(|c| c.more) && frames.len() < 600 {
            frames.push(measure(&mut rt, |_| {}));
        }
        glide.push(frames.iter().map(|c| c.cpu).fold(0.0, f64::max));
        assert!(rt.find(&row_key(&last)).is_some(), "the jump shows the last row");
        let (x, y) = rt.find(&row_key(&row_name(&rt, n - 3))).expect("a row near the end");
        click.push(measure(&mut rt, |rt| {
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        }));
        settle(&mut rt);
        let first = row_key(&row_name(&rt, 0));
        rt.update(move |_, tx| tx.scroll_into_view(first));
        rt.key(KeyEvent { key: Key::Home, mods: Mods::default() });
        settle(&mut rt);
    }
    // ⌘F (Ctrl+F) and a letter typed into the filter: 100,000 names matched, the rows rebuilt.
    let mac = cfg!(target_os = "macos");
    rt.key(KeyEvent { key: Key::Command('f'), mods: Mods { cmd: true, logo: mac, ctrl: !mac, ..Mods::default() } });
    settle(&mut rt);
    assert!(rt.find("tb-filter-input").is_some(), "the filter field is open");
    let mut typed = Vec::new();
    for letter in ["g", "z", "x"] {
        typed.push(measure(&mut rt, |rt| rt.key(KeyEvent { key: Key::Text(letter.into()), mods: Mods::default() })));
        settle(&mut rt);
        typed.push(measure(&mut rt, |rt| rt.key(KeyEvent { key: Key::Backspace, mods: Mods::default() })));
        settle(&mut rt);
    }
    assert_eq!(rt.state.conn().map(|c| c.row_count()), Some(n));
    m.events("filter-type", &typed);
    m.events("key-down", &down);
    m.events("key-end", &end);
    m.add("jump-end", "cpu_max_ms", percentile(&mut glide, 0.5));
    m.events("click", &click);
}

fn first_rows_gate(m: &mut Metrics) {
    let all = entries(10_000);
    let mut rt = runtime();
    let (c0, t0) = (cpu_ms(), Instant::now());
    apply(&mut rt, page(all[..PAGE].to_vec(), true, false));
    rt.frame(DT);
    let (wall, cpu) = (ms(t0), cpu_ms() - c0);
    assert!(rt.find(&row_key(&row_name(&rt, 0))).is_some(), "the first page's rows are on screen");
    m.add("first-rows-10k", "wall_ms", wall);
    m.add("first-rows-10k", "cpu_ms", cpu);
    // The rest of it, a page a frame.
    let t = Instant::now();
    let rest: Vec<Vec<Entry>> = all[PAGE..].chunks(PAGE).map(<[Entry]>::to_vec).collect();
    let last = rest.len() - 1;
    let mut worst = wall;
    for (i, p) in rest.into_iter().enumerate() {
        let tp = Instant::now();
        apply(&mut rt, page(p, false, i == last));
        rt.frame(DT);
        worst = worst.max(ms(tp));
    }
    assert_eq!(rt.state.conn().map(|c| c.row_count()), Some(10_000));
    m.add("stream-10k", "total_ms", ms(t) + wall);
    m.add("stream-10k", "worst_page_ms", worst);
}

fn stream_gate(m: &mut Metrics) {
    let mut rt = runtime();
    let all = entries(100_000);
    let t = Instant::now();
    let pages = stream(&mut rt, &all);
    let (mut frames, mut applies): (Vec<f64>, Vec<f64>) = pages.into_iter().unzip();
    m.add("stream-100k", "total_ms", ms(t));
    m.add("stream-100k", "worst_page_ms", percentile(&mut frames, 1.0));
    m.add("stream-100k", "p50_page_ms", percentile(&mut frames, 0.5));
    m.add("stream-100k", "worst_apply_ms", percentile(&mut applies, 1.0));
}

fn refresh_gate(m: &mut Metrics) {
    let mut rt = runtime();
    let mut all = entries(100_000);
    stream(&mut rt, &all);
    settle(&mut rt);
    all.push(Entry {
        name: "new-upload.bin".into(),
        kind: EntryKind::File,
        size: 1,
        mtime: Some(1_790_000_000),
        mode: None,
        owner: None,
        group: None,
    });
    // Its parts on their own: the listing diff and the sort.
    let t = Instant::now();
    let changes = dromon_sftp::diff(rt.state.conn().map_or(&[], |c| c.entries()), &all);
    m.add("refresh-100k", "diff_ms", ms(t));
    assert_eq!(changes.inserted, ["new-upload.bin"]);
    let mut sorted = all.clone();
    let t = Instant::now();
    dromon_core::sort_entries(&mut sorted, dromon_core::SortKey::Name, dromon_core::SortDir::Ascending);
    m.add("refresh-100k", "sort_ms", ms(t));
    let cost = measure(&mut rt, |rt| apply(rt, ServiceEvent::Refreshed { host: host(), path: folder(), entries: all }));
    assert!(rt.state.conn().is_some_and(|c| c.find("new-upload.bin").is_some()));
    m.events("refresh-100k", &[cost]);
}

/// The preview gate's folder, in name order: text files of [`TEXT_TOTAL`] bytes, each read for
/// its first [`TEXT_LIMIT`] (`.json` JSON-like, `.txt` log-like; the last, minified, one line).
const PREVIEWED: [&str; 9] = [
    "a-data.json",
    "b-events.txt",
    "c-data.json",
    "d-events.txt",
    "e-data.json",
    "f-events.txt",
    "g-data.json",
    "h-events.txt",
    "i-bundle.min.js",
];
const TEXT_TOTAL: u64 = 900_000;
/// The text's rows (`views::preview::LINES`).
const LINES: &str = "preview-lines";

/// `n` bytes of text in lines of varying length (about 20 to 200 characters): JSON-like
/// members when `json`, else log lines; different for each `seed`.
fn text(json: bool, seed: u64, n: usize) -> Vec<u8> {
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed.wrapping_mul(0xA24B_AED4_963E_E407);
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let words = ["alpha", "build", "cache", "deploy", "event", "fetch", "graph", "index", "queue", "worker"];
    let mut out = String::with_capacity(n + 256);
    let mut i = 0u64;
    while out.len() < n {
        let r = next();
        let w = |k: u64| words[((r >> k) % words.len() as u64) as usize];
        let many: Vec<&str> = (0..(r % 14)).map(|k| w(8 + k * 3)).collect();
        let line = if json {
            let indent = 2 * (1 + (r >> 4) % 3) as usize;
            format!(
                "{:indent$}\"{}-{i:05}\": {{ \"value\": {}, \"tags\": [\"{}\"] }},",
                "",
                w(20),
                (r >> 24) % 100_000,
                many.join("\", \""),
            )
        } else {
            let t = i * 7 % 86_400;
            let level = ["INFO", "INFO", "WARN", "INFO", "ERROR", "DEBUG"][(r % 6) as usize];
            let at = format!("2026-11-25T{:02}:{:02}:{:02}Z", t / 3600, t / 60 % 60, t % 60);
            format!("{at} {level:<5} {} {}ms {}", w(20), (r >> 32) % 900, many.join(" "))
        };
        out.push_str(&line);
        out.push('\n');
        i += 1;
    }
    out.truncate(n);
    out.into_bytes()
}

/// The read the panel waits for.
fn waiting(rt: &Runtime<App>) -> PreviewId {
    match &rt.state.ui.preview.phase {
        Phase::Loading { id, .. } => *id,
        other => panic!("the panel waits for no read: {other:?}"),
    }
}

/// The read the panel waits for answers with the first 256 KB of `PREVIEWED[row]`, and the
/// frame after. (A text the cursor moved to waits for it to rest: [`shows`].)
fn arrive(rt: &mut Runtime<App>, row: usize) -> Cost {
    let id = waiting(rt);
    let name = PREVIEWED[row];
    let mut bytes = text(name.ends_with(".json"), row as u64, TEXT_LIMIT as usize);
    if name.ends_with(".min.js") {
        // Minified: all of it on one line.
        bytes.iter_mut().filter(|b| **b == b'\n').for_each(|b| *b = b' ');
    }
    let ev = PreviewEvent::Done { id, host: host(), bytes: PreviewBytes(bytes.into()), total: TEXT_TOTAL };
    measure(rt, move |rt| apply(rt, ServiceEvent::Preview(ev)))
}

/// Frames until the panel's text shows (the cursor rested); the frame it shows in.
fn shows(rt: &mut Runtime<App>) -> Cost {
    for _ in 0..240 {
        let cost = measure(rt, |_| {});
        if rt.find(LINES).is_some() {
            return cost;
        }
    }
    panic!("the text never showed");
}

fn key(rt: &mut Runtime<App>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
}

/// The top of the panel's text (its rows, laid out or not), in the window.
fn text_top(rt: &Runtime<App>) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(LINES)).map(|e| e.rect[1]).expect("text")
}

/// `frames` frames wheel-scrolling the panel's text down `dy` points a frame, then back up.
fn scroll_text(rt: &mut Runtime<App>, dy: f32, frames: usize) -> Vec<Cost> {
    let (x, y) = rt.find("preview-body").expect("the panel's body");
    let top = text_top(rt);
    let mut costs = Vec::with_capacity(2 * frames);
    for i in 0..2 * frames {
        let d = if i < frames { -dy } else { dy };
        costs.push(measure(rt, |rt| rt.wheel(x, y, d)));
        if i + 1 == frames {
            assert!(text_top(rt) < top - dy, "the text scrolled");
        }
    }
    settle(rt);
    costs
}

/// A new window on the preview gate's folder ([`PREVIEWED`]).
fn previewing() -> Runtime<App> {
    let mut rt = runtime();
    let listed = PREVIEWED.iter().map(|name| Entry {
        name: (*name).into(),
        kind: EntryKind::File,
        size: TEXT_TOTAL,
        mtime: Some(1_789_990_000),
        mode: Some(0o100644),
        owner: None,
        group: None,
    });
    apply(&mut rt, ServiceEvent::Listing { host: host(), path: folder(), entries: listed.collect() });
    settle(&mut rt);
    rt
}

/// Space on `PREVIEWED[row]`: the panel springs in, its read under way, and its spinner turns
/// until the text comes: half a second of it, not a settle.
fn open(rt: &mut Runtime<App>, row: usize) {
    rt.update(move |s, _| drop(s.act(Action::Click { name: PREVIEWED[row].into(), extend: false, toggle: false })));
    key(rt, Key::Text(" ".into()));
    for _ in 0..60 {
        rt.frame(DT);
    }
}

/// The text arriving after Space in a window whose panel already showed another text (and
/// was closed): what a preview costs once the window has drawn the mono face.
fn warm_arrive() -> Cost {
    let mut rt = previewing();
    open(&mut rt, 0);
    arrive(&mut rt, 0);
    settle(&mut rt);
    key(&mut rt, Key::Escape);
    settle(&mut rt);
    assert!(matches!(rt.state.ui.preview.phase, Phase::Closed), "Escape closed the panel");
    // Another file of the same kind (JSON-like), not read before.
    open(&mut rt, 2);
    let cost = arrive(&mut rt, 2);
    assert!(rt.find(LINES).is_some(), "Space's text shows as it comes");
    cost
}

fn preview_gate(m: &mut Metrics) {
    // The window's first preview: its first text in the mono face.
    let mut rt = previewing();
    open(&mut rt, 0);
    let cold = arrive(&mut rt, 0);
    assert!(rt.find(LINES).is_some(), "Space's text shows as it comes");
    settle(&mut rt);
    // Open, nothing changing; then the pointer moving over the text.
    let idle: Vec<Cost> = (0..120).map(|_| measure(&mut rt, |_| {})).collect();
    let (x, y) = rt.find("preview-body").expect("the panel's body");
    let hover: Vec<Cost> = (0..120).map(|i| measure(&mut rt, |rt| rt.pointer_move(x, y + (i % 40) as f32))).collect();
    settle(&mut rt);
    // One warm-up pass, then the measured ones.
    scroll_text(&mut rt, 24.0, 60);
    let scroll = scroll_text(&mut rt, 24.0, 120);
    let fling = scroll_text(&mut rt, 280.0, 120);
    // ↓ to a row not read yet: the key's frame, the frame its text arrives in (held: the cursor
    // may move on), the frame it shows in once the cursor rested. Then ↑ to the row before (its
    // text kept by the panel, shown at once) and ↓ again (the text just left).
    let (mut down, mut held, mut switch) = (Vec::new(), Vec::new(), Vec::new());
    let (mut back, mut again) = (Vec::new(), Vec::new());
    let minified = PREVIEWED.len() - 1;
    for row in 1..minified {
        down.push(measure(&mut rt, |rt| key(rt, Key::Down)));
        held.push(arrive(&mut rt, row));
        assert!(rt.find(LINES).is_none(), "a text the cursor moved to waits for it to rest");
        switch.push(shows(&mut rt));
        settle(&mut rt);
        for (k, into) in [(Key::Up, &mut back), (Key::Down, &mut again)] {
            into.push(measure(&mut rt, |rt| key(rt, k)));
            assert!(rt.find(LINES).is_some(), "a kept text shows at once");
            settle(&mut rt);
        }
    }
    // A minified file: one 256 KB line, shown to its first 2,000 characters.
    key(&mut rt, Key::Down);
    rt.frame(DT);
    arrive(&mut rt, minified);
    let one_line = shows(&mut rt);
    settle(&mut rt);
    let warm = warm_arrive();
    m.events("preview-arrive-cold", &[cold]);
    m.events("preview-arrive-warm", &[warm]);
    m.costs("preview-idle", &idle);
    m.costs("preview-hover", &hover);
    m.costs("preview-scroll", &scroll);
    m.costs("preview-fling", &fling);
    m.events("preview-down", &down);
    m.events("preview-held", &held);
    m.events("preview-switch", &switch);
    m.events("preview-back", &back);
    m.events("preview-again", &again);
    m.events("preview-one-line", &[one_line]);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let samples: usize = value("--samples").map_or(5, |s| s.parse().expect("--samples N"));
    let gates = value("--gate").unwrap_or_else(|| "scroll,first-rows,stream,refresh".into());
    if cfg!(debug_assertions) {
        eprintln!("perf: debug build; time a --release build");
    }
    let mut runs: Vec<Metrics> = Vec::new();
    for _ in 0..samples.max(1) {
        let mut m = Metrics(Vec::new());
        for gate in gates.split(',') {
            match gate {
                "scroll" => scroll_gates(&mut m),
                "first-rows" => first_rows_gate(&mut m),
                "stream" => stream_gate(&mut m),
                "refresh" => refresh_gate(&mut m),
                "preview" => preview_gate(&mut m),
                other => panic!("unknown gate {other:?} (scroll, first-rows, stream, refresh, preview)"),
            }
        }
        runs.push(m);
    }
    for (i, (gate, metric, _)) in runs[0].0.iter().enumerate() {
        let mut v: Vec<f64> = runs.iter().map(|r| r.0[i].2).collect();
        let median = percentile(&mut v, 0.5);
        let worst = percentile(&mut v, 1.0);
        println!("METRIC gate={gate} metric={metric} value={median:.3} worst={worst:.3} n={}", v.len());
    }
}
