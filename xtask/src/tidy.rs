//! `cargo xtask tidy [--apply] [--fixtures] [--if-over GB]`: show what `target/` takes and reclaim the regenerable
//! part. Dry run by default; `--apply` deletes build caches (they rebuild on the next `cargo`
//! run); `--fixtures` also deletes generated test media (`cargo xtask fixtures` makes it again);
//! `--if-over GB` makes `--apply` a no-op below that size, for running after every task.

use std::path::{Path, PathBuf};

/// (folder under `target/`, what it is, what deleting it costs).
const CACHES: [(&str, &str); 4] = [
    ("debug", "debug build output (the big one)"),
    ("dev", "dev-profile build output"),
    ("wasm32-unknown-unknown", "WASM build output"),
    ("export-tests", "files written by export tests"),
];

fn size(p: &Path) -> u64 {
    let Ok(md) = std::fs::symlink_metadata(p) else { return 0 };
    if md.is_file() {
        return md.len();
    }
    if !md.is_dir() {
        return 0;
    }
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| size(&e.path())).sum()).unwrap_or(0)
}

fn human(b: u64) -> String {
    let g = b as f64 / (1u64 << 30) as f64;
    if g >= 1.0 { format!("{g:.1} GB") } else { format!("{:.0} MB", b as f64 / (1u64 << 20) as f64) }
}

pub fn run(root: &Path, args: &[&str]) -> Result<(), String> {
    let apply = args.contains(&"--apply");
    let fixtures = args.contains(&"--fixtures");
    let mut over_gb = None;
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match *a {
            "--apply" | "--fixtures" => {}
            "--if-over" => {
                over_gb = Some(it.next().and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite() && *v >= 0.0).ok_or("--if-over takes a number of GB")?);
            }
            bad => return Err(format!("unknown option `{bad}` (usage: cargo xtask tidy [--apply] [--fixtures] [--if-over GB])")),
        }
    }
    let target = root.join("target");
    let mut items: Vec<(PathBuf, String)> = CACHES.iter().map(|(d, why)| (target.join(d), (*why).to_string())).collect();
    if let Ok(rd) = std::fs::read_dir(&target) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("agent-") {
                items.push((e.path(), "a parallel agent's build directory".into()));
            }
        }
    }
    if fixtures {
        items.push((target.join("fixtures"), "generated test media".into()));
    }
    let mut total = 0;
    for (p, why) in &items {
        let b = size(p);
        total += b;
        if b > 0 {
            println!("{:>9}  {}  ({why})", human(b), p.strip_prefix(root).unwrap_or(p).display());
        }
    }
    let kept = size(&target.join("fixtures"));
    if !fixtures && kept > 0 {
        println!("{:>9}  target/fixtures  (kept; add --fixtures to delete)", human(kept));
    }
    if over_gb.is_some_and(|gb| (total as f64) < gb * (1u64 << 30) as f64) {
        println!("{} in build caches, under the limit; nothing deleted.", human(total));
        return Ok(());
    }
    if !apply {
        println!("{} can be reclaimed. Nothing deleted; run `cargo xtask tidy --apply` to delete the items above.", human(total));
        return Ok(());
    }
    for (p, _) in &items {
        if p.exists() {
            std::fs::remove_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
    }
    println!("deleted {}; the next build recreates what it needs.", human(total));
    Ok(())
}
