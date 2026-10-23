//! The removed-capability deny-list (#27).
//!
//! The reduction (#14) deleted everything outside the animation workflow. This check keeps it
//! deleted: it fails when a removed crate, module or command family reappears in a manifest, a
//! `use` path, a module declaration or a command registration. `layers` runs it, so `cargo xtask
//! ci` does too.
//!
//! Tests and test fixtures are not scanned: they may name a removed command id to prove it is
//! refused.

/// Crates the reduction deleted (name without the `waltcut-` / `filmcraft-` prefix).
const CRATES: &[&str] = &["ac3", "apv", "captions", "cfb", "dng", "dnx", "flac", "interchange", "mxf", "ogg", "scopes", "speech", "tts", "tts-text", "vp8"];

/// Application crates the reduction deleted (directory under `apps/`).
const APPS: &[&str] = &["filmcraft-web"];

/// Command-id families (the part before the first dot) that no registration may use.
const COMMAND_PREFIXES: &[&str] = &["captions", "essentialSound", "lumetri", "lut", "masks", "mixer", "multicam", "transcript", "tts"];

/// Modules the reduction deleted, by the source directory that must not declare them again.
const MODULES: &[(&str, &[&str])] = &[
    ("crates/project/src/", &["caption", "essential", "gtemplate", "mask", "mixer", "multicam", "narration", "transcript", "vtransition"]),
    (
        "crates/engine/src/",
        &[
            "aaf_omf",
            "captions",
            "essential_sound",
            "graphic_templates",
            "interchange",
            "masks",
            "media_browser",
            "mixer",
            "multicam",
            "narration",
            "project_manager",
            "proxies",
            "remix",
            "scene_detect",
            "scopes",
            "transcript",
            "voiceover",
        ],
    ),
    ("crates/render/src/", &["audio_fx", "lumetri_presets", "luts", "mixer", "multicam", "remix"]),
    (
        "crates/ui-egui/src/panels/",
        &[
            "audio_fx_editor",
            "essential_sound",
            "graphics_templates",
            "interchange_export",
            "lumetri",
            "masks",
            "media_browser",
            "metadata",
            "meters",
            "mixer",
            "multicam",
            "notes",
            "remix",
            "scopes",
            "timecode",
            "timeline_automation",
            "timeline_captions",
            "transcript_cache",
            "transition_controls",
            "tts",
            "voiceover",
        ],
    ),
];

/// A file to scan: its repo-relative path (forward slashes) and contents.
pub struct Source<'a> {
    pub path: &'a str,
    pub text: &'a str,
}

fn is_test_file(path: &str) -> bool {
    path.contains("/tests/") || path.rsplit('/').next().is_some_and(|name| name.contains("tests") || name.contains("test_util"))
}

/// Whether `text` holds `word` as a whole identifier (not part of a longer one).
fn has_word(text: &str, word: &str) -> bool {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(word).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + word.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

/// Every denied item found in `files`, one message each.
pub fn check(files: &[Source]) -> Vec<String> {
    let mut errors = Vec::new();
    for f in files {
        let (path, text) = (f.path, f.text);
        if path.starts_with("xtask/") {
            continue;
        }
        if path.ends_with("Cargo.toml") {
            check_manifest(path, text, &mut errors);
        } else if path.ends_with(".rs") && !is_test_file(path) {
            check_rust(path, text, &mut errors);
        }
    }
    errors
}

fn check_manifest(path: &str, text: &str, errors: &mut Vec<String>) {
    for c in CRATES {
        for prefix in ["waltcut-", "filmcraft-"] {
            let name = format!("{prefix}{c}");
            // a dependency key, or the package name
            let dependency = text.lines().any(|l| {
                let l = l.trim_start();
                l.strip_prefix(&name).is_some_and(|rest| rest.trim_start().starts_with('=') || rest.trim_start().starts_with('.'))
            });
            let package = text.lines().any(|l| l.trim().replace(' ', "") == format!("name=\"{name}\""));
            if dependency || package {
                errors.push(format!("{path}: removed crate `{name}` is back (deny-list, #27)"));
            }
        }
        if text.contains(&format!("\"crates/{c}\"")) {
            errors.push(format!("{path}: removed crate directory `crates/{c}` is a workspace member again (deny-list, #27)"));
        }
    }
    for a in APPS {
        if text.contains(&format!("\"apps/{a}\"")) || text.lines().any(|l| l.trim().replace(' ', "") == format!("name=\"{a}\"")) {
            errors.push(format!("{path}: removed application `{a}` is back (deny-list, #27)"));
        }
    }
}

fn check_rust(path: &str, text: &str, errors: &mut Vec<String>) {
    for c in CRATES {
        for prefix in ["waltcut_", "filmcraft_"] {
            let ident = format!("{prefix}{}", c.replace('-', "_"));
            if has_word(text, &ident) {
                errors.push(format!("{path}: uses removed crate `{ident}` (deny-list, #27)"));
            }
        }
    }
    for (dir, names) in MODULES {
        if !path.starts_with(dir) {
            continue;
        }
        for line in text.lines() {
            let line = line.trim_start();
            let rest = line.strip_prefix("pub(crate) ").or_else(|| line.strip_prefix("pub ")).unwrap_or(line);
            let Some(decl) = rest.strip_prefix("mod ") else { continue };
            let name = decl.trim_end_matches(|c: char| c == ';' || c.is_whitespace());
            if names.contains(&name) {
                errors.push(format!("{path}: removed module `{name}` is declared again (deny-list, #27)"));
            }
        }
    }
    if path.starts_with("crates/engine/src/") {
        check_commands(path, text, errors);
    }
}

/// Command registrations (`cmd!("id"`, `spec("id"`, `uic!("id"`), with rustfmt's line breaks.
fn check_commands(path: &str, text: &str, errors: &mut Vec<String>) {
    for opener in ["cmd!(", "spec(", "uic!("] {
        for (i, _) in text.match_indices(opener) {
            let rest = text[i + opener.len()..].trim_start();
            let Some(rest) = rest.strip_prefix('"') else { continue };
            let Some(end) = rest.find('"') else { continue };
            let id = &rest[..end];
            let family = id.split('.').next().unwrap_or(id);
            if id.contains('.') && COMMAND_PREFIXES.contains(&family) {
                errors.push(format!("{path}: registers removed command `{id}` (deny-list, #27)"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(path: &str, text: &str) -> Vec<String> {
        check(&[Source { path, text }])
    }

    #[test]
    fn a_clean_tree_passes() {
        assert!(one("crates/engine/src/lib.rs", "use waltcut_project::Project;\npub mod commands;\n").is_empty());
        assert!(one("crates/project/Cargo.toml", "[dependencies]\nwaltcut-time = { workspace = true }\n").is_empty());
    }

    #[test]
    fn a_re_added_import_fails() {
        let e = one("crates/engine/src/commands.rs", "use waltcut_captions::Cue;\n");
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(e[0].contains("waltcut_captions"));
        // a longer identifier is not the removed crate
        assert!(one("crates/engine/src/commands.rs", "let waltcut_captions_total = 1;\n").is_empty());
        assert_eq!(one("crates/engine/src/commands.rs", "use waltcut_tts_text::normalise;\n").len(), 1);
    }

    #[test]
    fn a_re_added_manifest_entry_fails() {
        assert_eq!(one("crates/engine/Cargo.toml", "[dependencies]\nwaltcut-interchange = { workspace = true }\n").len(), 1);
        assert_eq!(one("Cargo.toml", "members = [\"crates/scopes\"]\n").len(), 1);
        assert_eq!(one("crates/ac3/Cargo.toml", "[package]\nname = \"waltcut-ac3\"\n").len(), 1);
        assert_eq!(one("apps/filmcraft-web/Cargo.toml", "[package]\nname = \"filmcraft-web\"\n").len(), 1);
    }

    #[test]
    fn a_re_added_module_fails_only_in_its_crate() {
        assert_eq!(one("crates/engine/src/lib.rs", "pub mod mixer;\n").len(), 1);
        assert_eq!(one("crates/ui-egui/src/panels/mod.rs", "    pub(crate) mod masks;\n").len(), 1);
        assert!(one("crates/frame/src/lib.rs", "pub mod mixer;\n").is_empty());
    }

    #[test]
    fn a_re_added_command_fails_even_across_a_line_break() {
        assert_eq!(one("crates/engine/src/commands.rs", "cmd!(\"masks.add\", \"Add\", [], None, \"{}\", always, |s, p| Ok(()))\n").len(), 1);
        assert_eq!(one("crates/engine/src/commands.rs", "cmd!(\n    \"captions.import\",\n    \"Import\",\n)\n").len(), 1);
        assert!(one("crates/engine/src/commands.rs", "cmd!(\"project.save\", \"Save\", [], None, \"{}\", always, |s, p| Ok(()))\n").is_empty());
    }

    #[test]
    fn tests_may_name_removed_commands() {
        assert!(one("crates/engine/src/audio_reduction_tests.rs", "pub mod mixer;\nuse waltcut_captions::Cue;\n").is_empty());
        assert!(one("crates/ui-egui/tests/scripted.rs", "json!({\"command\": \"mixer.setStrip\"})\nuse waltcut_scopes::Scope;\n").is_empty());
    }
}
