//! Parsing the streamed JSON diagnostics emitted by
//! `cargo build --message-format=json`.

use crate::internal::diagnostics::normalize::{self, Context, Variations};
use crate::internal::model::{Name, Test};
use crate::internal::path::CanonicalPath;
use crate::internal::project::Project;
use serde_derive::Deserialize;
use std::collections::{BTreeMap as Map, BTreeSet as Set};
use std::path::PathBuf;

/// One `--message-format=json` line emitted by cargo.
#[derive(Deserialize)]
#[serde(tag = "reason")]
enum CargoMessage {
    /// `"compiler-message"` — a rustc diagnostic.
    #[serde(rename = "compiler-message")]
    CompilerMessage(CompilerMessage),
    /// `"compiler-artifact"` — a built artifact path.
    #[serde(rename = "compiler-artifact")]
    CompilerArtifact(CompilerArtifact),
}

/// The cargo payload carrying one rustc diagnostic.
#[derive(Deserialize)]
struct CompilerMessage {
    /// The build target the message pertains to.
    target: RustcTarget,
    /// The rustc diagnostic payload.
    message: RustcMessage,
}

/// The cargo payload carrying one compiler artifact.
#[derive(Deserialize)]
struct CompilerArtifact {
    /// The build target the artifact pertains to.
    target: RustcTarget,
    /// Path to the executable cargo produced, if this artifact is runnable.
    executable: Option<PathBuf>,
}

/// The build-target portion of a cargo message.
#[derive(Deserialize)]
struct RustcTarget {
    /// Cargo target name, used to distinguish registered test bins from
    /// unrelated artifacts in the generated project.
    name: String,
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

/// The outputs parsed out of one cargo build, keyed by source path, plus any
/// non-message stdout cargo interleaved.
pub(in crate::internal) struct ParsedOutputs {
    /// Cargo stdout with the JSON diagnostic messages removed.
    pub stdout: String,
    /// The normalized stderr for each test source file cargo reported on.
    pub stderrs: Map<CanonicalPath, Stderr>,
    /// The executable produced for each successfully built test source file.
    pub executables: Map<CanonicalPath, PathBuf>,
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
/// having failed to compile. Executable artifact paths for registered test
/// bins are kept beside the diagnostics. Returns the normalized per-file
/// stderrs and executable paths together with the leftover non-message stdout.
pub(in crate::internal) fn parse_cargo_json(
    project: &Project,
    stdout: &[u8],
    path_map: &Map<CanonicalPath, (&Name, &Test)>,
) -> ParsedOutputs {
    let mut stderrs = Map::new();
    let mut executables = Map::new();
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
        let Some((line, tail)) = rest.split_at_checked(len) else {
            break;
        };
        remaining = tail;
        if !seen.insert(line) {
            // Discard duplicate messages. This might no longer be necessary
            // after https://github.com/rust-lang/rust/issues/106571 is fixed.
            // Normally rustc would filter duplicates itself and I think this is
            // a short-lived bug.
            continue;
        }
        if let Ok(de) = serde_json::from_str::<CargoMessage>(line) {
            match de {
                CargoMessage::CompilerMessage(diagnostic) => {
                    record_diagnostic(project, path_map, &mut stderrs, &diagnostic);
                }
                CargoMessage::CompilerArtifact(artifact) => {
                    record_executable(path_map, &mut executables, artifact);
                }
            }
        }
    }
    nonmessage_stdout.push_str(remaining);
    ParsedOutputs {
        stdout: nonmessage_stdout,
        stderrs,
        executables,
    }
}

/// Records one rustc diagnostic when cargo attributed it to a registered test
/// target.
#[allow(
    clippy::single_call_fn,
    reason = "separates diagnostic parsing from artifact parsing so parse_cargo_json stays readable"
)]
fn record_diagnostic(
    project: &Project,
    path_map: &Map<CanonicalPath, (&Name, &Test)>,
    stderrs: &mut Map<CanonicalPath, Stderr>,
    cargo_message: &CompilerMessage,
) {
    if cargo_message.message.level == "failure-note" {
        return;
    }

    let src_path = CanonicalPath::new(&cargo_message.target.src_path);
    let Some(&(name, case)) = path_map.get(&src_path) else {
        return;
    };
    if cargo_message.target.name.as_str() != name.0.as_str() {
        return;
    }

    let entry = stderrs.entry(src_path).or_default();
    if cargo_message.message.level == "error" {
        entry.success = false;
    }
    let normalized = normalize::diagnostics(
        &cargo_message.message.rendered,
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

/// Records the executable path from a compiler artifact when it belongs to a
/// registered test target.
#[allow(
    clippy::single_call_fn,
    reason = "separates artifact parsing from diagnostic parsing so parse_cargo_json stays readable"
)]
fn record_executable(
    path_map: &Map<CanonicalPath, (&Name, &Test)>,
    executables: &mut Map<CanonicalPath, PathBuf>,
    cargo_artifact: CompilerArtifact,
) {
    let Some(executable) = cargo_artifact.executable else {
        return;
    };
    let src_path = CanonicalPath::new(&cargo_artifact.target.src_path);
    let Some(&(name, _case)) = path_map.get(&src_path) else {
        return;
    };
    if cargo_artifact.target.name != name.0.as_str() {
        return;
    }
    let _previous = executables.insert(src_path, executable);
}
