//! The synthesized `Cargo.toml` for the generated test project: the typed shape
//! that is serialized to disk, as opposed to the parsed manifest of the crate
//! under test in the sibling `dependencies` module.

use std::collections::BTreeMap as Map;
use std::path::PathBuf;

use serde::ser::SerializeMap as _;
use serde::ser::Serializer;
use serde_derive::Deserialize;
use serde_derive::Serialize;

use crate::internal::model::Name;
use crate::internal::project::dependencies::Dependency;
use crate::internal::project::dependencies::Patch;
use crate::internal::project::dependencies::RegistryPatch;
use crate::internal::project::dependencies::TargetDependencies;

/// The generated project's `Cargo.toml`, ready to serialize to disk.
#[derive(Serialize, Debug)]
pub(in crate::internal) struct Manifest {
  /// Unstable `cargo-features` forwarded from the crate under test.
  #[serde(rename = "cargo-features", skip_serializing_if = "Vec::is_empty")]
  pub cargo_features: Vec<String>,
  /// The `[package]` table.
  pub package:        Package,
  /// The `[features]` table.
  #[serde(skip_serializing_if = "Map::is_empty")]
  pub features:       Map<String, Vec<String>>,
  /// The merged `[dependencies]` (deps, dev-deps, and the crate-under-test path).
  pub dependencies:   Map<String, Dependency>,
  /// The `[target.*]` dependency tables.
  #[serde(skip_serializing_if = "Map::is_empty")]
  pub target:         Map<String, TargetDependencies>,
  /// One `[[bin]]` per test file, plus the placeholder `main.rs` bin.
  #[serde(rename = "bin")]
  pub bins:           Vec<Bin>,
  /// A nested `[workspace]` making the generated project self-contained and
  /// carrying inherited workspace dependencies.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub workspace:      Option<Workspace>,
  /// The `[patch]` tables copied from the workspace root.
  #[serde(serialize_with = "serialize_patch", skip_serializing_if = "empty_patch")]
  pub patch:          Map<String, RegistryPatch>,
  /// The `[replace]` table copied from the workspace root.
  #[serde(skip_serializing_if = "Map::is_empty")]
  pub replace:        Map<String, Patch>,
}

/// The generated project's `[package]` table.
#[derive(Serialize, Debug)]
pub(in crate::internal) struct Package {
  /// The package name, `<crate>-tests`.
  pub name:     String,
  /// A fixed placeholder version.
  pub version:  String,
  /// The edition, resolved from the crate under test (and its workspace).
  pub edition:  Edition,
  /// The dependency resolver version, forwarded from the crate under test.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub resolver: Option<String>,
  /// Always `false`; the generated project is never published.
  pub publish:  bool,
}

/// A Rust edition, serialized as its year string.
#[derive(Serialize, Deserialize, Default, Debug)]
pub(in crate::internal) enum Edition {
  /// Edition 2015 (the default).
  #[default]
  #[serde(rename = "2015")]
  E2015,
  /// Edition 2018.
  #[serde(rename = "2018")]
  E2018,
  /// Edition 2021.
  #[serde(rename = "2021")]
  E2021,
  /// Edition 2024.
  #[serde(rename = "2024")]
  E2024,
}

/// One `[[bin]]` target: a generated name pointing at a test source file.
#[derive(Serialize, Debug)]
pub(in crate::internal) struct Bin {
  /// The generated bin name, e.g. `trybuild007`.
  pub name: Name,
  /// The source file the bin compiles.
  pub path: PathBuf,
}

/// The nested `[workspace]` table embedded in the generated manifest.
#[derive(Serialize, Debug)]
pub(in crate::internal) struct Workspace {
  /// `[workspace.dependencies]` inherited from the real workspace.
  #[serde(skip_serializing_if = "Map::is_empty")]
  pub dependencies: Map<String, Dependency>,
}

/// Serializes the `[patch]` map, skipping registries whose crate set is empty.
#[allow(
  clippy::single_call_fn,
  reason = "a serde `serialize_with` target referenced by name from the #[serde] attribute on `Manifest::patch`, not a hand-called helper"
)]
fn serialize_patch<S>(patches: &Map<String, RegistryPatch>, serializer: S) -> Result<S::Ok, S::Error>
where
  S: Serializer,
{
  let mut map = serializer.serialize_map(None)?;
  for (registry, patch) in patches {
    if !patch.crates.is_empty() {
      map.serialize_entry(registry, patch)?;
    }
  }
  map.end()
}

/// serde `skip_serializing_if` predicate: whether every registry's patch set is
/// empty.
fn empty_patch(patch: &Map<String, RegistryPatch>) -> bool {
  patch.values().all(|registry_patch| registry_patch.crates.is_empty())
}

#[cfg(test)]
mod tests {
  use strict_test_support::TestFailure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_ok_source;

  use super::*;
  use crate::internal::project::dependencies::GitSource;

  #[test]
  fn patch_serialization_skips_only_empty_registries() -> Result<(), TestFailure> {
    let mut manifest = Manifest {
      cargo_features: Vec::new(),
      package:        Package {
        name:     "demo-tests".to_owned(),
        version:  "0.0.0".to_owned(),
        edition:  Edition::E2024,
        resolver: None,
        publish:  false,
      },
      features:       Map::new(),
      dependencies:   Map::new(),
      target:         Map::new(),
      bins:           vec![Bin {
        name: Name("trybuild000".to_owned()),
        path: PathBuf::from("tests/ui/case.rs"),
      }],
      workspace:      None,
      patch:          Map::new(),
      replace:        Map::new(),
    };

    let empty_rendered = ensure_ok_source(toml::to_string(&manifest), "manifest without patches serializes")?;
    let mut crates = Map::new();
    let replaced_crate = crates.insert("patched".to_owned(), Patch {
      path: Some(PathBuf::from("../patched")),
      git:  GitSource::default(),
      rest: Map::new(),
    });
    let mut patch = Map::new();
    let replaced_empty_registry = patch.insert("empty".to_owned(), RegistryPatch {
      crates: Map::new()
    });
    let replaced_crates_io_registry = patch.insert("crates-io".to_owned(), RegistryPatch {
      crates,
    });
    manifest.patch = patch;

    let non_empty_patch = empty_patch(&manifest.patch);
    let patched_rendered = ensure_ok_source(toml::to_string(&manifest), "manifest with patches serializes")?;

    ensure_all(&[
      (replaced_crate.is_none(), "the patched crate is inserted once"),
      (replaced_empty_registry.is_none(), "the empty patch registry is inserted once"),
      (
        replaced_crates_io_registry.is_none(),
        "the populated patch registry is inserted once",
      ),
      (empty_patch(&Map::new()), "an absent patch table is empty"),
      (!non_empty_patch, "a registry with patched crates makes the patch table non-empty"),
      (!empty_rendered.contains("[patch]"), "empty patch tables are omitted"),
      (
        patched_rendered.contains("[patch.crates-io.patched]"),
        "non-empty patch registries are serialized",
      ),
      (patched_rendered.contains("path = \"../patched\""), "patch entries keep their paths"),
      (
        !patched_rendered.contains("[patch.empty]"),
        "empty patch registries are skipped during serialization",
      ),
    ])
  }
}
