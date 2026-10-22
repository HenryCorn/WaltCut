//! End-to-end tests of the CLI binary (headless backend).

use std::io::Write;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_filmcraft-cli")).args(args).output().expect("spawn filmcraft-cli")
}

fn json_out(o: &Output) -> Value {
    assert!(o.status.success(), "failed: {}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).expect("JSON on stdout")
}

#[test]
fn help_and_usage_errors() {
    let h = cli(&["help"]);
    assert!(h.status.success());
    assert!(String::from_utf8_lossy(&h.stdout).contains("exec <id>"));
    assert_eq!(cli(&[]).status.code(), Some(2));
    assert_eq!(cli(&["frobnicate"]).status.code(), Some(2));
    assert_eq!(cli(&["exec", "file.newBin", "notkv"]).status.code(), Some(2));
    // Invalid --bridge is a usage error (exit 2) for both exec and mcp (#921).
    assert_eq!(cli(&["--bridge", "0.0.0.0:9876", "exec", "inspect.project"]).status.code(), Some(2));
    assert_eq!(cli(&["mcp", "--bridge", "0.0.0.0:9876"]).status.code(), Some(2));
}

#[test]
fn version_after_global_options() {
    let want = format!("filmcraft-cli {}\n", env!("CARGO_PKG_VERSION"));
    for args in [&["--version"][..], &["-V"], &["version"], &["--demo", "--version"], &["--compact", "-V"], &["--demo", "version"]] {
        let o = cli(args);
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        assert_eq!(String::from_utf8_lossy(&o.stdout), want, "{args:?}");
    }
}

#[test]
fn long_help_matches_help_command() {
    let reference = cli(&["help"]);
    assert!(reference.status.success());
    let long_help = cli(&["--help"]);
    assert!(long_help.status.success(), "--help failed: {}", String::from_utf8_lossy(&long_help.stderr));
    assert_eq!(long_help.stdout, reference.stdout);
    assert!(long_help.stderr.is_empty());
    let short_help = cli(&["-h"]);
    assert!(short_help.status.success());
    assert_eq!(short_help.stdout, reference.stdout);
    let sub_help = cli(&["export", "--help"]);
    assert!(sub_help.status.success(), "export --help failed: {}", String::from_utf8_lossy(&sub_help.stderr));
    assert_eq!(sub_help.stdout, reference.stdout);
}

#[test]
fn probe_reports_mpeg_transport_and_program_streams() {
    let Some(ffmpeg) = ["/opt/homebrew/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/usr/bin/ffmpeg"].into_iter().find(|p| std::path::Path::new(p).exists()) else {
        eprintln!("SKIPPED (probe_reports_mpeg_transport_and_program_streams): ffmpeg not found");
        return;
    };
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/cli-tests").join(format!("mpeg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, fmt, extra) in [("a.ts", "mpegts", &["-mpegts_m2ts_mode", "0"][..]), ("b.vob", "vob", &[][..])] {
        let out = dir.join(name);
        let st = Command::new(ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x240:rate=25", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000"])
            .args(["-t", "0.5", "-c:v", "mpeg2video", "-c:a", "mp2", "-ac", "2", "-f", fmt])
            .args(extra)
            .arg(&out)
            .status()
            .unwrap();
        assert!(st.success());
        let v = json_out(&cli(&["probe", out.to_str().unwrap()]));
        assert!(v["video"]["codec"].as_str().unwrap().starts_with("MPEG-2 Video"), "{v}");
        assert_eq!(v["audio_streams"][0]["codec"], "MPEG Audio");
        let streams = v["mpeg"]["streams"].as_array().unwrap();
        assert_eq!(streams.len(), 2, "{v}");
        assert_eq!(streams[0]["codec"], "MPEG-2 Video");
        assert!(streams[0]["pictures"]["I"].as_u64().unwrap() >= 1);
        assert_eq!(v["mpeg"]["format"], if fmt == "mpegts" { "MPEG-2 TS" } else { "MPEG-2 PS" });
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exec_inspect_and_describe() {
    let seq = json_out(&cli(&["--demo", "inspect", "sequence", "--compact"]));
    assert!(seq["tracks"].is_array() || seq.is_object(), "{seq}");
    let d = json_out(&cli(&["describe", "timeline.razor"]));
    assert_eq!(d["id"], "timeline.razor");
    assert!(d["params"].is_string());
    let list = json_out(&cli(&["commands", "razor", "--json"]));
    assert!(list.as_array().unwrap().iter().any(|c| c["id"] == "timeline.razor"));
    // An unknown command fails with exit status 1.
    let bad = cli(&["--demo", "exec", "no.such.command"]);
    assert_eq!(bad.status.code(), Some(1));
}

#[test]
fn exec_save_as_then_reopen() {
    let dir = std::env::temp_dir().join(format!("fc-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let proj = dir.join("p.fcproj");
    let p = proj.to_str().unwrap();
    json_out(&cli(&["--demo", "exec", "file.newBin", "name=CLI Selects", "--save-as", p]));
    let tree = json_out(&cli(&["--project", p, "inspect", "project"]));
    assert!(tree.to_string().contains("CLI Selects"), "{tree}");
    // `run` from stdin, editing and saving back in place.
    let mut child = Command::new(env!("CARGO_BIN_EXE_filmcraft-cli"))
        .args(["--project", p, "--save", "run", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"# comment\n{\"id\":\"file.newBin\",\"params\":{\"name\":\"Second\"}}\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    let line: Value = serde_json::from_slice(out.stdout.split(|b| *b == b'\n').next().unwrap()).unwrap();
    assert_eq!(line["ok"], true);
    assert!(json_out(&cli(&["--project", p, "inspect", "project"])).to_string().contains("Second"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn run_keep_going_reports_failures() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_filmcraft-cli"))
        .args(["--demo", "--keep-going", "run", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"{\"id\":\"no.such\"}\n{\"id\":\"file.newBin\",\"params\":{\"name\":\"x\"}}\n").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let lines: Vec<Value> = String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["ok"], false);
    assert_eq!(lines[1]["ok"], true);
}

fn export_project(name: &str) -> (std::path::PathBuf, String) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("export-tests")
        .join(format!("cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let project = dir.join("p.fcproj").to_str().unwrap().to_string();
    json_out(&cli(&["exec", "file.newSequence", "name=Small", "width=64", "height=36", "fps=24", "--save-as", &project]));
    (dir, project)
}

#[test]
fn retained_movie_exports_wait_for_real_files_and_report_the_written_path() {
    let (dir, project) = export_project("movies");
    let before = std::fs::read(&project).unwrap();
    for (format, filename, video, audio) in [("h264", "movie.mp4", "H.264 (High)", "AAC"), ("prores", "movie.mov", "Apple ProRes 4444", "PCM 32-bit float")] {
        let out = dir.join(filename).to_str().unwrap().to_string();
        let args = ["--project", &project, "export", &out, "--format", format, "--start", "0", "--end", "0.25"];
        let o = cli(&args);
        let result = json_out(&o);
        assert_eq!(result["result"]["frames"], 6, "{result}");
        assert_eq!(result["path"], out);
        assert!(std::fs::metadata(&out).unwrap().len() > 100);
        let info = json_out(&cli(&["probe", &out]));
        assert_eq!(info["video"]["codec"], video, "{info}");
        assert_eq!(info["audio_streams"][0]["codec"], audio, "{info}");
        assert_eq!(info["audio_streams"][0]["channels"], 2, "{info}");
        assert!(String::from_utf8_lossy(&o.stderr).contains(&format!("exported {out} in ")));
        assert!(!cli(&args).status.success(), "existing destination needs explicit overwrite");
        let mut replace = args.to_vec();
        replace.push("--overwrite");
        json_out(&cli(&replace));
    }
    assert_eq!(std::fs::read(&project).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn png_sequence_and_still_commands_use_retained_outputs_and_no_implicit_replacement() {
    let (dir, project) = export_project("images");
    let base = dir.join("frames.png").to_str().unwrap().to_string();
    let args = ["--project", &project, "export", &base, "--format", "png", "--start", "0", "--end", "0.25"];
    assert_eq!(json_out(&cli(&args))["result"]["frames"], 6);
    for i in 0..6 {
        assert!(dir.join(format!("frames{i:03}.png")).exists());
    }
    assert!(!dir.join("frames006.png").exists());
    let mut replace = args.to_vec();
    replace.push("--overwrite");
    assert!(!cli(&replace).status.success(), "PNG sequence replacement is refused");
    for name in ["frame.png", "frame.jpg"] {
        let out = dir.join(name).to_str().unwrap().to_string();
        let args = ["--project", &project, "render", "--seconds", "0", "--out", &out];
        let result = json_out(&cli(&args));
        assert_eq!((result["width"].as_u64(), result["height"].as_u64()), (Some(64), Some(36)));
        let bytes = std::fs::read(&out).unwrap();
        assert!(bytes.starts_with(if name.ends_with(".png") { b"\x89PNG\r\n\x1a\n" } else { b"\xff\xd8" }));
        assert!(!cli(&args).status.success());
        let mut replace = args.to_vec();
        replace.push("--overwrite");
        json_out(&cli(&replace));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn removed_delivery_options_formats_commands_and_hostile_ranges_fail_without_outputs() {
    let (dir, project) = export_project("unsupported");
    let out = dir.join("out.mov").to_str().unwrap().to_string();
    let before = std::fs::read(&project).unwrap();
    for extra in [
        vec!["--preset", "old"],
        vec!["--settings={}"],
        vec!["--queue"],
        vec!["--list-presets"],
        vec!["--data-dir", "old"],
        vec!["--quality", "95"],
        vec!["--scale", "0.5"],
        vec!["--gpu-rendering", "auto"],
        vec!["--no-audio"],
        vec!["--overwrite=false"],
        vec!["--format=prores", "--format=h264"],
        vec!["--format"],
        vec!["--start", "0"],
        vec!["--start", "NaN", "--end", "1"],
        vec!["--start", "0", "--end", "inf"],
        vec!["--start", "-1", "--end", "1"],
        vec!["--range", "entire", "--start", "0", "--end", "1"],
        vec!["--demo"],
    ] {
        let mut args = vec!["--project", &project, "export", &out];
        args.extend_from_slice(&extra);
        let response = cli(&args);
        assert_eq!(response.status.code(), Some(2), "{args:?}: {}", String::from_utf8_lossy(&response.stderr));
    }
    for format in ["wav", "aiff", "aac", "m4a", "hevc", "mjpeg", "gif", "tiff", "bmp", "tga", "dpx", "exr"] {
        assert!(!cli(&["--project", &project, "export", &out, "--format", format]).status.success(), "{format}");
    }
    for command in ["file.exportMedia", "file.exportFrame", "export.quick"] {
        for option in ["--preset=old", "--settings={}", "--queue", "--scale=0.5", "--quality=95", "--overwrite=false"] {
            assert_eq!(cli(&["--project", &project, "exec", command, option]).status.code(), Some(2), "{command} {option}");
        }
    }
    let script = dir.join("export.jsonl");
    std::fs::write(&script, serde_json::json!({"id":"file.exportMedia","params":{"path":out,"format":"prores","wait":true}}).to_string()).unwrap();
    for option in ["--preset=old", "--settings={}", "--queue", "--quality=95", "--scale=0.5"] {
        assert_eq!(cli(&["--project", &project, "run", script.to_str().unwrap(), option]).status.code(), Some(2), "run {option}");
    }
    std::fs::remove_file(script).unwrap();
    for id in ["export.queue.add", "export.queue.start", "export.queue.list", "export.presets.list", "export.presets.save", "export.formats", "export.resolve"]
    {
        assert_eq!(cli(&["--project", &project, "exec", id]).status.code(), Some(1), "{id}");
    }
    let commands = json_out(&cli(&["commands", "export.", "--json"]));
    let ids: Vec<_> = commands.as_array().unwrap().iter().map(|item| item["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["export.quick"]);
    assert_eq!(std::fs::read(&project).unwrap(), before);
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "only the unchanged project remains");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `filmcraft-cli commands | head -c 10` panicked with "failed printing to stdout: Broken pipe
/// (os error 32)" and exit status 101 (#285).
#[test]
fn closed_stdout_ends_quietly() {
    use std::io::Read;
    for args in [&["commands"][..], &["help"], &["--demo", "commands", "--json"]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_filmcraft-cli")).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let mut first = [0u8; 10];
        child.stdout.as_mut().unwrap().read_exact(&mut first).unwrap();
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success() && err.is_empty(), "{args:?}: {:?}: {err}", out.status);
    }
}

/// `run -` with stdout already closed: `run` reads its script to the end before it prints, so its
/// first write fails for sure. The rest of the script and the save still happen, and the exit status
/// still reports a failing command.
fn run_with_closed_stdout(args: &[&str], script: &[u8]) -> Output {
    let mut child =
        Command::new(env!("CARGO_BIN_EXE_filmcraft-cli")).args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    drop(child.stdout.take());
    child.stdin.take().unwrap().write_all(script).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn closed_stdout_still_runs_the_script_and_saves() {
    let dir = std::env::temp_dir().join(format!("fc-cli-pipe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let proj = dir.join("p.fcproj");
    let p = proj.to_str().unwrap();
    let script = b"{\"id\":\"file.newBin\",\"params\":{\"name\":\"Piped One\"}}\n{\"id\":\"file.newBin\",\"params\":{\"name\":\"Piped Two\"}}\n";
    let out = run_with_closed_stdout(&["--demo", "--save-as", p, "run", "-"], script);
    assert!(out.status.success(), "{:?}: {}", out.status, String::from_utf8_lossy(&out.stderr));
    let tree = json_out(&cli(&["--project", p, "inspect", "project"])).to_string();
    assert!(tree.contains("Piped One") && tree.contains("Piped Two"), "{tree}");

    // a failing command is still a failure, whether or not anyone reads stdout
    let out = run_with_closed_stdout(&["--demo", "run", "-"], b"{\"id\":\"no.such.command\"}\n");
    assert_eq!(out.status.code(), Some(1));
    let out = run_with_closed_stdout(
        &["--demo", "--keep-going", "run", "-"],
        b"{\"id\":\"no.such.command\"}\n{\"id\":\"file.newBin\",\"params\":{\"name\":\"x\"}}\n",
    );
    assert_eq!(out.status.code(), Some(1));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Invalid time and removed scaling are usage errors, not silently ignored delivery arguments.
#[test]
fn render_rejects_bad_numbers() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join(format!("fc-cli-render-nan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let still = dir.join("s.png").to_str().unwrap().to_string();
    for (opt, bad) in [
        ("--seconds", "1s"),
        ("--seconds", "1,5"),
        ("--seconds", "abc"),
        ("--seconds", "NaN"),
        ("--seconds", "inf"),
        ("--seconds", "-1"),
        ("--seconds", "1e300"),
        ("--scale", "0.5"),
        ("--scale", "NaN"),
    ] {
        let o = cli(&["--demo", "render", opt, bad, "--out", &still]);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(2), "{opt} {bad}: {err}");
        assert!(err.contains("filmcraft-cli:") && err.contains(opt), "{opt} {bad}: {err}");
        assert!(!std::path::Path::new(&still).exists(), "{opt} {bad} wrote a file");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// #1054: exporting or rendering a project whose media is missing fails (exit 1) and names the
/// offline items on stderr and in the result's `offlineMedia`, unless `--allow-offline` accepts the
/// placeholder frames; then it still warns.
#[test]
fn export_with_missing_media_fails_unless_allowed() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join(format!("fc-cli-1054-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let f = |n: &str| dir.join(n).to_str().unwrap().to_string();
    let (still, proj) = (f("still.png"), f("p.fcproj"));
    let o = cli(&["--demo", "render", "--seconds", "1", "--out", &still]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    json_out(&cli(&["exec", "file.newSequence", "name=s", "width=64", "height=36", "fps=24", "--save-as", &proj]));
    let paths = format!("paths=[\"{}\"]", still.replace('\\', "\\\\"));
    let item = json_out(&cli(&["--project", &proj, "--save", "exec", "file.import", &paths]))["items"][0].as_u64().unwrap();
    json_out(&cli(&["--project", &proj, "--save", "exec", "timeline.place", &format!("item={item}"), "track=V1", "frame=0", "audio=false"]));
    let export = |out: &str, extra: &[&str]| {
        let mut args = vec!["--project", proj.as_str(), "--compact", "export", out, "--format", "png", "--start", "0", "--end", "0.05"];
        args.extend_from_slice(extra);
        cli(&args)
    };

    let on = export(&f("on.png"), &[]);
    let v = json_out(&on);
    assert_eq!(v["offlineMedia"], serde_json::json!([]), "{v}");
    assert!(!String::from_utf8_lossy(&on.stderr).contains("warning"), "{}", String::from_utf8_lossy(&on.stderr));

    std::fs::remove_file(&still).unwrap();
    let off = export(&f("off.png"), &[]);
    let err = String::from_utf8_lossy(&off.stderr);
    assert_eq!(off.status.code(), Some(1), "{err}");
    assert!(err.contains("still.png") && err.contains("missing") && err.contains("--allow-offline"), "{err}");
    let v: Value = serde_json::from_slice(&off.stdout).expect("the result JSON is still printed");
    assert_eq!(v["offlineMedia"][0]["item"], item, "{v}");
    assert_eq!(v["offlineMedia"][0]["status"], "missing", "{v}");

    let allowed = export(&f("allowed.png"), &["--allow-offline"]);
    let err = String::from_utf8_lossy(&allowed.stderr);
    assert!(allowed.status.success(), "{err}");
    assert!(err.contains("still.png") && err.contains("missing"), "still warns: {err}");
    assert_eq!(json_out(&allowed)["offlineMedia"][0]["status"], "missing");

    let render = cli(&["--project", &proj, "render", "--seconds", "0", "--out", &f("r.png")]);
    let err = String::from_utf8_lossy(&render.stderr);
    assert_eq!(render.status.code(), Some(1), "{err}");
    assert!(err.contains("still.png") && err.contains("--allow-offline"), "{err}");
    let render = cli(&["--project", &proj, "render", "--seconds", "0", "--out", &f("r.png"), "--overwrite", "--allow-offline"]);
    assert!(render.status.success(), "{}", String::from_utf8_lossy(&render.stderr));
    assert!(String::from_utf8_lossy(&render.stderr).contains("still.png"));
    let _ = std::fs::remove_dir_all(&dir);
}
