//! Diagnostics for Hydrate Shader Diagnostics, `VICTORY_HYDRATE_LOG=<file>`.
//!
//! Hydrate is a collector that runs beside an application: it watches for
//! the process, and reads what the application itself appends to a log
//! while a capture is on, one JSON object a line:
//!
//! ```json
//! {"category":"shader_compilation","severity":"error","message":"..."}
//! ```
//!
//! It cannot see a shader translation, a refused pipeline or a removed
//! device from outside, so the layer says them here. With the variable set,
//! what the log already tells (warnings, errors and a few informational
//! lines of the graphics crates) is sorted into Hydrate's eleven categories
//! and appended to the file ([`Layer`], installed by `logging::init`), and
//! code that does not log through `tracing` (the crash report) calls
//! [`record`].
//!
//! Each record also carries what Hydrate's report shows apart from the
//! messages ([`Extra`]; its shareable summary leaves the messages out): when
//! the line was logged, the process, which part of VIctory wrote it, the
//! host API it is about, and where the line says them, the HRESULT, the
//! shader's stage, profile and hash, the frame and a time in milliseconds.
//! The rows of the APIs and subsystems (`victory_gpu::apis`) go to the
//! categories Hydrate's API reference gives them: the fences to
//! `synchronization`, the card's memory to `resource_residency`, root
//! signatures to `resource_bindings`; the frame statistics are
//! `performance`, an unhandled exception or a panic `crash_diagnostics`.
//!
//! A capture takes 60 seconds, 200 records, 128 KiB and 1,024 characters a
//! message, and a run logs a great deal more than that, so:
//!
//! - a line that says what an earlier one said (the same words once numbers
//!   and hashes are taken out) is counted, not written, until
//!   [`AGAIN`] has passed; the next one then carries the count;
//! - at most [`PER_WINDOW`] lines are written in [`WINDOW`]; errors may
//!   take as many again;
//! - a message is cut to [`MESSAGE_CHARS`] characters.
//!
//! Hydrate reads only what is appended while it captures, and a run that has
//! settled translates no more shaders and makes no more pipelines: so every
//! [`SUMMARY_EVERY`] the counts so far are written too, one line for the
//! shaders, one for the pipelines, and one for each other category that has
//! had lines ([`summaries`]). A capture started at any time then holds how
//! many shaders were translated and how many failed.
//!
//! The file is opened for appending and every record is written whole and
//! at once: Hydrate reads the bytes that arrive after its capture starts.
//! Nothing of the title is written here but what the log line itself held
//! (shader hashes, sizes, addresses).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Hydrate's categories (its diagnostic source format, v1.2 on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    ShaderCompilation,
    ShaderValidation,
    PipelineState,
    ResourceBindings,
    DirectxErrors,
    DeviceRemoval,
    ApplicationErrors,
    Synchronization,
    Performance,
    ResourceResidency,
    CrashDiagnostics,
}

impl Category {
    /// Every category, in the order the counts are kept and summed up.
    pub const ALL: [Category; 11] = [
        Category::ShaderCompilation,
        Category::ShaderValidation,
        Category::PipelineState,
        Category::ResourceBindings,
        Category::DirectxErrors,
        Category::DeviceRemoval,
        Category::ApplicationErrors,
        Category::Synchronization,
        Category::Performance,
        Category::ResourceResidency,
        Category::CrashDiagnostics,
    ];

    /// The name Hydrate knows the category by.
    pub fn name(self) -> &'static str {
        match self {
            Category::ShaderCompilation => "shader_compilation",
            Category::ShaderValidation => "shader_validation",
            Category::PipelineState => "pipeline_state",
            Category::ResourceBindings => "resource_bindings",
            Category::DirectxErrors => "directx_errors",
            Category::DeviceRemoval => "device_removal",
            Category::ApplicationErrors => "application_errors",
            Category::Synchronization => "synchronization",
            Category::Performance => "performance",
            Category::ResourceResidency => "resource_residency",
            Category::CrashDiagnostics => "crash_diagnostics",
        }
    }
}

/// Hydrate's severities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// Characters of a message kept (Hydrate refuses one over 1,024).
pub const MESSAGE_CHARS: usize = 1000;
/// Lines written in one [`WINDOW`] (errors: twice as many), so that a
/// capture's 60 seconds stay under its 200 records.
pub const PER_WINDOW: u32 = 24;
pub const WINDOW: Duration = Duration::from_secs(10);
/// How long a line that repeats an earlier one is only counted.
pub const AGAIN: Duration = Duration::from_secs(60);
/// How often the counts so far are written ([`summaries`]).
pub const SUMMARY_EVERY: Duration = Duration::from_secs(15);

/// Characters of an optional field kept (Hydrate refuses one over 256).
pub const FIELD_CHARS: usize = 250;

/// What a record carries besides its category, severity and message:
/// Hydrate's optional fields, each written when known. Its report shows
/// them where the shareable summary leaves the messages out (`api`,
/// `reported_source`, `code`, `id`, `profile`, `stage`, `frame`,
/// `duration_ms`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extra {
    /// When the line was logged, UTC (ISO 8601). Without it Hydrate gives a
    /// record the time it read it: the end of its capture.
    pub timestamp_utc: Option<String>,
    /// The process that wrote it (Hydrate matches it with the target's).
    pub pid: Option<u32>,
    /// The part of VIctory that wrote it.
    pub source: Option<String>,
    /// The host API it is about.
    pub source_api: Option<String>,
    /// A code's name: an exception's status, an HRESULT's
    /// (`DXGI_ERROR_DEVICE_REMOVED`), a panic.
    pub code: Option<String>,
    /// The HRESULT the line names.
    pub hresult: Option<u32>,
    /// The compiler (VIctory's translator) or the validator.
    pub compiler: Option<String>,
    /// The shader's profile (`cs_6_6`).
    pub profile: Option<String>,
    /// The shader's stage (`compute`).
    pub stage: Option<String>,
    /// The shader's hash.
    pub diagnostic_id: Option<String>,
    /// The frame the line speaks of.
    pub frame: Option<u64>,
    /// A time in milliseconds: a frame's, or what a frame waited.
    pub duration_ms: Option<f64>,
}

/// `text` escaped for a JSON string and cut to `chars` characters.
fn escape(out: &mut String, text: &str, chars: usize) {
    for c in text.chars().take(chars) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
}

/// One record as Hydrate reads it: the three keys, each once, and the
/// message cut to [`MESSAGE_CHARS`] characters.
pub fn line(category: Category, severity: Severity, message: &str) -> String {
    line_with(category, severity, message, &Extra::default())
}

/// [`line`] with the optional fields `extra` has, each once, with the
/// names and types Hydrate takes (it refuses a record with a field it does
/// not know): strings cut to [`FIELD_CHARS`], `pid` and `frame` integers,
/// `hresult` `0x` and eight hexadecimal digits, `duration_ms` a number.
pub fn line_with(category: Category, severity: Severity, message: &str, extra: &Extra) -> String {
    let mut out = String::with_capacity(message.len().min(MESSAGE_CHARS) + 64);
    out.push_str("{\"category\":\"");
    out.push_str(category.name());
    out.push_str("\",\"severity\":\"");
    out.push_str(severity.name());
    out.push_str("\",\"message\":\"");
    escape(&mut out, message, MESSAGE_CHARS);
    out.push('"');
    let text = |out: &mut String, key: &str, value: &Option<String>| {
        if let Some(value) = value {
            let _ = write!(out, ",\"{key}\":\"");
            escape(out, value, FIELD_CHARS);
            out.push('"');
        }
    };
    text(&mut out, "timestamp_utc", &extra.timestamp_utc);
    if let Some(pid) = extra.pid {
        let _ = write!(out, ",\"pid\":{pid}");
    }
    text(&mut out, "source", &extra.source);
    text(&mut out, "source_api", &extra.source_api);
    text(&mut out, "code", &extra.code);
    if let Some(hresult) = extra.hresult {
        let _ = write!(out, ",\"hresult\":\"0x{hresult:08X}\"");
    }
    text(&mut out, "compiler", &extra.compiler);
    text(&mut out, "profile", &extra.profile);
    text(&mut out, "stage", &extra.stage);
    text(&mut out, "diagnostic_id", &extra.diagnostic_id);
    if let Some(frame) = extra.frame {
        let _ = write!(out, ",\"frame\":{frame}");
    }
    if let Some(ms) = extra.duration_ms.filter(|ms| ms.is_finite() && *ms >= 0.0) {
        let _ = write!(out, ",\"duration_ms\":{ms:.3}");
    }
    out.push_str("}\n");
    out
}

/// A time since the Unix epoch as Hydrate's `timestamp_utc`: ISO 8601 in
/// UTC, to the microsecond.
pub fn utc_stamp(since_epoch: Duration) -> String {
    let secs = since_epoch.as_secs();
    let (days, of_day) = ((secs / 86_400) as i64, secs % 86_400);
    // The civil date of a day count (Howard Hinnant's `civil_from_days`).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:06}Z",
        of_day / 3_600,
        of_day / 60 % 60,
        of_day % 60,
        since_epoch.subsec_micros()
    )
}

/// Now, as [`utc_stamp`].
fn now_stamp() -> Option<String> {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok().map(utc_stamp)
}

/// What makes two messages "the same line": the words, with every number
/// and every run of hexadecimal digits (hashes, addresses, sizes) left out.
pub fn kind_of(category: Category, message: &str) -> u64 {
    // FNV-1a over the category and the words.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ category as u64;
    // The first 240 marks of the words: a number counts as one, however
    // many digits it has, so a long line whose numbers grow (the frame
    // statistics) stays one kind.
    let marks = std::cell::Cell::new(0usize);
    let mut mix = |b: u8| {
        h = (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        marks.set(marks.get() + 1);
    };
    let mut bytes = message.bytes().peekable();
    while let Some(b) = bytes.next() {
        if marks.get() >= 240 {
            break;
        }
        if b.is_ascii_digit() {
            // a number, or a hash or address that begins with a digit
            while bytes.peek().is_some_and(|n| n.is_ascii_hexdigit() || *n == b'x' || *n == b'.') {
                bytes.next();
            }
            mix(b'#');
        } else if b.is_ascii_hexdigit() {
            // letters a-f: a hash if six or more hexadecimal digits follow one another
            let mut run = vec![b];
            while bytes.peek().is_some_and(u8::is_ascii_hexdigit) {
                run.push(bytes.next().unwrap_or(b'0'));
            }
            if run.len() >= 6 {
                mix(b'#');
            } else {
                run.into_iter().for_each(&mut mix);
            }
        } else {
            mix(b);
        }
    }
    h
}

/// Which category and severity a log line is, if it is one Hydrate has a
/// category for. `level`: 0 error, 1 warning, 2 information.
///
/// Information lines are taken only where they are the record itself (a
/// shader translated, a host pipeline made, the device's own report);
/// warnings and errors by what they speak of. A warning that names none of
/// the categories' subjects is left out; an error that names none is the
/// application's.
/// The category of a row of the APIs and subsystems, by the name the row
/// begins with (`victory_gpu::apis::Api::name`), as Hydrate's API reference
/// files it, and the host calls the row counts.
fn api_row(row: &str) -> Option<(Category, &'static str)> {
    const ROWS: [(&str, Category, &str); 11] = [
        ("D3D12CreateDevice", Category::DirectxErrors, "D3D12CreateDevice"),
        ("Root signatures", Category::ResourceBindings, "ID3D12Device::CreateRootSignature"),
        ("Pipeline state creation", Category::PipelineState, PIPELINE_API),
        ("DXIL validation", Category::ShaderValidation, VALIDATE_API),
        ("Descriptor heaps", Category::ResourceBindings, "ID3D12Device::CreateDescriptorHeap"),
        ("Resource residency", Category::ResourceResidency, RESIDENCY_API),
        ("Command queues and fences", Category::Synchronization, FENCE_API),
        ("Present", Category::DirectxErrors, PRESENT_API),
        ("GetDeviceRemovedReason", Category::DeviceRemoval, REMOVED_API),
        // the title's lookups and loads, which VIctory answers
        ("GetProcAddress", Category::ApplicationErrors, "GetProcAddress (title calls)"),
        ("LoadLibrary", Category::ApplicationErrors, "LoadLibrary (title calls)"),
    ];
    ROWS.iter().find(|(name, ..)| row.starts_with(name)).map(|&(_, category, api)| (category, api))
}

/// The host calls behind the records of a category, where it has one.
/// Hydrate's shareable summary shows a field only when it looks like a name
/// (letters, digits, spaces and `_:.()-`): one with a comma, an asterisk or
/// an apostrophe it leaves out, so these are plain names.
const VALIDATE_API: &str = "IDxcValidator::Validate (dxil.dll)";
/// The pipelines are made by the three kinds of call (graphics, compute and
/// stream, the stream one for mesh pipelines) or loaded from the library;
/// named by the one that makes any kind.
const PIPELINE_API: &str = "ID3D12Device2::CreatePipelineState";
const RESIDENCY_API: &str = "IDXGIAdapter3::QueryVideoMemoryInfo";
const FENCE_API: &str = "ID3D12Fence::SetEventOnCompletion";
const PRESENT_API: &str = "IDXGISwapChain::Present";
const REMOVED_API: &str = "ID3D12Device::GetDeviceRemovedReason";

/// The frame statistics line of `victory_gpu::host` ("5.5 fps, 182.0
/// ms/frame (...)"): its frames a second and milliseconds a frame.
pub fn frame_stats(text: &str) -> Option<(f64, f64)> {
    let (fps, rest) = text.split_once(" fps, ")?;
    let (ms, _) = rest.split_once(" ms/frame")?;
    let (fps, ms) = (fps.trim().parse::<f64>().ok()?, ms.trim().parse::<f64>().ok()?);
    (fps.is_finite() && ms.is_finite()).then_some((fps, ms))
}

/// The number of milliseconds just before `unit` in `text` ("12.6 ms a
/// frame"): what a row of the APIs gives a frame.
fn ms_before(text: &str, unit: &str) -> Option<f64> {
    let head = &text[..text.find(unit)?];
    let number = head.rsplit(|c: char| !(c.is_ascii_digit() || c == '.')).next()?;
    number.parse().ok()
}

/// The first `0x` and eight hexadecimal digits in `text` (not more), whose
/// top bit is set: an HRESULT ("0x887A0005") or an exception's code
/// ("0xc0000005"); `0x80000000` and up.
fn failure_code(text: &str) -> Option<u32> {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(at) = text[from..].find("0x").map(|n| from + n) {
        let digits = &bytes[at + 2..];
        let n = digits.iter().take_while(|b| b.is_ascii_hexdigit()).count();
        if n == 8
            && let Ok(code) = u32::from_str_radix(&text[at + 2..at + 10], 16)
            && code >= 0x8000_0000
        {
            return Some(code);
        }
        from = at + 2;
    }
    None
}

/// The host profile of a console stage, where the stage alone decides it
/// (shader model 6.6, `victory_shader::emit::shader`): a compute program is
/// a compute shader, a pixel program a pixel shader. The others can run as
/// another kind (a vertex program as a mesh or geometry shader), which only
/// the translated shader's header tells; a line that has it says it as its
/// `profile` field.
fn profile_of(stage: &str) -> Option<String> {
    let kind = match stage {
        "pixel" => "ps",
        "compute" => "cs",
        _ => return None,
    };
    Some(format!("{kind}_6_6"))
}

/// The validator's version, as the log tells it at the start
/// (`VICTORY_VALIDATE=1`): for the records of its checks.
static VALIDATOR: OnceLock<String> = OnceLock::new();

/// The last frame statistics the log told (frames a second, milliseconds a
/// frame): the summaries' performance record.
static LAST_FRAME: Mutex<Option<(f64, f64)>> = Mutex::new(None);

/// The latest frame a line named (0: none yet): the summaries' frame.
static FRAME_NO: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The fields of a log line Hydrate has a place for, by their names in the
/// line: `stage`, `hash`, `profile` (the host's, which the translated
/// shader's own header gives: `victory_gpu::pipeline`) and `frame`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fields<'a> {
    pub stage: Option<&'a str>,
    pub hash: Option<&'a str>,
    pub profile: Option<&'a str>,
    pub frame: Option<u64>,
}

/// The frame a line names in its words ("(frame 4211)").
fn frame_in(text: &str) -> Option<u64> {
    let at = text.find("(frame ")? + "(frame ".len();
    let digits: String = text[at..].chars().take_while(char::is_ascii_digit).collect();
    if text[at + digits.len()..].starts_with(')') { digits.parse().ok() } else { None }
}

/// The name of an HRESULT or an exception's status, where it is one of
/// those a run of VIctory meets.
fn code_name(code: u32) -> Option<&'static str> {
    Some(match code {
        0x8000_4001 => "E_NOTIMPL",
        0x8000_4002 => "E_NOINTERFACE",
        0x8000_4003 => "E_POINTER",
        0x8000_4005 => "E_FAIL",
        0x8007_000E => "E_OUTOFMEMORY",
        0x8007_0057 => "E_INVALIDARG",
        0x887A_0001 => "DXGI_ERROR_INVALID_CALL",
        0x887A_0002 => "DXGI_ERROR_NOT_FOUND",
        0x887A_0004 => "DXGI_ERROR_UNSUPPORTED",
        0x887A_0005 => "DXGI_ERROR_DEVICE_REMOVED",
        0x887A_0006 => "DXGI_ERROR_DEVICE_HUNG",
        0x887A_0007 => "DXGI_ERROR_DEVICE_RESET",
        0x887A_000A => "DXGI_ERROR_WAS_STILL_DRAWING",
        0x887A_0020 => "DXGI_ERROR_DRIVER_INTERNAL_ERROR",
        0x887A_0022 => "DXGI_ERROR_NOT_CURRENTLY_AVAILABLE",
        0x887A_0027 => "DXGI_ERROR_WAIT_TIMEOUT",
        0x8000_0003 => "EXCEPTION_BREAKPOINT",
        0xC000_0005 => "EXCEPTION_ACCESS_VIOLATION",
        0xC000_0006 => "EXCEPTION_IN_PAGE_ERROR",
        0xC000_001D => "EXCEPTION_ILLEGAL_INSTRUCTION",
        0xC000_008C => "EXCEPTION_ARRAY_BOUNDS_EXCEEDED",
        0xC000_0094 => "EXCEPTION_INT_DIVIDE_BY_ZERO",
        0xC000_0096 => "EXCEPTION_PRIV_INSTRUCTION",
        0xC000_00FD => "EXCEPTION_STACK_OVERFLOW",
        0xC000_0374 => "STATUS_HEAP_CORRUPTION",
        0xC000_0409 => "STATUS_STACK_BUFFER_OVERRUN",
        _ => return None,
    })
}

/// What a line of the log says of itself, beside its words: the fields
/// Hydrate shows in its report ([`Extra`]). `target` is the line's,
/// `fields` those of its fields Hydrate has a place for.
pub fn details(category: Category, target: &str, text: &str, fields: Fields<'_>) -> Extra {
    let frame = fields.frame.or_else(|| frame_in(text));
    let mut e = Extra { source: Some(format!("VIctory {target}")), frame, ..Extra::default() };
    if let Some(row) = text.strip_prefix("api ") {
        e.source_api = api_row(row).map(|(_, api)| api.to_owned());
        // the queues' wait and Present's time, a frame
        e.duration_ms = ms_before(row, " ms a frame").or_else(|| ms_before(row, " ms inside Present a frame"));
    }
    let api = |api: &str| Some(api.to_owned());
    match category {
        Category::ShaderCompilation => {
            e.compiler = Some(format!("VIctory {} shader translator (console GPU code to DXIL)", env!("CARGO_PKG_VERSION")));
        }
        Category::ShaderValidation => {
            e.compiler = VALIDATOR.get().map(|v| format!("dxil.dll validator {v}"));
            e.source_api = e.source_api.or_else(|| api(VALIDATE_API));
        }
        Category::PipelineState if text.starts_with("host pipeline library") => {
            e.source_api = e.source_api.or_else(|| api("ID3D12Device1::CreatePipelineLibrary"));
        }
        Category::PipelineState if text.starts_with("host pipeline") && !text.starts_with("host pipeline wait") => {
            e.source_api = e.source_api.or_else(|| api(PIPELINE_API));
        }
        Category::Performance => {
            if let Some((_, ms)) = frame_stats(text) {
                e.duration_ms = Some(ms);
            }
            e.source_api = e.source_api.or_else(|| api(PRESENT_API));
        }
        Category::ResourceResidency => e.source_api = e.source_api.or_else(|| api(RESIDENCY_API)),
        Category::Synchronization => e.source_api = e.source_api.or_else(|| api(FENCE_API)),
        Category::DeviceRemoval => e.source_api = e.source_api.or_else(|| api(REMOVED_API)),
        _ => {}
    }
    match failure_code(text) {
        // an exception's status (0xC0000005), whatever its top bits
        Some(code) if category == Category::CrashDiagnostics => {
            e.code = Some(code_name(code).map_or_else(|| format!("0x{code:08X}"), str::to_owned));
        }
        // a panic has no status
        None if category == Category::CrashDiagnostics && text.contains("panic") => e.code = Some("RUST_PANIC".to_owned()),
        // an HRESULT: the device's, D3D12's, DXGI's, Windows'
        Some(code) if code >> 28 == 0x8 => {
            e.hresult = Some(code);
            e.code = code_name(code).map(str::to_owned);
        }
        _ => {}
    }
    if let Some(stage) = fields.stage {
        e.stage = Some(stage.to_owned());
    }
    e.profile = fields.profile.map(str::to_owned).or_else(|| fields.stage.and_then(profile_of));
    e.diagnostic_id = fields.hash.map(str::to_owned);
    e
}

pub fn classify(level: u8, target: &str, text: &str) -> Option<(Category, Severity)> {
    let severity = match level {
        0 => Severity::Error,
        1 => Severity::Warning,
        _ => Severity::Info,
    };
    let has = |words: &[&str]| words.iter().any(|w| text.contains(w));
    // A row of the APIs and subsystems (`victory_gpu::apis`): the category
    // is the row's own, the severity the line's (a good row is logged as
    // information, another as a warning, a removed device as an error).
    if let Some(row) = text.strip_prefix("api ") {
        return api_row(row).map(|(category, _)| (category, severity));
    }
    // The streaming of textures the title rewrites (a hundred lines a
    // second in the world) is no binding fault.
    if text.starts_with("a texture picked by index changed") {
        return None;
    }
    let lost = has(&[
        "device removed",
        "device was removed",
        "DEVICE_REMOVED",
        "DEVICE_HUNG",
        "DEVICE_RESET",
        "0x887A0005",
        "0x887a0005",
        "0x887A0006",
        "0x887a0006",
        "0x887A0007",
        "0x887a0007",
        "DRED",
    ]);
    if lost {
        return Some((Category::DeviceRemoval, severity.max(Severity::Warning)));
    }
    // A crash: an unhandled exception's report, a panic.
    if has(&["unhandled exception", "panicked", "panic on thread"]) {
        return Some((Category::CrashDiagnostics, severity.max(Severity::Warning)));
    }
    if severity == Severity::Info {
        return if text.starts_with("shader translated") {
            Some((Category::ShaderCompilation, Severity::Info))
        } else if text.starts_with("translated shaders are checked by dxil.dll") {
            Some((Category::ShaderValidation, Severity::Info))
        } else if text.starts_with("host pipeline ") || text.starts_with("the ray-tracing structure builder") {
            Some((Category::PipelineState, Severity::Info))
        } else if target.ends_with("::host") && frame_stats(text).is_some() {
            Some((Category::Performance, Severity::Info))
        } else if text.starts_with("the system changed the budget of the card's memory") || text.starts_with("video memory as the system counts it") {
            Some((Category::ResourceResidency, Severity::Info))
        } else {
            None
        };
    }
    let category = if has(&["does not translate", "translation", "translat", "shader cache", "bitcode"]) || target.contains("victory_shader") {
        if has(&["validator", "validation", "validate"]) { Category::ShaderValidation } else { Category::ShaderCompilation }
    } else if has(&["validator", "validation", "validate"]) {
        Category::ShaderValidation
    } else if has(&["pipeline", "root signature", "input layout", "PipelineLibrary"]) {
        Category::PipelineState
    } else if has(&["the card has no room", "no room for", "budget", "E_OUTOFMEMORY", "out of memory", "MakeResident", "Evict", "residency"]) {
        Category::ResourceResidency
    } else if has(&["fence", "not waited for", "waits for memory values", "waiting for the card", "timed out", "stall"]) {
        Category::Synchronization
    } else if has(&["view", "descriptor", "table", "slot", "sampler", "constant buffer", "heap"]) {
        Category::ResourceBindings
    } else if has(&["HRESULT", "0x8007", "0x887", "E_INVALIDARG", "host GPU failure", "D3D12", "no colour twin", "no twin for"])
        || text.contains("error=") && target.contains("victory_gpu")
    {
        Category::DirectxErrors
    } else if severity == Severity::Error {
        Category::ApplicationErrors
    } else {
        return None;
    };
    // A shader that does not translate and a pipeline the host refuses are
    // logged as warnings (the run goes on without their draws): for the
    // category they are its errors.
    let failed = matches!(category, Category::ShaderCompilation | Category::ShaderValidation | Category::PipelineState)
        && has(&["does not translate", "refused", "no host pipeline", "without a host pipeline", "failed"]);
    Some((category, if failed { Severity::Error } else { severity }))
}

/// Lines of each category and severity since the start, those held back
/// by the limits included.
static COUNTS: [[std::sync::atomic::AtomicU64; 3]; 11] = [const { [const { std::sync::atomic::AtomicU64::new(0) }; 3] }; 11];

fn count(category: Category, severity: Severity) {
    COUNTS[category as usize][severity as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The counts so far as records: always one for the shaders, one for the
/// pipelines and one for crashes (a capture then shows the source connected,
/// and watching, even while nothing new is translated and nothing has
/// crashed), and one for each other category that has had lines;
/// the performance record with the last frame statistics (`frame`: frames a
/// second, milliseconds a frame) as its time. A summary is information,
/// whatever it counts; each carries `base`'s fields (the time, the process).
pub fn summaries(counts: &[[u64; 3]; 11], seconds: u64, frame: Option<(f64, f64)>, base: &Extra) -> Vec<String> {
    let mut out = Vec::new();
    for c in Category::ALL {
        let [info, warnings, errors] = counts[c as usize];
        let always = matches!(c, Category::ShaderCompilation | Category::PipelineState | Category::CrashDiagnostics);
        if !always && info + warnings + errors == 0 {
            continue;
        }
        let mut extra = base.clone();
        let what = match c {
            Category::ShaderCompilation => format!("{info} shaders translated, {errors} did not translate, {warnings} other warnings"),
            // the crash handler and the panic hook report here: none so far is a finding
            Category::CrashDiagnostics if errors + warnings == 0 => "no unhandled exception and no panic so far (the crash handler and the panic hook are on)".to_owned(),
            Category::CrashDiagnostics => format!("{errors} unhandled exceptions or panics, {warnings} other crash lines"),
            Category::PipelineState => format!("{info} pipeline lines (host pipelines made, the pipeline library), {errors} refused or missing, {warnings} other warnings"),
            Category::Performance => match frame {
                Some((fps, ms)) => {
                    extra.duration_ms = Some(ms);
                    extra.source_api = Some(PRESENT_API.to_owned());
                    format!(
                        "frame time {ms:.1} ms ({fps:.1} frames a second): presented frames over the last second of the run, timed by VIctory with the CPU's clock; {errors} errors, {warnings} warnings, {info} information lines"
                    )
                }
                None => format!("{errors} errors, {warnings} warnings, {info} information lines"),
            },
            _ => format!("{errors} errors, {warnings} warnings, {info} information lines"),
        };
        out.push(line_with(c, Severity::Info, &format!("VIctory summary, {seconds} s into the run: {what}"), &extra));
    }
    out
}

/// What is decided for a record: written (with how many like it were held
/// back since the last one written), or held back.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Write(u64),
    Hold,
}

/// The limits, apart from the file: which records are written.
#[derive(Default)]
pub struct Limits {
    /// By kind: when one was last written, and those held back since.
    seen: HashMap<u64, (Duration, u64)>,
    /// The window (its number) and the lines written in it.
    window: (u64, u32),
}

impl Limits {
    /// Decides for a record of `kind` and `severity` at `now` (since the start).
    pub fn decide(&mut self, kind: u64, severity: Severity, now: Duration) -> Verdict {
        if let Some((last, held)) = self.seen.get_mut(&kind)
            && now.saturating_sub(*last) < AGAIN
        {
            *held += 1;
            return Verdict::Hold;
        }
        let window = now.as_secs() / WINDOW.as_secs().max(1);
        if self.window.0 != window {
            self.window = (window, 0);
        }
        let room = if severity == Severity::Error { PER_WINDOW * 2 } else { PER_WINDOW };
        if self.window.1 >= room {
            // Not noted as seen: it is tried again when there is room.
            return Verdict::Hold;
        }
        self.window.1 += 1;
        let held = self.seen.insert(kind, (now, 0)).map_or(0, |(_, held)| held);
        // (kinds are few: the log's own lines; kept for the run)
        Verdict::Write(held)
    }
}

struct Sink {
    file: std::fs::File,
    started: Instant,
    limits: Limits,
}

fn sink() -> Option<&'static Mutex<Sink>> {
    static SINK: OnceLock<Option<Mutex<Sink>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let path = std::path::PathBuf::from(std::env::var_os("VICTORY_HYDRATE_LOG").filter(|v| !v.is_empty())?);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut file = match std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("VIctory: VICTORY_HYDRATE_LOG: {} cannot be opened: {e}", path.display());
                return None;
            }
        };
        // The first record says the log is connected (a capture that starts
        // later does not see it; the lines after it are the evidence).
        let hello = format!(
            "VIctory {} diagnostics connected (pid {}): shader translation, pipeline, binding, device and crash lines follow as the run logs them",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        );
        let _ = file.write_all(line_with(Category::ApplicationErrors, Severity::Info, &hello, &stamped("VIctory")).as_bytes());
        // The counts so far, every `SUMMARY_EVERY`, for as long as the process lives.
        let _ = std::thread::Builder::new().name("victory-hydrate".into()).spawn(|| {
            loop {
                std::thread::sleep(SUMMARY_EVERY);
                let Some(sink) = sink() else { return };
                let counts = COUNTS.each_ref().map(|c| c.each_ref().map(|n| n.load(std::sync::atomic::Ordering::Relaxed)));
                let frame = *LAST_FRAME.lock().unwrap_or_else(|e| e.into_inner());
                let mut base = stamped("VIctory summary");
                base.frame = Some(FRAME_NO.load(std::sync::atomic::Ordering::Relaxed)).filter(|&n| n > 0);
                let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
                let text = summaries(&counts, sink.started.elapsed().as_secs(), frame, &base).concat();
                let _ = sink.file.write_all(text.as_bytes());
            }
        });
        Some(Mutex::new(Sink { file, started: Instant::now(), limits: Limits::default() }))
    })
    .as_ref()
}

/// Whether the diagnostics file is on.
pub fn on() -> bool {
    sink().is_some()
}

/// The fields every record of this process has: the time now, the process,
/// and `source`.
fn stamped(source: &str) -> Extra {
    Extra { timestamp_utc: now_stamp(), pid: Some(std::process::id()), source: Some(source.to_owned()), ..Extra::default() }
}

/// Appends a record, within the limits (see the module notes). Does nothing
/// without `VICTORY_HYDRATE_LOG`.
pub fn record(category: Category, severity: Severity, message: &str) {
    record_with(category, severity, message, Extra::default());
}

/// [`record`] with the fields `extra` has (the time and the process are
/// filled in, and the source if it has none).
pub fn record_with(category: Category, severity: Severity, message: &str, extra: Extra) {
    let Some(sink) = sink() else { return };
    write(&mut sink.lock().unwrap_or_else(|e| e.into_inner()), category, severity, message, extra);
}

/// [`record`] for the report of an unhandled exception (a crash, with its
/// code): the crashing thread may be the one that holds the file's lock,
/// interrupted in the middle of a line, so the lock is tried for a short
/// while and the record given up if it stays held.
pub fn record_from_crash(message: &str) {
    let Some(sink) = sink() else { return };
    let category = Category::CrashDiagnostics;
    let mut extra = details(category, "victory_loader::crash", message, Fields::default());
    extra.source = Some("VIctory crash report".to_owned());
    for _ in 0..40 {
        match sink.try_lock() {
            Ok(mut sink) => return write(&mut sink, category, Severity::Error, message, extra),
            Err(std::sync::TryLockError::Poisoned(e)) => {
                return write(&mut e.into_inner(), category, Severity::Error, message, extra);
            }
            Err(std::sync::TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(1)),
        }
    }
}

fn write(sink: &mut Sink, category: Category, severity: Severity, message: &str, extra: Extra) {
    count(category, severity);
    let now = sink.started.elapsed();
    let extra = Extra {
        timestamp_utc: extra.timestamp_utc.or_else(now_stamp),
        pid: extra.pid.or(Some(std::process::id())),
        source: extra.source.or_else(|| Some("VIctory".to_owned())),
        ..extra
    };
    let text = match sink.limits.decide(kind_of(category, message), severity, now) {
        Verdict::Hold => return,
        Verdict::Write(0) => line_with(category, severity, message, &extra),
        Verdict::Write(held) => {
            let cut: String = message.chars().take(MESSAGE_CHARS - 60).collect();
            line_with(category, severity, &format!("{cut} ({held} more like it since the last such line)"), &extra)
        }
    };
    let _ = sink.file.write_all(text.as_bytes());
}

/// The log's lines into the diagnostics file: see the module notes.
pub struct Layer;

/// The layer, if `VICTORY_HYDRATE_LOG` is set.
pub fn layer() -> Option<Layer> {
    on().then_some(Layer)
}

#[derive(Default)]
struct Text {
    message: String,
    fields: String,
    /// The fields Hydrate has a place for ([`Fields`]).
    stage: Option<String>,
    hash: Option<String>,
    profile: Option<String>,
    frame: Option<u64>,
}

impl Text {
    fn keep(&mut self, name: &str, value: &str) {
        let value = value.trim_matches('"');
        if value.is_empty() || value == "unknown" {
            return;
        }
        match name {
            "stage" => self.stage = Some(value.to_owned()),
            "hash" => self.hash = Some(value.to_owned()),
            "profile" => self.profile = Some(value.to_owned()),
            "frame" => self.frame = value.parse().ok(),
            _ => {}
        }
    }

    fn fields(&self) -> Fields<'_> {
        Fields { stage: self.stage.as_deref(), hash: self.hash.as_deref(), profile: self.profile.as_deref(), frame: self.frame }
    }
}

impl tracing::field::Visit for Text {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
            self.keep(field.name(), value);
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let value = format!("{value:?}");
            let _ = write!(self.fields, " {}={value}", field.name());
            self.keep(field.name(), &value);
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Layer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        let meta = event.metadata();
        let level = match *meta.level() {
            tracing::Level::ERROR => 0,
            tracing::Level::WARN => 1,
            tracing::Level::INFO => 2,
            _ => return,
        };
        // Information lines are many (the frame statistics, every few
        // seconds): only the targets that have records among them are read.
        let target = meta.target();
        if level == 2 && !(target.contains("pipeline") || target.ends_with("::apis") || target.ends_with("::host")) {
            return;
        }
        let mut text = Text::default();
        event.record(&mut text);
        let fields = std::mem::take(&mut text.fields);
        text.message.push_str(&fields);
        // what later records need: the validator's version, the last frame statistics
        if let Some(version) = text.message.strip_prefix("translated shaders are checked by dxil.dll's validator, version ") {
            let _ = VALIDATOR.set(version.trim().to_owned());
        }
        if target.ends_with("::host")
            && let Some(frame) = frame_stats(&text.message)
        {
            *LAST_FRAME.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
        }
        if let Some((category, severity)) = classify(level, target, &text.message) {
            let extra = details(category, target, &text.message, text.fields());
            if let Some(frame) = extra.frame {
                FRAME_NO.fetch_max(frame, std::sync::atomic::Ordering::Relaxed);
            }
            record_with(category, severity, &format!("{target}: {}", text.message), extra);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row of the APIs and subsystems is a record of the row's category
    /// at the line's severity, whatever its words: the categories Hydrate's
    /// API reference gives them.
    #[test]
    fn a_row_of_the_api_board_keeps_its_category() {
        let of = |level, text| classify(level, "victory_gpu::apis", text);
        assert_eq!(
            of(2, "api GetDeviceRemovedReason [GPU device failures]: S_OK: the device is alive"),
            Some((Category::DeviceRemoval, Severity::Info))
        );
        assert_eq!(
            of(0, "api GetDeviceRemovedReason [GPU device failures]: the device is REMOVED: 0x887A0006"),
            Some((Category::DeviceRemoval, Severity::Error))
        );
        assert_eq!(
            of(2, "api Pipeline state creation [Shader and pipeline compatibility]: 1567 made or loaded, 0 refused by the host"),
            Some((Category::PipelineState, Severity::Info))
        );
        assert_eq!(
            of(1, "api Descriptor heaps [Texture and buffer bindings]: 4 host heaps made, 0 failed; 61 buffer views refused"),
            Some((Category::ResourceBindings, Severity::Warning))
        );
        assert_eq!(
            of(1, "api GetProcAddress [Dynamically resolved API dependencies]: 40 lookups, 2 found nothing | kernelx.dll!Foo"),
            Some((Category::ApplicationErrors, Severity::Warning))
        );
        assert_eq!(of(2, "api DXIL validation [x]: 1955 shaders translated"), Some((Category::ShaderValidation, Severity::Info)));
        assert_eq!(of(2, "api Present [x]: 12 presents"), Some((Category::DirectxErrors, Severity::Info)));
        assert_eq!(of(2, "api Root signatures [Shader resource compatibility]: 31 made"), Some((Category::ResourceBindings, Severity::Info)));
        assert_eq!(
            of(1, "api Resource residency [GPU memory availability]: the card: 10,300 MB in use of a budget of 11,242 MB (92%)"),
            Some((Category::ResourceResidency, Severity::Warning))
        );
        assert_eq!(
            of(1, "api Command queues and fences [CPU/GPU synchronization and stalls]: 3 waits on the fence"),
            Some((Category::Synchronization, Severity::Warning))
        );
        assert_eq!(of(2, "api Nothing known: 1"), None);
    }

    /// A row's record names the host call it counts and, for the queues and
    /// Present, what a frame spent in them.
    #[test]
    fn a_row_says_its_api_and_its_time() {
        let queues = "api Command queues and fences [CPU/GPU synchronization and stalls]: 6022 waits on the fence in the last 61 s: 146.7 ms a frame our threads stood waiting for the card (the submitting thread, and those that land what the card wrote); the longest wait since the start 1210 ms";
        let e = details(Category::Synchronization, "victory_gpu::apis", queues, Fields::default());
        assert_eq!(e.source_api.as_deref(), Some(FENCE_API));
        assert_eq!(e.duration_ms, Some(146.7));
        assert_eq!(e.source.as_deref(), Some("VIctory victory_gpu::apis"));
        let present = "api Present [Frame presentation]: 244 presents in the last 61.0 s (4.0 a second), 0.52 ms inside Present a frame (the longest 31 ms); 0 failed since the start";
        let e = details(Category::DirectxErrors, "victory_gpu::apis", present, Fields::default());
        assert_eq!(e.source_api.as_deref(), Some(PRESENT_API));
        assert_eq!(e.duration_ms, Some(0.52));
        let residency = "api Resource residency [GPU memory availability]: the card: 10300 MB in use of a budget of 11242 MB (92%)";
        let e = details(Category::ResourceResidency, "victory_gpu::apis", residency, Fields::default());
        assert_eq!(e.source_api.as_deref(), Some(RESIDENCY_API));
        assert_eq!(e.duration_ms, None);
        // a removed device: the HRESULT and its name
        let removed = "api GetDeviceRemovedReason [GPU device failures]: the device is REMOVED: 0x887A0006";
        let e = details(Category::DeviceRemoval, "victory_gpu::apis", removed, Fields::default());
        assert_eq!((e.hresult, e.code.as_deref()), (Some(0x887A_0006), Some("DXGI_ERROR_DEVICE_HUNG")));
        assert_eq!(e.source_api.as_deref(), Some(REMOVED_API));
    }

    /// A shader's records carry its stage, hash, profile and the compiler.
    #[test]
    fn a_shaders_record_says_which_shader() {
        let f = Fields { stage: Some("vertex"), hash: Some("7e39ed4b99383eeb"), profile: Some("ms_6_6"), frame: None };
        let e = details(Category::ShaderCompilation, "victory_gpu::pipeline", "shader translated", f);
        assert_eq!(e.stage.as_deref(), Some("vertex"));
        // the host's kind as the translated shader says it, not the console's
        assert_eq!(e.profile.as_deref(), Some("ms_6_6"));
        assert_eq!(e.diagnostic_id.as_deref(), Some("7e39ed4b99383eeb"));
        assert!(e.compiler.as_deref().is_some_and(|c| c.starts_with("VIctory ") && c.contains("shader translator")));
        // without the header (a shader that does not translate): the kinds a stage decides
        let f = Fields { stage: Some("compute"), ..Fields::default() };
        assert_eq!(details(Category::ShaderCompilation, "victory_gpu::pipeline", "x", f).profile.as_deref(), Some("cs_6_6"));
        let f = Fields { stage: Some("pixel"), ..Fields::default() };
        assert_eq!(details(Category::ShaderCompilation, "victory_gpu::pipeline", "x", f).profile.as_deref(), Some("ps_6_6"));
        let f = Fields { stage: Some("vertex"), ..Fields::default() };
        assert_eq!(details(Category::ShaderCompilation, "victory_gpu::pipeline", "x", f).profile, None);
        // the pipeline library, a pipeline made, the wait budget (no call)
        let api_of = |text| details(Category::PipelineState, "victory_gpu::pipeline::host", text, Fields::default()).source_api;
        assert_eq!(api_of("host pipeline library loaded bytes=344843210 mapped=true").as_deref(), Some("ID3D12Device1::CreatePipelineLibrary"));
        assert_eq!(api_of("host pipeline shaders=[compute cedbff472d562f54]").as_deref(), Some(PIPELINE_API));
        assert_eq!(api_of("host pipeline wait budget per frame millis=500"), None);
        // a validation record: the validator, and the call that checks
        let e = details(Category::ShaderValidation, "victory_gpu::pipeline", "shader failed DXIL validation (dxil.dll)", Fields::default());
        assert_eq!(e.source_api.as_deref(), Some(VALIDATE_API));
    }

    /// The frame statistics and the frame a line names.
    #[test]
    fn frames_are_read_from_the_words() {
        assert_eq!(frame_stats("5.5 fps, 182.0 ms/frame (110.2 preparing: copies 3.1 ms)"), Some((5.5, 182.0)));
        assert_eq!(frame_stats("6 fps 163.4 ms/frame"), None);
        assert_eq!(frame_stats("nan fps, 1.0 ms/frame"), None);
        let e = details(Category::Performance, "victory_gpu::host", "4.0 fps, 246.3 ms/frame (x)", Fields::default());
        assert_eq!((e.duration_ms, e.source_api.as_deref()), (Some(246.3), Some(PRESENT_API)));
        assert_eq!(frame_in("budget from 11242 MB to 10300 MB (frame 4211); at the last look"), Some(4211));
        assert_eq!(frame_in("(frame x)"), None);
        assert_eq!(frame_in("(frame 12 of 40)"), None);
        let e = details(Category::ResourceResidency, "victory_gpu::host", "the system changed the budget of the card's memory from 11242 MB to 10300 MB (frame 4211)", Fields::default());
        assert_eq!(e.frame, Some(4211));
    }

    /// An HRESULT is `0x` and eight digits with the top bit set; an
    /// address, a shorter or longer number, is not one.
    #[test]
    fn codes_are_eight_digits() {
        assert_eq!(failure_code("host pipeline refused error=0x80070057"), Some(0x8007_0057));
        assert_eq!(failure_code("at image+0x8f92a (read 0xffffffffffffffff)"), None);
        assert_eq!(failure_code("flags 0x00000010 then 0x887a0005"), Some(0x887A_0005));
        assert_eq!(failure_code("0x887A00051"), None);
        let crash = "VIctory: unhandled exception 0xc0000005 at image+0x8f92a (read 0xffffffffffffffff: outside the address space)";
        let e = details(Category::CrashDiagnostics, "victory_loader::crash", crash, Fields::default());
        assert_eq!((e.code.as_deref(), e.hresult), (Some("EXCEPTION_ACCESS_VIOLATION"), None));
        let e = details(Category::CrashDiagnostics, "victory_loader::crash", "VIctory: unhandled exception 0xe06d7363 at x", Fields::default());
        assert_eq!(e.code.as_deref(), Some("0xE06D7363"));
        let panic = "VIctory: panic on thread \"main\": panicked at crates/victory-gpu/src/draw.rs:12:5:\nindex out of bounds";
        let e = details(Category::CrashDiagnostics, "victory_loader::crash", panic, Fields::default());
        assert_eq!(e.code.as_deref(), Some("RUST_PANIC"));
    }

    /// The names in the fields are ones Hydrate's shareable summary shows:
    /// letters, digits, spaces and `_:.()-` (a comma, an asterisk or an
    /// apostrophe made it leave the field out).
    #[test]
    fn the_fields_names_are_shown() {
        let shown = |v: &str| !v.is_empty() && v.len() <= FIELD_CHARS && v.chars().all(|c| c.is_ascii_alphanumeric() || " _:.()-".contains(c));
        let rows = [
            "D3D12CreateDevice",
            "Root signatures",
            "Pipeline state creation",
            "DXIL validation",
            "Descriptor heaps",
            "Resource residency",
            "Command queues and fences",
            "Present",
            "GetDeviceRemovedReason",
            "GetProcAddress",
            "LoadLibrary",
        ];
        for row in rows {
            let (_, api) = api_row(row).unwrap_or_else(|| panic!("{row}"));
            assert!(shown(api), "{row}: {api}");
        }
        for api in [VALIDATE_API, PIPELINE_API, RESIDENCY_API, FENCE_API, PRESENT_API, REMOVED_API, "ID3D12Device1::CreatePipelineLibrary"] {
            assert!(shown(api), "{api}");
        }
        let e = details(Category::ShaderCompilation, "victory_gpu::pipeline", "x", Fields::default());
        assert!(e.compiler.as_deref().is_some_and(shown) && e.source.as_deref().is_some_and(shown));
        for code in [0x887A_0005, 0x8007_0057, 0xC000_0005] {
            assert!(code_name(code).is_some_and(shown));
        }
        assert!(shown("VIctory summary") && shown("VIctory crash report") && shown("RUST_PANIC"));
    }

    /// Hydrate's `timestamp_utc`: ISO 8601, UTC.
    #[test]
    fn stamps_are_utc() {
        assert_eq!(utc_stamp(Duration::ZERO), "1970-01-01T00:00:00.000000Z");
        // 2024-02-29 23:59:59.25: a leap day
        assert_eq!(utc_stamp(Duration::from_millis(1_709_251_199_250)), "2024-02-29T23:59:59.250000Z");
        // 2026-11-25 12:34:56.000001
        assert_eq!(utc_stamp(Duration::new(1_795_610_096, 1_000)), "2026-11-25T12:34:56.000001Z");
        assert_eq!(utc_stamp(Duration::from_secs(951_782_400)), "2000-02-29T00:00:00.000000Z");
        assert_eq!(utc_stamp(Duration::from_secs(4_107_542_399)), "2100-02-28T23:59:59.000000Z");
    }

    /// A record is one line with Hydrate's three keys, the message escaped
    /// and cut to its limit.
    #[test]
    fn a_record_is_one_line_of_three_keys() {
        let l = line(Category::ShaderCompilation, Severity::Error, "a \"quoted\"\\ line\nwith\ttabs\u{1}");
        assert_eq!(
            l,
            "{\"category\":\"shader_compilation\",\"severity\":\"error\",\"message\":\"a \\\"quoted\\\"\\\\ line\\nwith\\ttabs\\u0001\"}\n"
        );
        assert_eq!(l.matches('\n').count(), 1);
        let long = "é".repeat(5000);
        let l = line(Category::DeviceRemoval, Severity::Warning, &long);
        assert_eq!(l.chars().filter(|&c| c == 'é').count(), MESSAGE_CHARS);
        assert!(l.ends_with("\"}\n"));
        for c in Category::ALL {
            assert!(line(c, Severity::Info, "x").starts_with(&format!("{{\"category\":\"{}\",\"severity\":\"info\"", c.name())));
        }
        // the eleven names, each once
        let names: std::collections::HashSet<_> = Category::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names.len(), 11);
        for (i, c) in Category::ALL.iter().enumerate() {
            assert_eq!(*c as usize, i, "the counts are kept in this order");
        }
    }

    /// The optional fields: each once, with Hydrate's names and types.
    #[test]
    fn a_record_carries_its_fields() {
        let extra = Extra {
            timestamp_utc: Some("2026-11-25T12:34:56.000001Z".into()),
            pid: Some(10716),
            source: Some("VIctory victory_gpu::host".into()),
            source_api: Some("IDXGISwapChain::Present".into()),
            code: Some("DXGI_ERROR_DEVICE_REMOVED".into()),
            hresult: Some(0x887a_0005),
            compiler: Some("c".into()),
            profile: Some("cs_6_6".into()),
            stage: Some("compute".into()),
            diagnostic_id: Some("6b82bc7d22016cb5".into()),
            frame: Some(4211),
            duration_ms: Some(146.7),
        };
        let l = line_with(Category::Performance, Severity::Warning, "m", &extra);
        assert_eq!(
            l,
            "{\"category\":\"performance\",\"severity\":\"warning\",\"message\":\"m\",\"timestamp_utc\":\"2026-11-25T12:34:56.000001Z\",\"pid\":10716,\"source\":\"VIctory victory_gpu::host\",\"source_api\":\"IDXGISwapChain::Present\",\"code\":\"DXGI_ERROR_DEVICE_REMOVED\",\"hresult\":\"0x887A0005\",\"compiler\":\"c\",\"profile\":\"cs_6_6\",\"stage\":\"compute\",\"diagnostic_id\":\"6b82bc7d22016cb5\",\"frame\":4211,\"duration_ms\":146.700}\n"
        );
        check_hydrate(l.trim_end());
        // a long field is cut; a time that is no number is left out
        let extra = Extra { source: Some("s".repeat(400)), duration_ms: Some(f64::NAN), ..Extra::default() };
        let l = line_with(Category::Performance, Severity::Info, "m", &extra);
        assert!(l.contains(&format!("\"source\":\"{}\"", "s".repeat(FIELD_CHARS))) && !l.contains("duration_ms"));
        check_hydrate(l.trim_end());
    }

    /// What Hydrate checks of a record: one flat object, the keys it knows
    /// each once, the three it needs, strings within their limits, `pid` and
    /// `frame` integers, `duration_ms` a number, `hresult` `0x` and eight
    /// hexadecimal digits, `timestamp_utc` ending in `Z`.
    fn check_hydrate(l: &str) {
        const KNOWN: [&str; 19] = [
            "category",
            "severity",
            "message",
            "timestamp_utc",
            "pid",
            "session_id",
            "source",
            "source_api",
            "scope",
            "code",
            "hresult",
            "compiler",
            "profile",
            "stage",
            "diagnostic_id",
            "frame",
            "duration_ms",
            "gpu_duration_ns",
            "supporting_evidence",
        ];
        let b: Vec<char> = l.chars().collect();
        let mut i = 0;
        let string = |i: &mut usize| -> String {
            assert_eq!(b[*i], '"', "a string at {i} of {l}");
            *i += 1;
            let mut out = String::new();
            while b[*i] != '"' {
                if b[*i] == '\\' {
                    *i += 1;
                    match b[*i] {
                        'u' => {
                            let hex: String = b[*i + 1..*i + 5].iter().collect();
                            out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
                            *i += 4;
                        }
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        c @ ('"' | '\\' | '/') => out.push(c),
                        c => panic!("escape \\{c} in {l}"),
                    }
                } else {
                    assert!(b[*i] as u32 >= 0x20, "a raw control character in {l}");
                    out.push(b[*i]);
                }
                *i += 1;
            }
            *i += 1;
            out
        };
        assert_eq!(b[i], '{');
        i += 1;
        let mut seen = std::collections::HashMap::new();
        loop {
            let key = string(&mut i);
            assert!(KNOWN.contains(&key.as_str()), "Hydrate refuses the field {key}: {l}");
            assert_eq!(b[i], ':');
            i += 1;
            let value = if b[i] == '"' {
                let v = string(&mut i);
                let limit = if key == "message" { 1024 } else { 256 };
                assert!(v.chars().count() <= limit, "{key} is longer than {limit}");
                v
            } else {
                let from = i;
                while i < b.len() && !matches!(b[i], ',' | '}') {
                    i += 1;
                }
                let n: String = b[from..i].iter().collect();
                assert!(matches!(key.as_str(), "pid" | "frame" | "duration_ms" | "gpu_duration_ns"), "{key} is a number: {l}");
                if key == "duration_ms" {
                    assert!(n.parse::<f64>().is_ok_and(f64::is_finite), "{n}");
                } else {
                    n.parse::<u64>().unwrap_or_else(|_| panic!("{key} is an integer: {n}"));
                }
                n
            };
            match key.as_str() {
                "hresult" => assert!(value.len() == 10 && value.starts_with("0x") && value[2..].chars().all(|c| c.is_ascii_hexdigit()), "{value}"),
                "timestamp_utc" => assert!(value.ends_with('Z') && value.len() >= 20, "{value}"),
                "category" => assert!(Category::ALL.iter().any(|c| c.name() == value), "{value}"),
                "severity" => assert!(["info", "warning", "error"].contains(&value.as_str()), "{value}"),
                _ => {}
            }
            assert!(seen.insert(key.clone(), value).is_none(), "{key} twice: {l}");
            match b[i] {
                ',' => i += 1,
                '}' => break,
                c => panic!("{c} after a value in {l}"),
            }
        }
        assert_eq!(i + 1, b.len(), "one object a line: {l}");
        for k in ["category", "severity", "message"] {
            assert!(seen.contains_key(k), "{k} is needed: {l}");
        }
    }

    /// Lines that differ in their numbers and hashes only are one kind;
    /// other words, or another category, are another.
    #[test]
    fn numbers_and_hashes_do_not_make_another_kind() {
        let c = Category::ShaderCompilation;
        let a = kind_of(c, "shader translated stage=\"pixel\" hash=7e39ed4b99383eeb user_data=2 samplers=5");
        let b = kind_of(c, "shader translated stage=\"pixel\" hash=ae6cafce2e785964 user_data=14 samplers=0");
        assert_eq!(a, b);
        assert_ne!(a, kind_of(c, "shader translated stage=\"compute\" hash=7e39ed4b99383eeb user_data=2 samplers=5"));
        assert_ne!(a, kind_of(Category::PipelineState, "shader translated stage=\"pixel\" hash=7e39ed4b99383eeb user_data=2 samplers=5"));
        // short words of the letters a to f stay words
        assert_ne!(kind_of(c, "a bad face"), kind_of(c, "a dead cafe"));
        // the frame statistics: a long line whose numbers grow by a digit stays one kind
        let p = Category::Performance;
        let stats = |fps: &str, ms: &str, draws: &str, made: &str| {
            format!(
                "victory_gpu::host: {fps} fps, {ms} ms/frame (0.1 preparing: copies 0.0 ms 0.0 MB, textures 0.0 ms 0.0 MB; 0.0 on the GPU; 0.0 copying back 0.0 MB), {draws} draws/frame, dropped 0 without a host pipeline ({made} being made) and 0 indirect; heap buffers 0.0 passes 0.0 ms; texture slots written so far: 1353 placed"
            )
        };
        assert_eq!(kind_of(p, &stats("30.0", "33.3", "3", "931")), kind_of(p, &stats("5.5", "182.0", "4962", "17")));
        let r = Category::ResourceBindings;
        assert_eq!(
            kind_of(r, "buffer view the host would refuse left null format=39 first=2 count=16 width=4194304"),
            kind_of(r, "buffer view the host would refuse left null format=0 first=18 count=1 width=65536")
        );
    }

    /// The log's lines land in the category of what they speak of.
    #[test]
    fn lines_are_sorted_by_what_they_speak_of() {
        use Category::*;
        use Severity::*;
        let gpu = "victory_gpu::pipeline";
        assert_eq!(classify(1, gpu, "shader does not translate; its pipelines' draws are dropped stage=pixel"), Some((ShaderCompilation, Error)));
        assert_eq!(classify(1, "victory_gpu::pipeline::host", "host pipeline refused error=0x80070057"), Some((PipelineState, Error)));
        assert_eq!(classify(1, "victory_gpu::draw", "a texture picked by index changed after its upload with nothing to tell of it: uploaded again slot=5"), None);
        assert_eq!(classify(2, gpu, "shader translated stage=\"pixel\" hash=0"), Some((ShaderCompilation, Info)));
        assert_eq!(classify(2, "victory_gpu::pipeline::host", "host pipeline shaders=[vertex 1, pixel 2]"), Some((PipelineState, Info)));
        assert_eq!(classify(1, gpu, "a ray-tracing structure builder program without a host pipeline"), Some((PipelineState, Error)));
        assert_eq!(classify(1, "victory_gpu::descriptor::host", "buffer view the host would refuse left null"), Some((ResourceBindings, Warning)));
        assert_eq!(classify(0, "victory_gpu::host", "host GPU failure error=0x887A0005"), Some((DeviceRemoval, Error)));
        assert_eq!(classify(2, "victory_gpu::host", "device removed: DEVICE_HUNG"), Some((DeviceRemoval, Warning)));
        assert_eq!(classify(1, "victory_gpu::draw", "textures picked by index: the card has no room for more; they wait"), Some((ResourceResidency, Warning)));
        assert_eq!(classify(0, "victory_hle::modules::xmem", "something broke"), Some((ApplicationErrors, Error)));
        // the categories Hydrate added
        assert_eq!(classify(1, "victory_gpu::host", "submissions sent ahead not waited for error=timeout"), Some((Synchronization, Warning)));
        assert_eq!(classify(1, "victory_gpu::queue", "a queue still held a submission at a present: its fence was not reached"), Some((Synchronization, Warning)));
        assert_eq!(classify(1, "victory_gpu::list", "a list named twice by a submission that waits for memory values runs once"), Some((Synchronization, Warning)));
        assert_eq!(classify(2, "victory_gpu::host", "5.5 fps, 182.0 ms/frame (110.2 preparing)"), Some((Performance, Info)));
        assert_eq!(
            classify(2, "victory_gpu::host", "the system changed the budget of the card's memory from 11242 MB to 10300 MB (frame 4211)"),
            Some((ResourceResidency, Info))
        );
        assert_eq!(classify(2, "victory_gpu::host", "video memory as the system counts it, at the start: the card's own: budget 11242 MB"), Some((ResourceResidency, Info)));
        assert_eq!(classify(2, "victory_gpu::pipeline", "translated shaders are checked by dxil.dll's validator, version 1.9"), Some((ShaderValidation, Info)));
        // a shader the validator rejects is logged as a warning (the run goes on): the category's error
        assert_eq!(classify(1, "victory_gpu::pipeline", "shader failed DXIL validation (dxil.dll) stage=mesh hash=72da0ba1efaec1e0"), Some((ShaderValidation, Error)));
        assert_eq!(classify(0, "victory_loader::crash", "VIctory: unhandled exception 0xc0000005 at image+0x8f92a"), Some((CrashDiagnostics, Error)));
        assert_eq!(classify(0, "victory", "thread 'main' panicked at crates/x.rs:1:1: index out of bounds"), Some((CrashDiagnostics, Error)));
        // a warning that speaks of none of them, and information of no record: left out
        assert_eq!(classify(1, "victory_hle::modules::net", "HTTP connection blocked"), None);
        assert_eq!(classify(2, "victory_gpu::host", "group buffer made: wave64 compute programs whose LDS leaves no room"), None);
        assert_eq!(classify(2, "victory_gpu::pipeline", "first bind"), None);
    }

    /// End to end: with the variable set, the log's lines of a category
    /// arrive in the file as records with the fields Hydrate shows, a repeat
    /// is held back, a crash report's line is a crash, and the rest of the
    /// log stays out. Every line is one Hydrate takes.
    #[test]
    fn the_logs_lines_arrive_in_the_file() {
        use tracing_subscriber::layer::SubscriberExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("hydrate.jsonl");
        // SAFETY: the only test that reads or sets this variable, set before the file is first asked for.
        unsafe { std::env::set_var("VICTORY_HYDRATE_LOG", &path) };
        let layer = layer().expect("the variable is set");
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            tracing::warn!(target: "victory_gpu::pipeline", stage = "pixel", hash = %"7e39ed4b99383eeb", error = "an image store to a mip level", "shader does not translate; its pipelines' draws are dropped");
            tracing::warn!(target: "victory_gpu::pipeline", stage = "pixel", hash = %"ae6cafce2e785964", error = "an image store to a mip level", "shader does not translate; its pipelines' draws are dropped");
            tracing::warn!(target: "victory_gpu::descriptor::host", format = 39, first = 2, "buffer view the host would refuse left null");
            tracing::info!(target: "victory_gpu::host", "5.5 fps, 182.0 ms/frame");
            tracing::info!(target: "victory_gpu::pipeline::host", "host pipeline shaders=[vertex 1d7b30217b395cd8, pixel 91c9a0113a71a427]");
            tracing::error!(target: "victory_gpu::host", error = "0x887A0005", "host GPU failure");
            tracing::info!(target: "victory_gpu::apis", "api Command queues and fences [CPU/GPU synchronization and stalls]: 6022 waits on the fence in the last 61 s: 146.7 ms a frame our threads stood waiting for the card");
            tracing::info!(target: "victory_gpu::pipeline", stage = "compute", hash = %"6b82bc7d22016cb5", profile = "cs_6_6", user_data = 2, "shader translated");
            tracing::debug!(target: "victory_gpu::pipeline", "shader does not translate (debug lines are not read)");
            tracing::info!(target: "victory_gpu::draw", "textures picked by index: 5 uploaded (information of other targets is not read)");
        });
        record_from_crash("VIctory: unhandled exception 0xc0000005 at image+0x8f92a (read 0xffffffffffffffff: outside the address space)");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 9, "{text}");
        for l in &lines {
            check_hydrate(l);
            assert!(l.contains(&format!("\"pid\":{}", std::process::id())) && l.contains("\"timestamp_utc\":\""), "{l}");
        }
        assert!(lines[0].contains("\"severity\":\"info\"") && lines[0].contains("diagnostics connected"));
        let starts = |i: usize, head: &str| assert!(lines[i].starts_with(head), "{}", lines[i]);
        starts(
            1,
            "{\"category\":\"shader_compilation\",\"severity\":\"error\",\"message\":\"victory_gpu::pipeline: shader does not translate; its pipelines' draws are dropped stage=pixel hash=7e39ed4b99383eeb error=an image store to a mip level\",\"timestamp_utc\":\"",
        );
        for field in ["\"source\":\"VIctory victory_gpu::pipeline\"", "\"stage\":\"pixel\"", "\"profile\":\"ps_6_6\"", "\"diagnostic_id\":\"7e39ed4b99383eeb\"", "\"compiler\":\"VIctory "] {
            assert!(lines[1].contains(field), "{field}: {}", lines[1]);
        }
        starts(2, "{\"category\":\"resource_bindings\",\"severity\":\"warning\"");
        starts(3, "{\"category\":\"performance\",\"severity\":\"info\"");
        assert!(lines[3].contains("\"duration_ms\":182.000") && lines[3].contains("\"source_api\":\"IDXGISwapChain::Present\""), "{}", lines[3]);
        starts(4, "{\"category\":\"pipeline_state\",\"severity\":\"info\"");
        starts(5, "{\"category\":\"device_removal\",\"severity\":\"error\"");
        for field in ["\"hresult\":\"0x887A0005\"", "\"code\":\"DXGI_ERROR_DEVICE_REMOVED\"", "\"source_api\":\"ID3D12Device::GetDeviceRemovedReason\""] {
            assert!(lines[5].contains(field), "{field}: {}", lines[5]);
        }
        starts(6, "{\"category\":\"synchronization\",\"severity\":\"info\"");
        assert!(lines[6].contains("\"source_api\":\"ID3D12Fence::SetEventOnCompletion\"") && lines[6].contains("\"duration_ms\":146.700"), "{}", lines[6]);
        starts(7, "{\"category\":\"shader_compilation\",\"severity\":\"info\"");
        assert!(lines[7].contains("\"profile\":\"cs_6_6\"") && lines[7].contains("\"diagnostic_id\":\"6b82bc7d22016cb5\""), "{}", lines[7]);
        starts(8, "{\"category\":\"crash_diagnostics\",\"severity\":\"error\",\"message\":\"VIctory: unhandled exception");
        assert!(lines[8].contains("\"code\":\"EXCEPTION_ACCESS_VIOLATION\"") && lines[8].contains("\"source\":\"VIctory crash report\""), "{}", lines[8]);
        // the frame statistics are the summaries' too
        assert_eq!(*LAST_FRAME.lock().unwrap(), Some((5.5, 182.0)));
    }

    /// The counts so far: the shaders, the pipelines and crashes always, the
    /// other categories once they have had lines; each a record of
    /// information, the performance one with the last frame time.
    #[test]
    fn the_counts_so_far_are_records_too() {
        let mut counts = [[0u64; 3]; 11];
        let lines = summaries(&counts, 15, None, &Extra::default());
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[0],
            "{\"category\":\"shader_compilation\",\"severity\":\"info\",\"message\":\"VIctory summary, 15 s into the run: 0 shaders translated, 0 did not translate, 0 other warnings\"}\n"
        );
        assert!(lines[1].starts_with("{\"category\":\"pipeline_state\",\"severity\":\"info\""));
        assert!(lines[2].starts_with("{\"category\":\"crash_diagnostics\",\"severity\":\"info\"") && lines[2].contains("no unhandled exception and no panic so far"));
        counts[Category::ShaderCompilation as usize] = [3312, 1, 2];
        counts[Category::DeviceRemoval as usize] = [0, 0, 1];
        counts[Category::Performance as usize] = [59, 0, 0];
        counts[Category::CrashDiagnostics as usize] = [0, 0, 1];
        let base = Extra { pid: Some(7), source: Some("VIctory summary".into()), ..Extra::default() };
        let lines = summaries(&counts, 300, Some((5.5, 182.0)), &base);
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("300 s into the run: 3312 shaders translated, 2 did not translate, 1 other warnings"));
        assert!(lines[2].starts_with("{\"category\":\"device_removal\",\"severity\":\"info\"") && lines[2].contains("1 errors, 0 warnings"));
        assert!(lines[3].starts_with("{\"category\":\"performance\",\"severity\":\"info\"") && lines[3].contains("frame time 182.0 ms (5.5 frames a second)"));
        assert!(lines[3].contains("\"duration_ms\":182.000") && lines[3].contains("\"source_api\":\"IDXGISwapChain::Present\""));
        assert!(lines[4].starts_with("{\"category\":\"crash_diagnostics\",\"severity\":\"info\"") && lines[4].contains("1 unhandled exceptions or panics"));
        for l in &lines {
            assert!(l.contains("\"pid\":7") && l.contains("\"source\":\"VIctory summary\""));
            check_hydrate(l.trim_end());
        }
        // four summaries a minute beside a minute's lines: under a capture's 200 records
        const { assert!(6 * PER_WINDOW + 4 * 11 < 200) };
    }

    /// A kind is written once in [`AGAIN`], the next one with the count of
    /// those held back; a window takes [`PER_WINDOW`] lines, errors twice
    /// as many; a minute's lines stay under a capture's 200.
    #[test]
    fn repeats_are_counted_and_a_window_is_bounded() {
        let mut l = Limits::default();
        let s = Duration::from_secs;
        assert_eq!(l.decide(1, Severity::Warning, s(0)), Verdict::Write(0));
        assert_eq!(l.decide(1, Severity::Warning, s(1)), Verdict::Hold);
        assert_eq!(l.decide(1, Severity::Warning, s(59)), Verdict::Hold);
        assert_eq!(l.decide(1, Severity::Warning, s(60)), Verdict::Write(2));
        assert_eq!(l.decide(1, Severity::Warning, s(120)), Verdict::Write(0));
        // a window's room
        let mut l = Limits::default();
        let written = (0..500u64).filter(|&k| l.decide(100 + k, Severity::Warning, s(3)) != Verdict::Hold).count();
        assert_eq!(written as u32, PER_WINDOW);
        let errors = (0..500u64).filter(|&k| l.decide(1000 + k, Severity::Error, s(4)) != Verdict::Hold).count();
        assert_eq!(errors as u32, PER_WINDOW);
        // a kind held for lack of room is written when there is room again
        assert_eq!(l.decide(100 + 400, Severity::Warning, s(11)), Verdict::Write(0));
        // a minute of new kinds every second of both severities: under 200 with errors rare
        let mut l = Limits::default();
        let mut n = 0;
        for t in 0..60u64 {
            for k in 0..50u64 {
                if l.decide(t * 1000 + k, Severity::Warning, s(t)) != Verdict::Hold {
                    n += 1;
                }
            }
        }
        assert_eq!(n, 6 * PER_WINDOW);
        assert!(n < 200);
    }
}
