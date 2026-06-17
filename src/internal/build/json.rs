//! Parsing the streamed JSON diagnostics emitted by
//! `cargo build --message-format=json`.

use crate::internal::diagnostics::normalize::{self, Context, Variations};
use crate::internal::model::{Name, Test};
use crate::internal::path::CanonicalPath;
use crate::internal::project::Project;
use serde_derive::Deserialize;
use std::collections::{BTreeMap as Map, BTreeSet as Set};
use std::path::PathBuf;

/// One `--message-format=json` line emitted by cargo (a `compiler-message`).
#[derive(Deserialize)]
struct CargoMessage {
    /// Tag distinguishing the cargo message kind; drives serde's variant
    /// filtering but is never read directly.
    #[allow(
        dead_code,
        reason = "the reason tag drives serde's variant filtering but is never read directly"
    )]
    reason: Reason,
    /// The build target the message pertains to.
    target: RustcTarget,
    /// The rustc diagnostic payload.
    message: RustcMessage,
}

/// The `reason` tag values trybuild accepts; any other line fails to
/// deserialize and is skipped.
#[derive(Deserialize)]
enum Reason {
    /// `"compiler-message"` — a rustc diagnostic.
    #[serde(rename = "compiler-message")]
    CompilerMessage,
}

/// The build-target portion of a cargo message.
#[derive(Deserialize)]
struct RustcTarget {
    /// Absolute path to the target's root source file, used to attribute the
    /// diagnostic to a registered test case.
    src_path: PathBuf,
}

/// The rustc diagnostic portion of a cargo message.
#[derive(Deserialize)]
struct RustcMessage {
    /// The human-readable rendering of the diagnostic.
    rendered: String,
    /// The diagnostic level, e.g. `"error"`, `"warning"`, or `"failure-note"`.
    level: String,
}

/// The diagnostics parsed out of one cargo build, keyed by source path, plus any
/// non-message stdout cargo interleaved.
pub(in crate::internal) struct ParsedOutputs {
    /// Cargo stdout with the JSON diagnostic messages removed.
    pub stdout: String,
    /// The normalized stderr for each test source file cargo reported on.
    pub stderrs: Map<CanonicalPath, Stderr>,
}

/// The accumulated, normalized stderr for a single test source file.
pub(in crate::internal) struct Stderr {
    /// Whether the file compiled without an `error`-level diagnostic.
    pub success: bool,
    /// The normalized diagnostic variations for the file.
    pub stderr: Variations,
}

impl Default for Stderr {
    fn default() -> Self {
        Self {
            success: true,
            stderr: Variations::default(),
        }
    }
}

/// Splits cargo's stdout into JSON diagnostic messages and the surrounding
/// plain output, normalizing each diagnostic and grouping it by source file.
///
/// Only messages whose source path appears in `path_map` are kept; duplicate
/// messages are discarded, and an `error`-level diagnostic marks its file as
/// having failed to compile. Returns the normalized per-file stderrs together
/// with the leftover non-message stdout.
pub(in crate::internal) fn parse_cargo_json(
    project: &Project,
    stdout: &[u8],
    path_map: &Map<CanonicalPath, (&Name, &Test)>,
) -> ParsedOutputs {
    let mut map = Map::new();
    let mut nonmessage_stdout = String::new();
    let mut remaining = &*String::from_utf8_lossy(stdout);
    let mut seen = Set::new();
    while !remaining.is_empty() {
        let Some(begin) = remaining.find("{\"reason\":") else {
            break;
        };
        let Some((nonmessage, rest)) = remaining.split_at_checked(begin) else {
            break;
        };
        nonmessage_stdout.push_str(nonmessage);
        let len = rest
            .find('\n')
            .map_or(rest.len(), |end| end.saturating_add(1));
        let Some((message, tail)) = rest.split_at_checked(len) else {
            break;
        };
        remaining = tail;
        if !seen.insert(message) {
            // Discard duplicate messages. This might no longer be necessary
            // after https://github.com/rust-lang/rust/issues/106571 is fixed.
            // Normally rustc would filter duplicates itself and I think this is
            // a short-lived bug.
            continue;
        }
        if let Ok(de) = serde_json::from_str::<CargoMessage>(message)
            && de.message.level != "failure-note"
        {
            let src_path = CanonicalPath::new(&de.target.src_path);
            let Some(&(name, case)) = path_map.get(&src_path) else {
                continue;
            };
            let entry = map.entry(src_path).or_insert_with(Stderr::default);
            if de.message.level == "error" {
                entry.success = false;
            }
            let normalized = normalize::diagnostics(
                &de.message.rendered,
                &Context {
                    krate: &name.0,
                    source_dir: &project.source_dir,
                    workspace: &project.workspace,
                    input_file: &case.path,
                    target_dir: &project.target_dir,
                    path_dependencies: &project.path_dependencies,
                },
            );
            entry.stderr.concat(&normalized);
        }
    }
    nonmessage_stdout.push_str(remaining);
    ParsedOutputs {
        stdout: nonmessage_stdout,
        stderrs: map,
    }
}
