// SPDX-License-Identifier: MIT

//! Docs <-> CLI drift guard: every `openbnct <sub> [<subsub> ...]`
//! invocation quoted in the README, docs and examples (and in the CLI's own
//! `///` help comments) must name a command that exists in the real clap
//! tree. The tree is read from the built binary's `--help` output.
//!
//! Mark a fenced block as not-yet-implemented by putting `planned` or
//! `future` in its info string (```` ```text planned ````) or in the line
//! directly above the fence; a single line is skipped when it contains
//! `(planned)` or `(future)`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Subcommand names under `path`, or empty when it is a leaf.
fn subcommands(
    path: &[String],
    cache: &mut BTreeMap<Vec<String>, BTreeSet<String>>,
) -> BTreeSet<String> {
    if let Some(hit) = cache.get(path) {
        return hit.clone();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_openbnct"))
        .args(path)
        .arg("--help")
        .output()
        .expect("spawn openbnct --help");
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut names = BTreeSet::new();
    let mut in_commands = false;
    for line in text.lines() {
        if line.starts_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.trim().is_empty() {
                break;
            }
            if let Some(name) = line.split_whitespace().next() {
                names.insert(name.to_owned());
            }
        }
    }
    names.remove("help");
    cache.insert(path.to_vec(), names.clone());
    names
}

fn is_binary_token(token: &str) -> bool {
    token == "openbnct" || token.ends_with("/openbnct")
}

/// Command words following the binary name on one line of text.
fn words_after_binary(text: &str) -> Vec<Vec<String>> {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut found = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let bare = token.trim_matches(|c| c == '`' || c == '"' || c == '(');
        let cargo_package = bare == "openbnct-cli" && tokens.get(index + 1) == Some(&"--");
        if !is_binary_token(bare) && !cargo_package {
            continue;
        }
        // `pip install openbnct jupyter` names a package, not the binary.
        if tokens[..index].contains(&"install") {
            continue;
        }
        // `cargo run --bin openbnct --` names the binary as a flag value;
        // the real command words follow the `--`.
        let mut rest = &tokens[index + 1..];
        if rest.first() == Some(&"--") {
            rest = &rest[1..];
        }
        let mut words = Vec::new();
        for raw in rest {
            let word = raw.trim_matches(|c: char| {
                matches!(c, '`' | '.' | ',' | ';' | ':' | ')' | '(' | '"' | '\'')
            });
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && word.chars().next().is_some_and(|c| c.is_ascii_lowercase());
            let closes = raw.ends_with('`') || raw.ends_with('.') || raw.ends_with(',');
            if !plain || raw.starts_with('-') {
                break;
            }
            words.push(word.to_owned());
            if closes {
                break;
            }
        }
        found.push(words);
    }
    found
}

struct Invocation {
    origin: String,
    words: Vec<String>,
}

fn scan_markdown(path: &Path, root: &Path, out: &mut Vec<Invocation>) {
    let text = std::fs::read_to_string(path).unwrap();
    let rel = path.strip_prefix(root).unwrap().display().to_string();
    let mut in_fence = false;
    let mut skip_fence = false;
    let mut previous = String::new();
    for (number, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            if in_fence {
                in_fence = false;
                skip_fence = false;
            } else {
                in_fence = true;
                let flagged = |s: &str| {
                    let s = s.to_lowercase();
                    s.contains("planned") || s.contains("future")
                };
                skip_fence = flagged(trimmed) || flagged(&previous);
            }
            previous = line.to_owned();
            continue;
        }
        previous = line.to_owned();
        let lower = line.to_lowercase();
        if lower.contains("(planned)") || lower.contains("(future)") {
            continue;
        }
        let origin = format!("{rel}:{}", number + 1);
        if in_fence {
            if skip_fence {
                continue;
            }
            for words in words_after_binary(line) {
                out.push(Invocation {
                    origin: origin.clone(),
                    words,
                });
            }
        } else {
            for (i, span) in line.split('`').enumerate() {
                if i % 2 == 1 {
                    for words in words_after_binary(span) {
                        out.push(Invocation {
                            origin: origin.clone(),
                            words,
                        });
                    }
                }
            }
        }
    }
}

fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut files = vec![root.join("README.md"), root.join("README.ja.md")];
    // docs/ recursively; docs/adr is skipped in the test (historical
    // decision records may name since-renamed commands).
    let mut stack = vec![root.join("docs"), root.join("examples")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md") {
                files.push(path);
            }
        }
    }
    files.retain(|p| p.exists());
    files.sort();
    files
}

/// Does the command path exist? Returns the first bad word if not.
fn first_unknown(
    words: &[String],
    cache: &mut BTreeMap<Vec<String>, BTreeSet<String>>,
) -> Option<String> {
    let mut path: Vec<String> = Vec::new();
    for word in words {
        let children = subcommands(&path, cache);
        if children.is_empty() {
            return None; // leaf: remaining words are positional arguments
        }
        if word == "help" {
            return None;
        }
        if !children.contains(word) {
            return Some(word.clone());
        }
        path.push(word.clone());
    }
    None
}

#[test]
fn documented_commands_exist_in_the_cli() {
    let root = repo_root();
    let mut invocations = Vec::new();
    for file in markdown_files(&root) {
        // Historical ADRs record past decisions; skipped on purpose.
        if file.starts_with(root.join("docs/adr")) {
            continue;
        }
        scan_markdown(&file, &root, &mut invocations);
    }
    // The CLI's own `///` help comments quote sibling commands too.
    let main_rs = root.join("crates/openbnct-cli/src/main.rs");
    for (number, line) in std::fs::read_to_string(&main_rs)
        .unwrap()
        .lines()
        .enumerate()
    {
        if line.trim_start().starts_with("///") {
            for (i, span) in line.split('`').enumerate() {
                if i % 2 == 1 {
                    for words in words_after_binary(span) {
                        invocations.push(Invocation {
                            origin: format!("crates/openbnct-cli/src/main.rs:{}", number + 1),
                            words,
                        });
                    }
                }
            }
        }
    }
    assert!(
        invocations.len() > 50,
        "scanner found only {} invocations — extraction is broken",
        invocations.len()
    );
    let mut cache = BTreeMap::new();
    let mut bad = Vec::new();
    for invocation in &invocations {
        if let Some(word) = first_unknown(&invocation.words, &mut cache) {
            bad.push(format!(
                "{}: `openbnct {}` — no command `{word}`",
                invocation.origin,
                invocation.words.join(" ")
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "docs name commands the CLI does not have:\n{}",
        bad.join("\n")
    );
}

#[test]
fn help_text_does_not_advertise_a_report_command() {
    // Regression for `pk dose --help` once claiming a `report` consumer.
    let mut cache = BTreeMap::new();
    assert!(!subcommands(&[], &mut cache).contains("report"));
    let output = Command::new(env!("CARGO_BIN_EXE_openbnct"))
        .args(["pk", "dose", "--help"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(!text.contains("`report`"), "{text}");
}

#[test]
fn scanner_extracts_prefixed_invocations() {
    assert_eq!(
        words_after_binary("cargo run --bin openbnct -- dicom import-ct --series x"),
        vec![vec!["dicom".to_owned(), "import-ct".to_owned()]]
    );
    assert_eq!(
        words_after_binary("`openbnct pk dose`."),
        vec![vec!["pk".to_owned(), "dose".to_owned()]]
    );
}
