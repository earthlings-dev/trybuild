//! Parsing the streamed JSON diagnostics emitted by
//! `cargo build --message-format=json`.

use std::collections::BTreeMap as Map;
use std::collections::BTreeSet as Set;
use std::path::PathBuf;

use serde_derive::Deserialize;

use crate::internal::diagnostics::normalize;
use crate::internal::diagnostics::normalize::Context;
use crate::internal::diagnostics::normalize::Variations;
use crate::internal::model::Name;
use crate::internal::model::Test;
use crate::internal::path::CanonicalPath;
use crate::internal::project::Project;

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
  target:  RustcTarget,
  /// The rustc diagnostic payload.
  message: RustcMessage,
}

/// The cargo payload carrying one compiler artifact.
#[derive(Deserialize)]
struct CompilerArtifact {
  /// The build target the artifact pertains to.
  target:     RustcTarget,
  /// Path to the executable cargo produced, if this artifact is runnable.
  executable: Option<PathBuf>,
}

/// The build-target portion of a cargo message.
#[derive(Deserialize)]
struct RustcTarget {
  /// Cargo target name, used to distinguish registered test bins from
  /// unrelated artifacts in the generated project.
  name:     String,
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
  level:    String,
}

/// The outputs parsed out of one cargo build, keyed by source path, plus any
/// non-message stdout cargo interleaved.
pub(in crate::internal) struct ParsedOutputs {
  /// Cargo stdout with the JSON diagnostic messages removed.
  pub stdout:      String,
  /// The normalized stderr for each test source file cargo reported on.
  pub stderrs:     Map<CanonicalPath, Stderr>,
  /// The executable produced for each successfully built test source file.
  pub executables: Map<CanonicalPath, PathBuf>,
}

/// The accumulated, normalized stderr for a single test source file.
pub(in crate::internal) struct Stderr {
  /// Whether the file compiled without an `error`-level diagnostic.
  pub success: bool,
  /// The normalized diagnostic variations for the file.
  pub stderr:  Variations,
}

impl Default for Stderr {
  fn default() -> Self {
    Self {
      success: true,
      stderr:  Variations::default(),
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
    let len = rest.find('\n').map_or(rest.len(), |end| end.saturating_add(1));
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
  reason = "compiler-message attribution is a distinct Cargo JSON interpretation rule with target identity and diagnostic normalization"
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
  let normalized = normalize::diagnostics(&cargo_message.message.rendered, &Context {
    krate:             &name.0,
    source_dir:        &project.source_dir,
    workspace:         &project.workspace,
    input_file:        &case.path,
    target_dir:        &project.target_dir,
    path_dependencies: &project.path_dependencies,
  });
  entry.stderr.concat(&normalized);
}

/// Records the executable path from a compiler artifact when it belongs to a
/// registered test target.
#[allow(
  clippy::single_call_fn,
  reason = "compiler-artifact attribution is a distinct Cargo JSON interpretation rule that records executables only for registered \
            targets"
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

#[cfg(test)]
mod tests {
  use std::collections::BTreeMap as Map;
  use std::fs;
  use std::path::Path;
  use std::path::PathBuf;
  use std::result::Result as StdResult;

  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  use super::*;
  use crate::internal::model::Expected;
  use crate::internal::project::KeepGoing;
  use crate::internal::project::Selected;
  use crate::internal::project::manifest::Edition;
  use crate::internal::project::manifest::Manifest;
  use crate::internal::project::manifest::Package;
  use crate::internal::sys::directory::Directory;
  use crate::internal::sys::env::Update;

  fn project(fixture: &TempDir) -> Project {
    Project {
      dir:               Directory::new(fixture.child("generated")),
      source_dir:        Directory::new(fixture.child("source")),
      target_dir:        Directory::new(fixture.child("target")),
      name:              "demo-tests".to_owned(),
      update:            Update::Verify,
      selected:          Selected::Both,
      features:          None,
      workspace:         Directory::new(fixture.path()),
      path_dependencies: Vec::new(),
      manifest:          Manifest {
        cargo_features: Vec::new(),
        package:        Package {
          name:     "demo-tests".to_owned(),
          version:  "0.0.0".to_owned(),
          edition:  Edition::default(),
          resolver: None,
          publish:  false,
        },
        features:       Map::new(),
        dependencies:   Map::new(),
        target:         Map::new(),
        bins:           Vec::new(),
        workspace:      None,
        patch:          Map::new(),
        replace:        Map::new(),
      },
      keep_going:        KeepGoing::No,
    }
  }

  fn path_map<'a>(name: &'a Name, case: &'a Test, source: &Path) -> Map<CanonicalPath, (&'a Name, &'a Test)> {
    let mut map = Map::new();
    let _previous = map.insert(CanonicalPath::new(source), (name, case));
    map
  }

  fn compiler_message(target: &str, source: &Path, level: &str, rendered: &str) -> String {
    let target_json = serde_json::Value::String(target.to_owned());
    let source_json = serde_json::Value::String(source.to_string_lossy().into_owned());
    let level_json = serde_json::Value::String(level.to_owned());
    let rendered_json = serde_json::Value::String(rendered.to_owned());
    format!(
      r#"{{"reason":"compiler-message","target":{{"name":{target_json},"src_path":{source_json}}},"message":{{"rendered":{rendered_json},"level":{level_json}}}}}"#
    )
  }

  fn compiler_artifact(target: &str, source: &Path, executable: Option<&Path>) -> String {
    let target_json = serde_json::Value::String(target.to_owned());
    let source_json = serde_json::Value::String(source.to_string_lossy().into_owned());
    let executable_json = executable.map_or(serde_json::Value::Null, |path| {
      serde_json::Value::String(path.to_string_lossy().into_owned())
    });
    format!(r#"{{"reason":"compiler-artifact","target":{{"name":{target_json},"src_path":{source_json}}},"executable":{executable_json}}}"#)
  }

  #[test]
  fn parse_cargo_json_records_matching_diagnostics_and_executables() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("cargo-json-records")?;
    let project = project(&fixture);
    let source = fixture.child("source/case.rs");
    ensure_ok_source(
      fs::create_dir_all(source.parent().unwrap_or_else(|| Path::new("."))),
      "source dir can be created",
    )?;
    ensure_ok_source(fs::write(&source, "fn main() {}\n"), "source file can be written")?;
    let executable = fixture.child("target/debug/trybuild000");
    let name = Name("trybuild000".to_owned());
    let case = Test {
      path:     PathBuf::from("case.rs"),
      expected: Expected::CompileFail,
    };
    let warning = compiler_message(&name.0, &source, "warning", "warning: caution\n");
    let error = compiler_message(&name.0, &source, "error", "error: broken\n");
    let artifact = compiler_artifact(&name.0, &source, Some(&executable));
    let stdout = format!("prelude\n{warning}\n{warning}\n{error}\n{artifact}\ntrailer\n");

    let parsed = parse_cargo_json(&project, stdout.as_bytes(), &path_map(&name, &case, &source));
    let stderr = ensure_some(
      parsed.stderrs.get(&CanonicalPath::new(&source)),
      "matching diagnostics are recorded",
    )?;
    let executable_path = ensure_some(
      parsed.executables.get(&CanonicalPath::new(&source)),
      "matching compiler artifacts record their executable",
    )?;

    ensure_all(&[
      (parsed.stdout == "prelude\ntrailer\n", "non-message cargo stdout is preserved"),
      (!stderr.success, "an error-level diagnostic marks the test as failed"),
      (
        stderr.stderr.preferred().contains("warning: caution"),
        "warning diagnostics are normalized into stderr",
      ),
      (
        stderr.stderr.preferred().contains("error: broken"),
        "error diagnostics are normalized into stderr",
      ),
      (executable_path == &executable, "matching artifact executable paths are retained"),
    ])
  }

  #[test]
  fn parse_cargo_json_discards_unrelated_messages_and_artifacts() -> StdResult<(), TestFailure> {
    let fixture = TempDir::new("cargo-json-discards")?;
    let project = project(&fixture);
    let source = fixture.child("source/case.rs");
    let other = fixture.child("source/other.rs");
    ensure_ok_source(
      fs::create_dir_all(source.parent().unwrap_or_else(|| Path::new("."))),
      "source dir can be created",
    )?;
    ensure_ok_source(fs::write(&source, "fn main() {}\n"), "source file can be written")?;
    ensure_ok_source(fs::write(&other, "fn main() {}\n"), "other source file can be written")?;
    let name = Name("trybuild000".to_owned());
    let case = Test {
      path:     PathBuf::from("case.rs"),
      expected: Expected::CompileFail,
    };
    let failure_note = compiler_message(&name.0, &source, "failure-note", "error: skipped\n");
    let wrong_source = compiler_message(&name.0, &other, "error", "error: skipped\n");
    let wrong_target = compiler_message("trybuild999", &source, "error", "error: skipped\n");
    let missing_executable = compiler_artifact(&name.0, &source, None);
    let wrong_artifact_source = compiler_artifact(&name.0, &other, Some(&fixture.child("other-bin")));
    let wrong_artifact_target = compiler_artifact("trybuild999", &source, Some(&fixture.child("wrong-bin")));
    let unknown = r#"{"reason":"build-finished","success":true}"#;
    let stdout = format!(
      "{failure_note}\n{wrong_source}\n{wrong_target}\n{missing_executable}\n{wrong_artifact_source}\n{wrong_artifact_target}\n{unknown}\n"
    );

    let parsed = parse_cargo_json(&project, stdout.as_bytes(), &path_map(&name, &case, &source));

    ensure_all(&[
      (parsed.stdout.is_empty(), "discarded JSON messages do not leak into cargo stdout"),
      (parsed.stderrs.is_empty(), "unrelated diagnostics are discarded"),
      (parsed.executables.is_empty(), "unrelated artifacts are discarded"),
    ])
  }
}
