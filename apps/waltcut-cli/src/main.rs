//! WaltCut headless CLI: every engine command is one shell call away, for scripts and AI agents.
//!
//! Run `waltcut-cli help` for the full reference (also in docs/agents.md).

#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable))]

/// `println!` that doesn't panic when stdout is closed (`waltcut-cli commands | head`) with
/// "failed printing to stdout: Broken pipe (os error 32)". Once stdout has failed, later output is
/// dropped and the work goes on: see `stdout_failed`.
macro_rules! outln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if !$crate::stdout_gone() {
            if let Err(e) = writeln!(std::io::stdout(), $($arg)*) {
                $crate::stdout_failed(e);
            }
        }
    }};
}

/// `print!` counterpart of `outln!`.
macro_rules! out {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        if !$crate::stdout_gone() {
            if let Err(e) = write!(std::io::stdout(), $($arg)*) {
                $crate::stdout_failed(e);
            }
        }
    }};
}

mod args;
mod probe;

use args::Args;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use waltcut_automation::BridgeClient;
use waltcut_engine::Session;

/// Set once a write to stdout has failed; later output is dropped.
static STDOUT_GONE: AtomicBool = AtomicBool::new(false);
/// Set when stdout failed for a reason other than a closed reader: the exit status becomes 1.
static STDOUT_ERROR: AtomicBool = AtomicBool::new(false);

fn stdout_gone() -> bool {
    STDOUT_GONE.load(Ordering::Relaxed)
}

/// stdout went away. The program doesn't stop there: the rest of `exec`/`run`/`import` (later
/// script lines, `--save`, `--save-as`) still runs and the exit status still tells how it went, so
/// `exec … --save | head -1` keeps the edit. A reader that stopped early (a closed pipe) is not an
/// error, as in ripgrep; any other write error is reported and makes the exit status 1.
fn stdout_failed(e: std::io::Error) {
    if STDOUT_GONE.swap(true, Ordering::Relaxed) || e.kind() == std::io::ErrorKind::BrokenPipe {
        return;
    }
    diag(format_args!("waltcut-cli: can't write to stdout: {e}"));
    STDOUT_ERROR.store(true, Ordering::Relaxed);
}

const HELP: &str = "\
waltcut-cli: drive WaltCut from the shell (headless engine, or the live app with --bridge)

USAGE
  waltcut-cli [OPTIONS] <SUBCOMMAND> [ARGS]

SUBCOMMANDS
  exec <id> [key=value ...]     run one command; prints its result as JSON
  exec <id> '<json params>'     same, params as one JSON object
  run <script.jsonl | ->        run commands, one {\"id\",\"params\"} per line (# comments ok)
  commands [filter] [--json]    list commands (id, label, shortcut, params)
  describe <id>                 one command as JSON (menu, shortcut, params, enabled now)
  inspect [project|sequence]    project tree or active sequence as JSON (default: both)
  import <file>...              import media into the project
  export <out> [--format h264|prores|png] [--range entire|inOut|workArea]
         [--start seconds --end seconds] [--overwrite] [--allow-offline]
                                export one sequence and wait for its job: H.264 + AAC MP4,
                                ProRes 4444 MOV with alpha and stereo PCM, or PNG sequence with
                                alpha. Format is guessed from .mp4/.mov/.png. Output size and rate
                                follow the sequence. PNG sequences need unused filenames.
                                Existing movies require explicit --overwrite; a failed/cancelled
                                export leaves them untouched. Offline media renders placeholders,
                                warns on stderr and exits 1 unless --allow-offline accepts it.
  render --seconds S --out f.png|f.jpg [--overwrite] [--allow-offline]
                                save one full-resolution Program frame via file.exportFrame;
                                PNG keeps alpha, JPEG flattens over black (offline media: as above)
  probe <media> [--image-sequence]
                                media info as JSON, with MPEG transport/program stream and BWF details; with
                                --image-sequence <media> is the first numbered still of a sequence
  bench-decode <media> [--frames N]
  mcp                           MCP server on stdio (headless, or --bridge to the live app)
  help                          this text
  --version                     print the version

OPTIONS
  --project <p.waltcut>          open this project first (headless)
  --demo                        open the built-in demo project (headless)
  --save                        save the project back to --project when done
  --save-as <p.waltcut>          save the project to this path when done
  --bridge <127.0.0.1:PORT>     send commands to the running app (`waltcut --control PORT`)
  --keep-going                  `run`: report failing lines and continue
  --compact                     one-line JSON output

VALUES
  key=value parses value as JSON when it can (3.5, true, [1,2], {\"a\":1}), else as a string:
  `exec timeline.razor seconds=3.5`, `exec file.newBin name=Selects`.
  Times: 254016000000 ticks per second; most commands also take seconds=, frame= or timecode=.

EXIT STATUS
  0 success · 1 a command failed, or export/render drew offline media without --allow-offline
  · 2 usage error
";

/// Writes one diagnostic line to `w`, dropping any write error. `eprintln!` panics (exit 101) when stderr's
/// reader is gone, which would turn a usage error, a failure or a finished export into a crash.
fn write_diag(w: &mut dyn std::io::Write, msg: std::fmt::Arguments) {
    let _ = writeln!(w, "{msg}");
}

fn diag(msg: std::fmt::Arguments) {
    write_diag(&mut std::io::stderr(), msg);
}

fn usage(msg: impl std::fmt::Display) -> ! {
    diag(format_args!("waltcut-cli: {msg}\nRun `waltcut-cli help` for usage."));
    std::process::exit(2)
}

fn fail(msg: impl std::fmt::Display) -> ! {
    diag(format_args!("waltcut-cli: {msg}"));
    std::process::exit(1)
}

/// Where commands go: an in-process session, or the desktop app's control channel.
enum Backend {
    Local(Box<Session>),
    Bridge(BridgeClient),
}

impl Backend {
    fn open(a: &Args) -> Self {
        if a.opt("--project").is_some() && a.flag("--demo") {
            usage("choose --project or --demo, not both");
        }
        if let Some(addr) = a.opt("--bridge") {
            if a.opt("--project").is_some() || a.flag("--demo") {
                usage("--bridge drives the running app; open projects there (exec file.open path=…)");
            }
            return Backend::Bridge(BridgeClient::new(addr).unwrap_or_else(|e| usage(e)));
        }
        let mut s = Session::default();
        if let Some(p) = a.opt("--project") {
            if let Err(e) = s.execute("file.open", json!({"path": p})) {
                fail(e);
            }
        } else if a.flag("--demo") {
            let _ = s.execute("file.openDemoProject", json!({}));
        }
        Backend::Local(Box::new(s))
    }

    async fn exec(&mut self, id: &str, params: Value) -> Result<Value, String> {
        match self {
            Backend::Local(s) => s.execute(id, params).map_err(|e| e.to_string()),
            // a blocking export runs as an app job polled by the client (#91, #92)
            Backend::Bridge(b) => b.execute(id, params).await.map_err(|e| e.to_string()),
        }
    }

    /// Save to the --project path (`--save`) or to `--save-as`. Bridge mode saves in the app.
    async fn finish(&mut self, a: &Args) {
        let r = if let Some(p) = a.opt("--save-as") {
            Some(self.exec("file.saveAs", json!({"path": p})).await)
        } else if a.flag("--save") {
            match a.opt("--project") {
                Some(p) => Some(self.exec("file.save", json!({"path": p})).await),
                None => Some(self.exec("file.save", json!({})).await),
            }
        } else {
            None
        };
        if let Some(Err(e)) = r {
            fail(format!("saving: {e}"));
        }
    }
}

fn print(a: &Args, v: &Value) {
    let s = if a.flag("--compact") { serde_json::to_string(v) } else { serde_json::to_string_pretty(v) };
    outln!("{}", s.unwrap_or_default());
}

/// Export format from the output file's extension.
fn format_for(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "mp4" => "h264",
        "mov" => "prores",
        "png" => "png",
        _ => return None,
    })
}

/// OS hardware video decoders (VideoToolbox on macOS) in front of our
/// own, as in the desktop app. Also what `mcp` and the headless commands decode with.
/// The GPU export frame renderer (waltcut-gpu's off-screen compositor behind waltcut-export's
/// frame-renderer hook): exports with GPU rendering Auto composite on the GPU and fall back to the
/// CPU reference renderer wherever it cannot.
struct GpuFrameRenderer(waltcut_gpu::ExportRenderer);

impl waltcut_export::FrameRenderer for GpuFrameRenderer {
    fn render(
        &mut self,
        project: &waltcut_project::Project,
        seq: waltcut_project::ItemId,
        t: waltcut_time::Tick,
        opts: waltcut_render::RenderOptions,
        sources: &dyn waltcut_render::SourceProvider,
    ) -> Option<waltcut_render::Image> {
        self.0.render(project, seq, t, opts, sources)
    }
}

fn register_gpu_frame_renderer() {
    waltcut_export::register_frame_renderer(|| {
        waltcut_gpu::ExportRenderer::new().map(|r| Box::new(GpuFrameRenderer(r)) as Box<dyn waltcut_export::FrameRenderer>)
    });
}

fn register_hardware_decoders() -> waltcut_platform::Availability {
    waltcut_platform::register()
}

/// `path` made absolute against the working directory, as the project stores it. Unlike
/// `canonicalize`, `std::path::absolute` keeps symlinks and adds no Windows verbatim `\\?\`
/// prefix (#850). A path it cannot resolve is kept as given.
fn absolute(path: &str) -> String {
    std::path::absolute(path).ok().and_then(|p| p.into_os_string().into_string().ok()).unwrap_or_else(|| path.to_string())
}

#[tokio::main]
async fn main() {
    cli().await;
    if STDOUT_ERROR.load(Ordering::Relaxed) {
        std::process::exit(1);
    }
}

async fn cli() {
    // `std::env::args` panics on an argument that is not Unicode (a Linux file name in Latin-1)
    let a = Args::parse(std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()));
    // `--help` anywhere (`waltcut-cli --help`, `waltcut-cli export --help`) prints the reference:
    // the parser takes it as a flag option, so it never reaches the subcommand match.
    if a.flag("--help") {
        out!("{HELP}");
        return;
    }
    // `--version` anywhere and `-V` as the subcommand, also after global options (`--demo --version`, #710).
    if a.wants_version() {
        outln!("waltcut-cli {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    register_hardware_decoders();
    register_gpu_frame_renderer();
    let Some(cmd) = a.pos(0) else { usage("missing subcommand") };
    match cmd {
        "help" | "-h" => out!("{HELP}"),
        "version" => outln!("waltcut-cli {}", env!("CARGO_PKG_VERSION")),
        "probe" => {
            let path = a.pos(1).unwrap_or_else(|| usage("probe <media> [--image-sequence]"));
            match probe::probe(path, a.flag("--image-sequence")) {
                Ok(v) => print(&a, &v),
                Err(e) => fail(e),
            }
        }
        "bench-decode" => {
            let path = a.pos(1).unwrap_or_else(|| usage("bench-decode <media>"));
            let n: i64 = a.opt("--frames").and_then(|v| v.parse().ok()).unwrap_or(120);
            let src = waltcut_codecs::open_bytes(path, std::fs::read(path).unwrap_or_else(|e| fail(e)).into()).unwrap_or_else(|e| fail(e));
            let rate = src.info().frame_rate();
            let t0 = std::time::Instant::now();
            for f in 0..n {
                src.video_frame(waltcut_media::FrameRequest::full(rate.tick_of(f))).unwrap_or_else(|e| fail(e));
            }
            let dt = t0.elapsed().as_secs_f64();
            outln!("{n} frames in {dt:.2}s → {:.1} fps (sequential, via Mp4Source)", n as f64 / dt);
            let t1 = std::time::Instant::now();
            let f = src.video_frame(waltcut_media::FrameRequest::full(rate.tick_of(n / 2))).unwrap_or_else(|e| fail(e));
            let (w, h, _) = f.to_linear_f32_decimated(2);
            outln!("½-res linear conversion {w}x{h}: {:.1} ms", t1.elapsed().as_secs_f64() * 1000.0);
        }
        "commands" | "describe" => {
            let mut b = Backend::open(&a);
            let list = b.exec("command.list", json!({})).await.unwrap_or_else(|e| fail(e));
            let list = list.as_array().cloned().unwrap_or_default();
            if cmd == "describe" {
                let id = a.pos(1).unwrap_or_else(|| usage("describe <id>"));
                match list.iter().find(|c| c["id"] == id) {
                    Some(c) => print(&a, c),
                    None => fail(format!("unknown command `{id}` (try `waltcut-cli commands {id}`)")),
                }
                return;
            }
            let f = a.pos(1).map(str::to_ascii_lowercase).unwrap_or_default();
            let hit = |c: &Value| {
                let id = c["id"].as_str().unwrap_or("").to_ascii_lowercase();
                let label = c["label"].as_str().unwrap_or("").to_ascii_lowercase();
                f.is_empty() || id.contains(&f) || label.contains(&f)
            };
            let list: Vec<Value> = list.into_iter().filter(hit).collect();
            if a.flag("--json") {
                print(&a, &Value::Array(list));
            } else {
                for c in &list {
                    let s = |k: &str| c[k].as_str().unwrap_or("").to_string();
                    outln!("{:<32} {:<34} {:<14} {}", s("id"), s("label"), s("shortcut"), s("params"));
                }
            }
        }
        "exec" => {
            let id = a.pos(1).unwrap_or_else(|| usage("exec <id> [key=value ...]"));
            if matches!(id, "file.exportMedia" | "file.exportFrame" | "export.quick") {
                a.check_options(&["--project", "--bridge", "--save-as"], &["--demo", "--compact", "--save"]).unwrap_or_else(|e| usage(e));
            }
            let params = a.params_from(2).unwrap_or_else(|e| usage(e));
            let mut b = Backend::open(&a);
            match b.exec(id, params).await {
                Ok(v) => print(&a, &v),
                Err(e) => fail(format!("{id}: {e}")),
            }
            b.finish(&a).await;
        }
        "run" => {
            a.check_options(&["--project", "--bridge", "--save-as"], &["--demo", "--compact", "--save", "--keep-going"]).unwrap_or_else(|e| usage(e));
            let script = a.pos(1).unwrap_or_else(|| usage("run <script.jsonl | ->"));
            let text = if script == "-" {
                std::io::read_to_string(std::io::stdin()).unwrap_or_else(|e| fail(e))
            } else {
                std::fs::read_to_string(script).unwrap_or_else(|e| fail(format!("{script}: {e}")))
            };
            let mut b = Backend::open(&a);
            let mut failed = 0;
            for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#')) {
                let v: Value = serde_json::from_str(line).unwrap_or_else(|e| usage(format!("line {}: {e}", n + 1)));
                let id = v["id"].as_str().unwrap_or_else(|| usage(format!("line {}: missing \"id\"", n + 1)));
                match b.exec(id, v.get("params").cloned().unwrap_or(json!({}))).await {
                    Ok(r) => outln!("{}", json!({"line": n + 1, "id": id, "ok": true, "result": r})),
                    Err(e) => {
                        outln!("{}", json!({"line": n + 1, "id": id, "ok": false, "error": e}));
                        failed += 1;
                        if !a.flag("--keep-going") {
                            std::process::exit(1);
                        }
                    }
                }
            }
            b.finish(&a).await;
            if failed > 0 {
                std::process::exit(1);
            }
        }
        "inspect" => {
            let mut b = Backend::open(&a);
            let what = a.pos(1).unwrap_or("all");
            let mut get = async |id: &str| b.exec(id, json!({})).await.unwrap_or_else(|e| fail(e));
            let v = match what {
                "project" => get("project.inspect").await,
                "sequence" => get("sequence.inspect").await,
                "all" => json!({"project": get("project.inspect").await, "sequence": get("sequence.inspect").await}),
                other => usage(format!("inspect: unknown `{other}` (project | sequence)")),
            };
            print(&a, &v);
        }
        "import" => {
            let paths: Vec<String> = a.positionals.get(1..).unwrap_or_default().iter().map(|p| absolute(p)).collect();
            if paths.is_empty() {
                usage("import <file>...");
            }
            let mut b = Backend::open(&a);
            match b.exec("file.import", json!({"paths": paths})).await {
                Ok(v) => print(&a, &v),
                Err(e) => fail(format!("import: {e}")),
            }
            b.finish(&a).await;
        }
        "export" => {
            a.check_options(&["--project", "--bridge", "--format", "--range", "--start", "--end"], &["--demo", "--compact", "--overwrite", "--allow-offline"])
                .unwrap_or_else(|e| usage(e));
            if a.positionals.len() != 2 {
                usage("export <out> [--format h264|prores|png]");
            }
            let out = a.pos(1).unwrap_or_else(|| usage("export needs a destination"));
            let format = a
                .opt("--format")
                .or_else(|| format_for(out))
                .unwrap_or_else(|| usage("export supports .mp4, .mov or .png; give --format for an extensionless path"));
            let mut p = json!({"path": out, "format": format, "wait": true, "overwrite": a.flag("--overwrite")});
            if let Some(range) = a.opt("--range") {
                p["range"] = json!(range);
            }
            set_custom_bounds(&mut p, a.opt("--start"), a.opt("--end")).unwrap_or_else(|e| usage(e));
            let mut b = Backend::open(&a);
            let t0 = std::time::Instant::now();
            let r = b.exec("file.exportMedia", p).await;
            match r {
                Ok(v) => {
                    print(&a, &v);
                    let written = written_paths(&v, out).join(", ");
                    check_offline(&a, &offline_media(&v), &format!("export: {written}"));
                    diag(format_args!("exported {written} in {:.1}s", t0.elapsed().as_secs_f64()));
                }
                Err(e) => fail(format!("export: {e}")),
            }
        }
        "render" => {
            a.check_options(&["--project", "--bridge", "--seconds", "--out"], &["--demo", "--compact", "--overwrite", "--allow-offline"])
                .unwrap_or_else(|e| usage(e));
            if a.positionals.len() != 1 {
                usage("render --seconds S --out frame.png|frame.jpg");
            }
            let secs: f64 = finite_opt(&a, "--seconds").unwrap_or(0.0);
            let time = render_time(secs).unwrap_or_else(|e| usage(e));
            let out = a.opt("--out").unwrap_or("frame.png");
            let extension = std::path::Path::new(out).extension().and_then(|s| s.to_str()).unwrap_or_default().to_ascii_lowercase();
            let format = match extension.as_str() {
                "png" => "png",
                "jpg" | "jpeg" => "jpeg",
                _ => usage("render destination must end in .png, .jpg or .jpeg"),
            };
            let mut backend = Backend::open(&a);
            let t0 = std::time::Instant::now();
            let result = backend
                .exec("file.exportFrame", json!({"path":out,"format":format,"time":time.0,"overwrite":a.flag("--overwrite")}))
                .await
                .unwrap_or_else(|e| fail(format!("render: {e}")));
            print(&a, &result);
            check_offline(&a, &offline_media(&result), &format!("render: {out}"));
            diag(format_args!("rendered {}x{} in {:.1} ms → {out}", result["width"], result["height"], t0.elapsed().as_secs_f64() * 1000.0));
        }
        "mcp" => {
            if let Some(opt) = a.unknown_opt(&["--bridge", "--project", "--demo"]) {
                usage(format!("unknown mcp option `{opt}`"));
            }
            let server = match a.opt("--bridge") {
                Some(addr) => waltcut_automation::WaltCutMcp::bridge(addr).unwrap_or_else(|e| usage(e)),
                None => match Backend::open(&a) {
                    Backend::Local(s) => waltcut_automation::WaltCutMcp::headless(*s),
                    Backend::Bridge(_) => usage("mcp: use --bridge ADDR to drive a running app"),
                },
            };
            if let Err(e) = server.serve_stdio().await {
                fail(format!("mcp: {e}"));
            }
        }
        other => usage(format!("unknown subcommand `{other}`")),
    }
}

/// The destination returned by the direct export command (which may add an extension).
fn written_paths(result: &Value, requested: &str) -> Vec<String> {
    vec![result.get("path").and_then(Value::as_str).unwrap_or(requested).to_string()]
}

fn offline_media(result: &Value) -> Vec<Value> {
    result.get("offlineMedia").and_then(Value::as_array).cloned().unwrap_or_default()
}

/// One stderr line per offline item that rendered as placeholder frames (#1054).
fn offline_warning(offline: &[Value]) -> String {
    let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut msg = format!("waltcut-cli: warning: {} offline media item(s) rendered as placeholder frames:", offline.len());
    for o in offline {
        let id = o.get("item").and_then(Value::as_u64).map(|i| i.to_string()).unwrap_or_default();
        msg.push_str(&format!("\n  item {id} {} ({}): {}", s(o, "name"), s(o, "status"), s(o, "path")));
    }
    msg
}

/// Warn about offline media on stderr; without `--allow-offline` that fails the run (exit 1), after
/// the output (`what`) was written, so scripts don't take placeholder frames for the real picture.
fn check_offline(a: &Args, offline: &[Value], what: &str) {
    if offline.is_empty() {
        return;
    }
    diag(format_args!("{}", offline_warning(offline)));
    if !a.flag("--allow-offline") {
        fail(format!("{what} has placeholder frames for offline media; relink it (exec media.relink …) or pass --allow-offline"));
    }
}

/// A numeric option, or a usage error when it is not a finite number (so a typo never silently
/// falls back to a default).
fn finite_opt<T: std::str::FromStr + Copy + Into<f64>>(a: &Args, name: &str) -> Option<T> {
    let v = a.opt(name)?;
    match v.parse::<T>() {
        Ok(n) if n.into().is_finite() => Some(n),
        _ => usage(format!("{name}: expected a finite number, got `{v}`")),
    }
}

/// Validate still-frame seconds before converting to exact timeline time.
fn render_time(seconds: f64) -> Result<waltcut_time::Tick, String> {
    if !seconds.is_finite() || seconds < 0.0 || seconds > waltcut_time::Tick::MAX.seconds() {
        return Err("--seconds must be finite, non-negative and within supported time bounds".into());
    }
    let tick = waltcut_time::Tick::from_seconds_f64(seconds);
    if tick > waltcut_time::Tick::MAX {
        return Err("--seconds exceeds supported time bounds after rounding".into());
    }
    Ok(tick)
}

fn set_custom_bounds(p: &mut Value, start: Option<&str>, end: Option<&str>) -> Result<(), String> {
    let (start, end) = match (start, end) {
        (None, None) => return Ok(()),
        (Some(start), Some(end)) => (start, end),
        _ => return Err("provide both --start and --end for a custom export range".into()),
    };
    if p.get("range").is_some_and(|value| value != "custom") {
        return Err("--start/--end cannot be combined with a non-custom --range".into());
    }
    let parse = |text: &str, name: &str| {
        text.parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value >= 0.0 && *value <= waltcut_time::Tick::MAX.seconds())
            .ok_or_else(|| format!("{name} must be finite, non-negative seconds within supported bounds"))
    };
    let (start, end) = (parse(start, "--start")?, parse(end, "--end")?);
    if end <= start {
        return Err("--end must be after --start".into());
    }
    p["range"] = json!("custom");
    p["startSeconds"] = json!(start);
    p["endSeconds"] = json!(end);
    Ok(())
}

#[cfg(test)]
mod format_tests {
    #[test]
    fn render_time_rejects_hostile_seconds_before_tick_conversion() {
        for seconds in [-1.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert!(super::render_time(seconds).is_err());
        }
        assert_eq!(super::render_time(0.25).unwrap(), waltcut_time::FrameRate::FPS_24.tick_of(6));
    }

    /// The CLI (and MCP / headless runs, which share its entry point) registers the hardware
    /// decoders at start-up.
    #[test]
    fn startup_registers_the_hardware_decoders() {
        let hardware = super::register_hardware_decoders();
        assert!(waltcut_platform::registered(), "{hardware:?}");
    }

    #[test]
    fn a_failing_diagnostic_stream_does_not_panic() {
        struct Closed;
        impl std::io::Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        super::write_diag(&mut Closed, format_args!("exported {}", "a.mov"));
        let mut ok = Vec::new();
        super::write_diag(&mut ok, format_args!("hi"));
        assert_eq!(ok, b"hi\n");
    }

    #[test]
    fn export_message_names_the_written_files() {
        use serde_json::json;
        let direct = json!({"job": 1, "path": "out-prores.mov", "result": {"path": "out-prores.mov"}});
        assert_eq!(super::written_paths(&direct, "out-prores"), ["out-prores.mov"]);
        assert_eq!(super::written_paths(&json!({}), "x.mov"), ["x.mov"]);
    }

    #[test]
    fn offline_media_from_direct_results() {
        use serde_json::json;
        let a = json!({"item": 8, "name": "a.mp4", "path": "/m/a.mp4", "status": "missing", "error": ""});
        let b = json!({"item": 9, "name": "b.mov", "path": "/m/b.mov", "status": "offline", "error": ""});
        assert_eq!(super::offline_media(&json!({"path": "x.mp4", "offlineMedia": [a]})), std::slice::from_ref(&a));
        assert!(super::offline_media(&json!({"path": "x.mp4"})).is_empty());
        let w = super::offline_warning(&[a, b]);
        assert!(w.contains("2 offline media item(s)") && w.contains("item 8 a.mp4 (missing): /m/a.mp4") && w.contains("item 9 b.mov (offline)"), "{w}");
    }

    #[test]
    fn export_custom_bounds_are_explicit_and_never_ignore_a_conflicting_range() {
        use serde_json::json;
        let mut p = json!({});
        assert!(super::set_custom_bounds(&mut p, Some("0.25"), None).is_err());
        assert!(super::set_custom_bounds(&mut p, None, Some("0.25")).is_err());
        super::set_custom_bounds(&mut p, Some("0"), Some("0.25")).unwrap();
        assert_eq!(p["range"], "custom");
        assert_eq!(p["endSeconds"], 0.25);
        assert_eq!(p["startSeconds"], 0.0);
        for (start, end) in [("NaN", "1"), ("0", "inf"), ("-1", "1"), ("1", "0"), ("0", "0"), ("0", "1e300")] {
            assert!(super::set_custom_bounds(&mut json!({}), Some(start), Some(end)).is_err());
        }
        assert!(super::set_custom_bounds(&mut json!({"range":"entire"}), Some("0"), Some("1")).is_err());
        let mut p = json!({});
        super::set_custom_bounds(&mut p, None, None).unwrap();
        assert!(p.get("range").is_none());
    }

    /// `import` used `canonicalize`, which resolves symlinks and on Windows stores `\\?\C:\…`
    /// verbatim paths in the project (#850).
    #[test]
    fn imported_paths_are_absolute_without_resolving_links() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(super::absolute("clip.mov"), cwd.join("clip.mov").to_string_lossy());
        assert!(!super::absolute("clip.mov").starts_with(r"\\?\"));
        assert_eq!(super::absolute(""), "");
        {
            let dir = std::env::temp_dir().join(format!("waltcut-cli-abs-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("real.mov"), b"x").unwrap();
            let link = dir.join("link.mov");
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(dir.join("real.mov"), &link).unwrap();
            assert_eq!(super::absolute(&link.to_string_lossy()), link.to_string_lossy());
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn export_format_from_extension() {
        assert_eq!(super::format_for("a.mov"), Some("prores"));
        assert_eq!(super::format_for("noext"), None);
    }
}
