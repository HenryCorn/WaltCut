//! Cheap shared gate. Unknown/global inputs deliberately select the full release test suite.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

#[derive(Debug)]
struct Package {
    name: String,
    directory: String,
    dependencies: BTreeSet<String>,
}

fn normalized(path: &str) -> String {
    path.replace('\\', "/").trim_end_matches('/').to_owned()
}

fn packages(md: &Value) -> Result<Vec<Package>, String> {
    let root = normalized(md["workspace_root"].as_str().ok_or("metadata: missing workspace_root")?);
    let members: BTreeSet<&str> = md["workspace_members"].as_array().ok_or("metadata: missing workspace_members")?.iter().filter_map(Value::as_str).collect();
    let mut result = Vec::new();
    for p in md["packages"].as_array().ok_or("metadata: missing packages")? {
        if !p["id"].as_str().is_some_and(|id| members.contains(id)) {
            continue;
        }
        let manifest = normalized(p["manifest_path"].as_str().ok_or("metadata: missing manifest_path")?);
        let directory = manifest
            .strip_prefix(&format!("{root}/"))
            .and_then(|m| m.strip_suffix("/Cargo.toml"))
            .ok_or_else(|| format!("metadata: package outside workspace or at workspace root: {manifest}"))?;
        result.push(Package {
            name: p["name"].as_str().ok_or("metadata: missing package name")?.to_owned(),
            directory: directory.to_owned(),
            // Include normal, build, dev and target-specific dependencies, even renamed ones.
            dependencies: p["dependencies"]
                .as_array()
                .ok_or("metadata: missing dependencies")?
                .iter()
                .filter(|d| d["path"].is_string())
                .filter_map(|d| d["name"].as_str().map(str::to_owned))
                .collect(),
        });
    }
    if result.is_empty() {
        return Err("metadata: no workspace packages".into());
    }
    Ok(result)
}

#[derive(Debug, PartialEq, Eq)]
enum Selection {
    All(String),
    Packages(BTreeSet<String>),
}

fn documentation(path: &str) -> bool {
    // Do not treat code, templates, assets or configuration under docs/ as prose.
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("md" | "rst") => true,
        Some("txt") => path.starts_with("docs/"),
        _ => false,
    }
}

fn select(packages: &[Package], changed: &BTreeSet<String>) -> Selection {
    let mut selected = BTreeSet::new();
    for path in changed {
        let path = normalized(path);
        // Configuration remains global even if its filename happens to look like prose.
        if path.starts_with(".github/")
            || path.starts_with(".cargo/")
            || matches!(Path::new(&path).file_name().and_then(|name| name.to_str()), Some("Cargo.toml" | "Cargo.lock"))
            || matches!(path.as_str(), "rust-toolchain" | "rust-toolchain.toml" | "rustfmt.toml" | ".rustfmt.toml" | "clippy.toml" | ".clippy.toml")
        {
            return Selection::All(format!("global input: {path}"));
        }
        if documentation(&path) {
            continue;
        }
        let owner = packages.iter().filter(|p| path.strip_prefix(&p.directory).is_some_and(|tail| tail.starts_with('/'))).max_by_key(|p| p.directory.len());
        let Some(owner) = owner else {
            return Selection::All(format!("unclassified input (possibly a deleted package): {path}"));
        };
        selected.insert(owner.name.clone());
    }
    // At most one insertion per workspace member: cycles cannot loop indefinitely.
    loop {
        let before = selected.len();
        for p in packages {
            if p.dependencies.iter().any(|d| selected.contains(d)) {
                selected.insert(p.name.clone());
            }
        }
        if selected.len() == before {
            break;
        }
    }
    Selection::Packages(selected)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git").current_dir(root).args(args).output().map_err(|e| format!("git {args:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

fn text(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|e| format!("git output is not UTF-8; cannot safely classify changes: {e}"))
}

fn changed_files(root: &Path, base: &str) -> Result<BTreeSet<String>, String> {
    // Resolve before using the argument in any other Git command. Never let a ref become an option.
    let revision = format!("{base}^{{commit}}");
    let commit = text(git(root, &["rev-parse", "--verify", "--end-of-options", &revision])?).map_err(|e| format!("invalid base ref {base:?}: {e}"))?;
    let ancestor = text(git(root, &["merge-base", commit.trim(), "HEAD"])?)?;
    let mut changed = BTreeSet::new();
    // Disabling rename detection reports both old and new names, including deleted crate paths.
    for args in [
        vec!["diff", "--no-ext-diff", "--no-textconv", "--name-only", "-z", "--no-renames", ancestor.trim(), "HEAD", "--"],
        vec!["diff", "--no-ext-diff", "--no-textconv", "--name-only", "-z", "--no-renames", "--cached", "HEAD", "--"],
        vec!["diff", "--no-ext-diff", "--no-textconv", "--name-only", "-z", "--no-renames", "--"],
        vec!["ls-files", "--others", "--exclude-standard", "-z", "--"],
    ] {
        changed.extend(text(git(root, &args)?)?.split('\0').filter(|s| !s.is_empty()).map(str::to_owned));
    }
    Ok(changed)
}

fn base_arg(args: &[String]) -> Result<&str, String> {
    match args {
        [] => Ok("origin/main"),
        [flag, base] if flag == "--base" && !base.is_empty() => Ok(base),
        _ => Err("usage: cargo xtask check [--base REF]".into()),
    }
}

fn test_command(selection: &Selection) -> Option<Command> {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["test", "--release"]);
    match selection {
        Selection::All(_) => {
            cmd.arg("--workspace");
        }
        Selection::Packages(names) if names.is_empty() => return None,
        Selection::Packages(names) => {
            for name in names {
                cmd.args(["--package", name]);
            }
        }
    }
    Some(cmd)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let base = base_arg(args)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("xtask has no workspace parent")?;
    let changed = changed_files(root, base).map_err(|e| format!("check: cannot compare with base {base:?}: {e}"))?;
    std::env::set_current_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let selection = select(&packages(&super::metadata()?)?, &changed);
    match &selection {
        Selection::All(reason) => eprintln!("check: all workspace tests ({reason})"),
        Selection::Packages(names) if names.is_empty() => eprintln!("check: no code changes; skipping tests only"),
        Selection::Packages(names) => eprintln!("check: testing {}", names.iter().cloned().collect::<Vec<_>>().join(", ")),
    }
    super::run(Command::new(env!("CARGO")).args(["fmt", "--check"]))?;
    super::run(Command::new(env!("CARGO")).args(["check", "--workspace", "--all-targets"]))?;
    super::layers()?;
    super::assets()?;
    if let Some(mut cmd) = test_command(&selection) {
        super::run(&mut cmd)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn workspace() -> Vec<Package> {
        let mut md = json!({
            "workspace_root": "C:\\work\\repo",
            "workspace_members": ["core", "dev", "build", "app", "nested", "unrelated"],
            "packages": []
        });
        for (name, directory, deps) in [
            ("core", "crates/core", vec![]),
            ("dev", "crates/dev", vec![("core", "dev")]),
            ("build", "crates/build", vec![("dev", "build")]),
            ("app", "apps/app", vec![("build", "normal")]),
            ("nested", "crates/core/nested", vec![]),
            ("unrelated", "crates/unrelated", vec![]),
        ] {
            md["packages"].as_array_mut().unwrap().push(json!({
                "id": name, "name": name, "manifest_path": format!("C:\\work\\repo\\{directory}\\Cargo.toml"),
                "dependencies": deps.into_iter().map(|(name, kind)| json!({
                    "name": name, "kind": kind, "rename": "alias", "target": "cfg(unix)", "path": format!("C:\\work\\repo\\crates\\{name}")
                })).collect::<Vec<_>>()
            }));
        }
        packages(&md).unwrap()
    }

    fn selection(paths: &[&str]) -> Selection {
        select(&workspace(), &paths.iter().map(|p| p.to_string()).collect())
    }

    #[test]
    fn includes_transitive_dev_build_and_renamed_dependencies() {
        assert_eq!(selection(&["crates/core/src/lib.rs"]), Selection::Packages(["app", "build", "core", "dev"].map(str::to_owned).into()));
    }

    #[test]
    fn longest_package_path_and_windows_separators() {
        assert_eq!(selection(&["crates\\core\\nested\\src\\lib.rs"]), Selection::Packages(["nested".to_owned()].into()));
    }

    #[test]
    fn dependency_cycles_terminate_and_unrelated_packages_stay_out() {
        let mut packages = workspace();
        packages.iter_mut().find(|p| p.name == "core").unwrap().dependencies.insert("app".into());
        assert_eq!(
            select(&packages, &["crates/core/src/lib.rs".to_owned()].into()),
            Selection::Packages(["app", "build", "core", "dev"].map(str::to_owned).into())
        );
    }

    #[test]
    fn real_no_deps_metadata_is_supported() {
        let parsed = packages(&super::super::metadata().unwrap()).unwrap();
        assert!(parsed.iter().any(|p| p.name == "xtask" && p.directory == "xtask"));
        assert!(parsed.iter().any(|p| p.name == "waltcut-testkit"));
    }

    #[test]
    fn global_and_unknown_inputs_select_everything() {
        for path in [
            "Cargo.toml",
            "Cargo.lock",
            "crates/core/Cargo.toml",
            "crates/core/Cargo.lock",
            "crates/core/deleted-nested/Cargo.toml",
            ".cargo/config.toml",
            "rust-toolchain.toml",
            "rustfmt.toml",
            ".github/workflows/ci.yml",
            ".github/scripts/test.sh",
            "scripts/check.rs",
            "docs/example.rs",
            "assets/input.png",
            "fonts/manifest.txt",
            "crates/deleted/src/lib.rs",
            "crates/core-other/src/lib.rs",
        ] {
            assert!(matches!(selection(&[path]), Selection::All(_)), "{path}");
        }
    }

    #[test]
    fn docs_only_and_no_changes_skip_only_tests() {
        for paths in [&[][..], &["docs/testing.md", "README.md", "crates/core/README.md"][..]] {
            let selected = selection(paths);
            assert_eq!(selected, Selection::Packages(BTreeSet::new()));
            assert!(test_command(&selected).is_none());
        }
    }

    #[test]
    fn test_runner_is_single_sorted_release_invocation() {
        let cmd = test_command(&selection(&["crates/core/src/lib.rs"])).unwrap();
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["test", "--release", "--package", "app", "--package", "build", "--package", "core", "--package", "dev"]
        );
        let cmd = test_command(&Selection::All("config".into())).unwrap();
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["test", "--release", "--workspace"]);
    }

    #[test]
    fn strict_arguments_and_invalid_refs() {
        assert_eq!(base_arg(&[]).unwrap(), "origin/main");
        assert_eq!(base_arg(&["--base".into(), "HEAD".into()]).unwrap(), "HEAD");
        for args in [vec!["--base".into()], vec!["--unknown".into()], vec!["--base".into(), "".into()]] {
            assert!(base_arg(&args).is_err());
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for base in ["--help", "--output=hostile", "--exec-path=hostile", "HEAD; echo hostile", "refs/heads/xtask-nonexistent-ref-for-test"] {
            assert!(changed_files(root, base).is_err(), "{base}");
        }
    }

    #[test]
    fn git_changes_include_merge_base_renames_deletions_and_local_files() {
        struct Repo(std::path::PathBuf);
        impl Drop for Repo {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let repo = Repo(Path::new(env!("CARGO_MANIFEST_DIR")).join("../target").join(format!("xtask-check-test-{}", std::process::id())));
        std::fs::create_dir_all(&repo.0).unwrap();
        git(&repo.0, &["init", "--initial-branch=main"]).unwrap();
        git(&repo.0, &["config", "user.name", "xtask test"]).unwrap();
        git(&repo.0, &["config", "user.email", "xtask@example.invalid"]).unwrap();
        git(&repo.0, &["config", "commit.gpgsign", "false"]).unwrap();
        for name in ["rename.rs", "local rename.rs", "deleted.rs", "staged.rs", "dirty.rs", "reverted.rs"] {
            std::fs::write(repo.0.join(name), name).unwrap();
        }
        git(&repo.0, &["add", "."]).unwrap();
        git(&repo.0, &["commit", "-m", "initial"]).unwrap();
        git(&repo.0, &["checkout", "-b", "topic"]).unwrap();
        git(&repo.0, &["mv", "rename.rs", "renamed.rs"]).unwrap();
        git(&repo.0, &["rm", "deleted.rs"]).unwrap();
        std::fs::write(repo.0.join("committed.rs"), "committed").unwrap();
        git(&repo.0, &["add", "."]).unwrap();
        git(&repo.0, &["commit", "-m", "topic"]).unwrap();
        // The base has advanced independently; its change must not be selected.
        git(&repo.0, &["checkout", "main"]).unwrap();
        std::fs::write(repo.0.join("base-only.rs"), "base only").unwrap();
        git(&repo.0, &["add", "."]).unwrap();
        git(&repo.0, &["commit", "-m", "base advances"]).unwrap();
        git(&repo.0, &["checkout", "topic"]).unwrap();
        std::fs::write(repo.0.join("staged.rs"), "staged change").unwrap();
        std::fs::write(repo.0.join("reverted.rs"), "staged then reverted").unwrap();
        git(&repo.0, &["add", "."]).unwrap();
        std::fs::write(repo.0.join("reverted.rs"), "reverted.rs").unwrap();
        std::fs::write(repo.0.join("dirty.rs"), "unstaged change").unwrap();
        std::fs::write(repo.0.join("untracked file.rs"), "new").unwrap();
        std::fs::rename(repo.0.join("local rename.rs"), repo.0.join("untracked rename.rs")).unwrap();
        assert_eq!(
            changed_files(&repo.0, "main").unwrap(),
            [
                "rename.rs",
                "renamed.rs",
                "local rename.rs",
                "untracked rename.rs",
                "deleted.rs",
                "committed.rs",
                "staged.rs",
                "dirty.rs",
                "reverted.rs",
                "untracked file.rs"
            ]
            .map(str::to_owned)
            .into()
        );
        git(&repo.0, &["add", "."]).unwrap();
        git(&repo.0, &["commit", "-m", "local changes"]).unwrap();
        // GitHub checks out a synthetic PR merge, not the unmerged topic tip.
        git(&repo.0, &["merge", "--no-ff", "main", "-m", "synthetic PR merge"]).unwrap();
        let merged_changes = changed_files(&repo.0, "main").unwrap();
        assert!(repo.0.join("base-only.rs").exists(), "the merge includes the updated base");
        assert!(!merged_changes.contains("base-only.rs"), "current base changes are not PR changes");
        assert!(merged_changes.contains("committed.rs"));
        assert!(merged_changes.contains("untracked rename.rs"));
        assert!(changed_files(&repo.0, "main~1").unwrap().contains("base-only.rs"), "updates since an older requested base are included");
    }
}
