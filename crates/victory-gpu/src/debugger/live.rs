//! The live view: `live.html` in the debugger's folder, written anew every
//! [`EVERY`] while the title runs. Open it in a browser; it reloads itself.
//!
//! A recording (F9, F11) is one frame in full and takes half a minute to
//! write. This is the run as it goes, from counters kept all the time:
//!
//! - **what the frame's time goes to**: the overlay's lines (frame rate,
//!   preparing, the card, copies, textures, drops), as the frame line has
//!   them;
//! - **the calls by pipeline**: for every pipeline the title drew or
//!   dispatched with since the last write, the calls a frame (a draw or a
//!   dispatch each), the time the recording threads spent binding it (its
//!   tables, views and state: most of a call's cost on the CPU), and in a
//!   timing run (`VICTORY_GPU_TIMING`) its commands' time on the card;
//!   listed by cost, heaviest first, and by calls.
//!
//! The counters cost a lock and a map lookup a call (some 5,000 a frame),
//! and only with the debugger on.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

/// How often the page is written.
const EVERY: Duration = Duration::from_secs(2);
/// Pipelines listed in each table.
const ROWS: usize = 40;

#[derive(Default, Clone, Copy)]
struct Row {
    compute: bool,
    calls: u64,
    cpu_ns: u64,
    /// Commands timed on the card and their microseconds (timing runs).
    gpu: (u64, u64),
    /// The host root signature and pipeline state bound (their addresses).
    objects: (usize, usize),
}

static BY_PIPELINE: Mutex<Option<HashMap<String, Row>>> = Mutex::new(None);
/// The overlay's text of the last frame line.
static STATUS: Mutex<String> = Mutex::new(String::new());
/// Frames presented since the page was last written, and pages written.
static FRAMES: AtomicU64 = AtomicU64::new(0);
static PAGES: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<Option<Instant>> = Mutex::new(None);

fn with_row(name: &str, f: impl FnOnce(&mut Row)) {
    let mut by = BY_PIPELINE.lock().unwrap_or_else(|e| e.into_inner());
    let by = by.get_or_insert_with(HashMap::new);
    match by.get_mut(name) {
        Some(row) => f(row),
        None => f(by.entry(name.to_owned()).or_default()),
    }
}

/// A draw's or dispatch's binding of a pipeline, timed while it lives.
pub(crate) struct Bind<'a> {
    name: &'a str,
    compute: bool,
    objects: (usize, usize),
    started: Instant,
}

/// Before a pipeline is bound for a draw or dispatch (`State::bind`).
pub(crate) fn bind(name: &str, compute: bool, root: usize, pso: usize) -> Option<Bind<'_>> {
    super::on().then(|| Bind { name, compute, objects: (root, pso), started: Instant::now() })
}

impl Drop for Bind<'_> {
    fn drop(&mut self) {
        let ns = self.started.elapsed().as_nanos() as u64;
        let (compute, objects) = (self.compute, self.objects);
        with_row(self.name, |row| {
            row.objects = objects;
            row.compute = compute;
            row.calls += 1;
            row.cpu_ns += ns;
        });
    }
}

/// A command of `pipeline` (the listing's key: its first word, then the
/// pipeline in brackets) took `us` on the card (timing runs).
pub(crate) fn card(key: &str, us: u64) {
    if !super::on() {
        return;
    }
    // `dispatch [compute 0123...]` → `[compute 0123...]`, the name binds use.
    let Some(at) = key.find('[') else { return };
    with_row(&key[at..], |row| {
        row.gpu.0 += 1;
        row.gpu.1 += us;
    });
}

/// The overlay's text, with the frame line.
pub(crate) fn status(text: &str) {
    text.clone_into(&mut STATUS.lock().unwrap_or_else(|e| e.into_inner()));
}

/// At every present: the page, if it is due.
pub(crate) fn tick() {
    let frames = FRAMES.fetch_add(1, Relaxed) + 1;
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let since = *last.get_or_insert_with(Instant::now);
    let elapsed = since.elapsed();
    if elapsed < EVERY {
        return;
    }
    *last = Some(Instant::now());
    drop(last);
    FRAMES.store(0, Relaxed);
    let rows = BY_PIPELINE.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    let status = STATUS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let rows: Vec<(String, Row)> = rows.into_iter().collect();
    let page = page(&status, rows, frames, elapsed, PAGES.fetch_add(1, Relaxed) + 1);
    let Some(dir) = super::dir() else { return };
    // Whole or not at all for a browser that reloads meanwhile: written
    // beside and moved over; written in place where the move is refused.
    let (to, beside) = (dir.join("live.html"), dir.join("live.html.new"));
    if std::fs::write(&beside, &page).is_err() || std::fs::rename(&beside, &to).is_err() {
        let _ = std::fs::write(&to, &page);
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The page: the status lines, the totals, and the pipelines by cost and
/// by calls, all per frame over the `frames` presented in `elapsed`.
fn page(
    status: &str,
    mut rows: Vec<(String, Row)>,
    frames: u64,
    elapsed: Duration,
    number: u64,
) -> String {
    let per = frames.max(1) as f64;
    let fps = frames as f64 / elapsed.as_secs_f64().max(1e-3);
    let total = |f: &dyn Fn(&Row) -> u64, compute: Option<bool>| -> f64 {
        rows.iter().filter(|r| compute.is_none_or(|c| r.1.compute == c)).map(|r| f(&r.1)).sum::<u64>() as f64
    };
    let (draws, dispatches) = (total(&|r| r.calls, Some(false)) / per, total(&|r| r.calls, Some(true)) / per);
    let cpu_ms = total(&|r| r.cpu_ns, None) / 1e6 / per;
    let card_ms = total(&|r| r.gpu.1, None) / 1e3 / per;
    let timed = card_ms > 0.0;
    let pipelines = rows.len();
    let mut out = String::with_capacity(32 << 10);
    out.push_str(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"refresh\" content=\"2\">\
         <title>VIctory live</title><style>\
         body{background:#14161a;color:#d7dae0;font:13px/1.45 Consolas,monospace;margin:14px}\
         h1{font-size:15px;margin:0 0 6px}h2{font-size:13px;margin:14px 0 4px;color:#9fd0ff}\
         pre{white-space:pre-wrap;background:#1c1f25;padding:8px;margin:0;border-radius:4px}\
         table{border-collapse:collapse}td,th{padding:1px 10px 1px 0;text-align:right}\
         td:first-child,th:first-child{text-align:left}th{color:#8b93a1;font-weight:normal;border-bottom:1px solid #333}\
         .bar{display:inline-block;height:9px;background:#4c8dd6;vertical-align:middle}.c{color:#e0b060}.g{color:#7fd08a}.bad{color:#ff6b6b;font-weight:bold}\
         .cols{display:flex;gap:28px;flex-wrap:wrap}.note{color:#8b93a1}</style></head><body>",
    );
    let _ = write!(
        out,
        "<h1>VIctory live <span class=\"note\">page {number}, the last {:.1} s: {frames} frames, {fps:.1} fps</span></h1>",
        elapsed.as_secs_f64()
    );
    let _ = write!(
        out,
        "<pre>{}</pre><h2>Calls a frame</h2><pre>{draws:.0} draws with {} graphics pipelines, {dispatches:.0} dispatches with {} compute pipelines \
         ({pipelines} pipelines in all); binding them took the recording threads {cpu_ms:.1} ms a frame{}</pre>",
        if status.is_empty() { "(the frame line has not come yet)".to_owned() } else { escape(status) },
        rows.iter().filter(|r| !r.1.compute).count(),
        rows.iter().filter(|r| r.1.compute).count(),
        if timed {
            format!("; their commands took the card {card_ms:.1} ms a frame (a timing run: lists run one at a time)")
        } else {
            " (the card's time by pipeline needs a timing run: VICTORY_GPU_TIMING=1 with dred)".to_owned()
        }
    );
    let table = |out: &mut String, title: &str, rows: &[(String, Row)], widest: f64, bar: &dyn Fn(&Row) -> f64| {
        let _ = write!(
            out,
            "<div><h2>{title}</h2><table><tr><th>pipeline</th><th>calls</th><th>CPU ms</th><th>us each</th>{}<th></th></tr>",
            if timed { "<th>card ms</th>" } else { "" }
        );
        for (name, r) in rows.iter().take(ROWS) {
            let calls = r.calls as f64 / per;
            let cpu = r.cpu_ns as f64 / 1e6 / per;
            let each = r.cpu_ns as f64 / 1e3 / r.calls.max(1) as f64;
            let _ = write!(
                out,
                "<tr><td class=\"{}\">{}</td><td>{calls:.1}</td><td>{cpu:.2}</td><td>{each:.1}</td>",
                if r.compute { "c" } else { "g" },
                escape(name)
            );
            if timed {
                let _ = write!(out, "<td>{:.2}</td>", r.gpu.1 as f64 / 1e3 / per);
            }
            let _ = write!(out, "<td><span class=\"bar\" style=\"width:{:.0}px\"></span></td></tr>", 120.0 * bar(r) / widest.max(1e-9));
        }
        out.push_str("</table></div>");
    };
    out.push_str("<div class=\"cols\">");
    if timed {
        rows.sort_by(|a, b| b.1.gpu.1.cmp(&a.1.gpu.1));
        let widest = rows.first().map_or(0.0, |r| r.1.gpu.1 as f64);
        table(&mut out, "Heaviest on the card", &rows, widest, &|r| r.gpu.1 as f64);
    }
    rows.sort_by(|a, b| b.1.cpu_ns.cmp(&a.1.cpu_ns));
    let widest = rows.first().map_or(0.0, |r| r.1.cpu_ns as f64);
    table(&mut out, "Heaviest on the CPU (binding)", &rows, widest, &|r| r.cpu_ns as f64);
    rows.sort_by(|a, b| b.1.calls.cmp(&a.1.calls));
    let widest = rows.first().map_or(0.0, |r| r.1.calls as f64);
    table(&mut out, "Most calls", &rows, widest, &|r| r.calls as f64);
    out.push_str(
        "</div><p class=\"note\">Green: graphics pipelines (a draw a call). Yellow: compute (a dispatch a call). \
         Per frame, over the frames since the page before. F9 / F11 in the game still record one frame in full.</p></body></html>",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page lists the pipelines per frame, heaviest first, and says
    /// what it cannot (the card's time) outside a timing run.
    #[test]
    fn the_page_lists_pipelines_per_frame() {
        let row = |compute, calls, cpu_ns| Row { compute, calls, cpu_ns, gpu: (0, 0), objects: (0, 0) };
        let rows = vec![
            ("[mesh aaaa, pixel bbbb]".to_owned(), row(false, 300, 3_000_000)),
            ("[compute cccc]".to_owned(), row(true, 10, 9_000_000)),
            ("[vertex <x>]".to_owned(), row(false, 20, 1_000_000)),
        ];
        let p = page("5.5 fps, 182 ms/frame", rows.clone(), 10, Duration::from_secs(2), 3);
        assert!(p.contains("32 draws with 2 graphics pipelines, 1 dispatches with 1 compute pipelines"));
        assert!(p.contains("binding them took the recording threads 1.3 ms a frame"));
        assert!(p.contains("needs a timing run"));
        assert!(p.contains("[vertex &lt;x&gt;]") && !p.contains("card ms"));
        // by cost: the compute pipeline first; by calls: the mesh one
        let cost = p.find("Heaviest on the CPU").unwrap();
        let calls = p.find("Most calls").unwrap();
        assert!(p[cost..calls].find("[compute cccc]").unwrap() < p[cost..calls].find("[mesh aaaa").unwrap());
        assert!(p[calls..].find("[mesh aaaa").unwrap() < p[calls..].find("[compute cccc]").unwrap());
        assert!(p.contains("<td>30.0</td><td>0.30</td><td>10.0</td>"));
        // a timing run adds the card's column and table
        let mut timed = rows;
        timed[1].1.gpu = (10, 45_000);
        let p = page("", timed, 10, Duration::from_secs(2), 4);
        assert!(p.contains("Heaviest on the card") && p.contains("<th>card ms</th>") && p.contains("<td>4.50</td>"));
        assert!(p.contains("the frame line has not come yet"));
    }
}
