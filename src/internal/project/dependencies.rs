//! Reading the crate-under-test's manifest — and its workspace's `[workspace]`,
//! `[patch]`, and `[replace]` sections — and rewriting relative path
//! dependencies so the generated project resolves them from its own location.
#![allow(
  clippy::same_name_method,
  reason = "serde's `remote = \"Self\"` makes the `Dependency` derive generate inherent `serialize`/`deserialize` fns that the \
            hand-written `Serialize`/`Deserialize` impls delegate to; same_name_method targets that derive-generated inherent impl, which \
            cannot carry its own attribute, so it is suppressed at module scope"
)]

use std::collections::BTreeMap as Map;
use std::fmt;
use std::fs;
use std::path::PathBuf;

use serde::de::Deserialize;
use serde::de::Deserializer;
use serde::de::Visitor;
use serde::de::value::MapAccessDeserializer;
use serde::de::value::StrDeserializer;
use serde::de::{
  self,
};
use serde::ser::Serialize;
use serde::ser::Serializer;
use serde_derive::Deserialize;
use serde_derive::Serialize;
use serde_json::Value;

use crate::internal::error;
use crate::internal::project::ProjectError;
use crate::internal::project::Result as ProjectResult;
use crate::internal::project::inherit::InheritEdition;
use crate::internal::project::manifest::Edition;
use crate::internal::sys::SysError;
use crate::internal::sys::directory::Directory;

/// Reads the crate-under-test's `Cargo.toml`, rewriting relative path
/// dependencies to absolute paths and dropping the `trybuild` self-dependency.
#[allow(
  clippy::single_call_fn,
  reason = "the crate-under-test manifest reader on this module's surface to the runner, a named project-synthesis step"
)]
pub(in crate::internal) fn get_manifest(manifest_dir: &Directory) -> ProjectResult<Manifest> {
  let cargo_toml_path = manifest_dir.join("Cargo.toml");
  let mut manifest = (|| -> error::Result<Manifest> {
    let manifest_str = fs::read_to_string(&cargo_toml_path).map_err(SysError::Io)?;
    let manifest: Manifest = toml::from_str(&manifest_str).map_err(ProjectError::TomlDe)?;
    Ok(manifest)
  })()
  .map_err(|err| ProjectError::GetManifest(cargo_toml_path, Box::new(err)))?;

  fix_dependencies(&mut manifest.dependencies, manifest_dir);
  fix_dependencies(&mut manifest.dev_dependencies, manifest_dir);
  for target in manifest.target.values_mut() {
    fix_dependencies(&mut target.dependencies, manifest_dir);
    fix_dependencies(&mut target.dev_dependencies, manifest_dir);
  }

  Ok(manifest)
}

/// Reads the workspace manifest, returning an empty default if it cannot be
/// read (for instance when there is no enclosing workspace).
#[allow(
  clippy::single_call_fn,
  reason = "the infallible workspace-manifest reader on this module's surface, wrapping the fallible try_ variant with unwrap_or_default"
)]
pub(in crate::internal) fn get_workspace_manifest(manifest_dir: &Directory) -> WorkspaceManifest {
  try_get_workspace_manifest(manifest_dir).unwrap_or_default()
}

/// Reads the workspace's `[workspace]`, `[patch]`, and `[replace]` sections,
/// rewriting their relative paths to absolute and dropping any `trybuild` entry.
#[allow(
  clippy::single_call_fn,
  reason = "the fallible workspace-manifest reader, named to pair with get_workspace_manifest's unwrap_or_default and isolate the \
            `?`-laden body"
)]
pub(in crate::internal) fn try_get_workspace_manifest(manifest_dir: &Directory) -> error::Result<WorkspaceManifest> {
  let cargo_toml_path = manifest_dir.join("Cargo.toml");
  let manifest_str = fs::read_to_string(cargo_toml_path).map_err(SysError::Io)?;
  let mut manifest: WorkspaceManifest = toml::from_str(&manifest_str).map_err(ProjectError::TomlDe)?;

  fix_dependencies(&mut manifest.workspace.dependencies, manifest_dir);
  fix_patches(&mut manifest.patch, manifest_dir);
  fix_replacements(&mut manifest.replace, manifest_dir);

  Ok(manifest)
}

/// Drops the `trybuild` dependency and rewrites each remaining dependency's
/// relative `path` to be absolute against `dir`.
fn fix_dependencies(dependencies: &mut Map<String, Dependency>, dir: &Directory) {
  let _removed = dependencies.remove("trybuild");
  for dep in dependencies.values_mut() {
    dep.path = dep.path.as_ref().map(|path| Directory::new(dir.join(path)));
  }
}

/// Drops any `trybuild` patch and rewrites each remaining patch's relative path
/// to be absolute against `dir`.
#[allow(
  clippy::single_call_fn,
  reason = "a documented path-rewriting helper kept symmetric with fix_dependencies and fix_replacements so the three \
            `[patch]`/`[replace]`/dep fixups read alike"
)]
fn fix_patches(patches: &mut Map<String, RegistryPatch>, dir: &Directory) {
  for registry in patches.values_mut() {
    let _removed = registry.crates.remove("trybuild");
    for patch in registry.crates.values_mut() {
      patch.path = patch.path.as_ref().map(|path| dir.join(path));
    }
  }
}

/// Drops any `trybuild` replacement and rewrites each remaining replacement's
/// relative path to be absolute against `dir`.
#[allow(
  clippy::single_call_fn,
  reason = "a documented path-rewriting helper kept symmetric with fix_dependencies and fix_patches so the three \
            `[patch]`/`[replace]`/dep fixups read alike"
)]
fn fix_replacements(replacements: &mut Map<String, Patch>, dir: &Directory) {
  let _removed = replacements.remove("trybuild");
  for replacement in replacements.values_mut() {
    replacement.path = replacement.path.as_ref().map(|path| dir.join(path));
  }
}

/// The workspace-root sections trybuild copies into the generated project.
#[derive(Deserialize, Default, Debug)]
pub(in crate::internal) struct WorkspaceManifest {
  /// The `[workspace]` table.
  #[serde(default)]
  pub workspace: WorkspaceWorkspace,
  /// The `[patch]` tables, keyed by registry.
  #[serde(default)]
  pub patch:     Map<String, RegistryPatch>,
  /// The `[replace]` table.
  #[serde(default)]
  pub replace:   Map<String, Patch>,
}

/// The `[workspace]` table, reduced to the parts trybuild inherits.
#[derive(Deserialize, Default, Debug)]
pub(in crate::internal) struct WorkspaceWorkspace {
  /// The `[workspace.package]` defaults.
  #[serde(default)]
  pub package:      WorkspacePackage,
  /// The `[workspace.dependencies]` table.
  #[serde(default)]
  pub dependencies: Map<String, Dependency>,
}

/// The `[workspace.package]` defaults trybuild may inherit.
#[derive(Deserialize, Default, Debug)]
pub(in crate::internal) struct WorkspacePackage {
  /// The workspace's default edition, inherited via `edition.workspace = true`.
  pub edition: Option<Edition>,
}

/// The crate-under-test's `Cargo.toml`, parsed into the fields trybuild needs.
#[derive(Deserialize, Default, Debug)]
pub(in crate::internal) struct Manifest {
  /// Unstable `cargo-features` declared by the manifest.
  #[serde(rename = "cargo-features", default)]
  pub cargo_features:   Vec<String>,
  /// The `[package]` table.
  #[serde(default)]
  pub package:          Package,
  /// The `[features]` table.
  #[serde(default)]
  pub features:         Map<String, Vec<String>>,
  /// The `[dependencies]` table.
  #[serde(default)]
  pub dependencies:     Map<String, Dependency>,
  /// The `[dev-dependencies]` table.
  #[serde(default, alias = "dev-dependencies")]
  pub dev_dependencies: Map<String, Dependency>,
  /// The `[target.*]` dependency tables.
  #[serde(default)]
  pub target:           Map<String, TargetDependencies>,
}

/// The parsed `[package]` table of the crate under test.
#[derive(Deserialize, Default, Debug)]
pub(in crate::internal) struct Package {
  /// The package name.
  pub name:     String,
  /// The edition, possibly inherited from the workspace.
  #[serde(default)]
  pub edition:  EditionOrInherit,
  /// The dependency resolver version, if specified.
  pub resolver: Option<String>,
}

/// A package edition that is either given directly or inherited from the
/// workspace via `edition.workspace = true`.
#[derive(Debug)]
pub(in crate::internal) enum EditionOrInherit {
  /// An edition specified directly.
  Edition(Edition),
  /// `edition.workspace = true`; resolved from the workspace manifest.
  Inherit,
}

/// A single dependency entry, accepting both the `"1.2.3"` shorthand and the
/// `{ version = "1.2.3", ... }` table form.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(remote = "Self")]
pub(in crate::internal) struct Dependency {
  /// The version requirement.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub version:          Option<String>,
  /// A path dependency's location.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub path:             Option<Directory>,
  /// Whether the dependency is optional.
  #[serde(default, skip_serializing_if = "is_false")]
  pub optional:         bool,
  /// Whether the dependency's default features are enabled.
  #[serde(rename = "default-features", skip_serializing_if = "Option::is_none")]
  pub default_features: Option<bool>,
  /// The enabled features.
  #[serde(default, skip_serializing_if = "Vec::is_empty")]
  pub features:         Vec<String>,
  /// A git dependency's repository URL.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub git:              Option<String>,
  /// The git branch to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub branch:           Option<String>,
  /// The git tag to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tag:              Option<String>,
  /// The git revision to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rev:              Option<String>,
  /// Whether the dependency is inherited from the workspace.
  #[serde(default, skip_serializing_if = "is_false")]
  pub workspace:        bool,
  /// Any other keys, preserved verbatim for re-serialization.
  #[serde(flatten)]
  pub rest:             Map<String, Value>,
}

/// The dependency tables under a `[target.'cfg(...)']` section.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(in crate::internal) struct TargetDependencies {
  /// The target's `[dependencies]`.
  #[serde(default, skip_serializing_if = "Map::is_empty")]
  pub dependencies:     Map<String, Dependency>,
  /// The target's `[dev-dependencies]`.
  #[serde(default, alias = "dev-dependencies", skip_serializing_if = "Map::is_empty")]
  pub dev_dependencies: Map<String, Dependency>,
}

/// The crates patched for one registry in a `[patch.<registry>]` table.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(transparent)]
pub(in crate::internal) struct RegistryPatch {
  /// The per-crate patch entries.
  pub crates: Map<String, Patch>,
}

/// A single `[patch]` or `[replace]` entry.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(in crate::internal) struct Patch {
  /// A path override's location.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub path:   Option<PathBuf>,
  /// A git override's repository URL.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub git:    Option<String>,
  /// The git branch to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub branch: Option<String>,
  /// The git tag to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tag:    Option<String>,
  /// The git revision to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rev:    Option<String>,
  /// Any other keys, preserved verbatim for re-serialization.
  #[serde(flatten)]
  pub rest:   Map<String, Value>,
}

/// serde `skip_serializing_if` predicate: whether a boolean is `false`.
#[allow(
  clippy::trivially_copy_pass_by_ref,
  reason = "serde invokes a `skip_serializing_if` predicate as `fn(&T) -> bool`; the `&bool` receiver is mandated by that call signature, \
            not a missed by-value optimization"
)]
const fn is_false(boolean: &bool) -> bool {
  !*boolean
}

impl Default for EditionOrInherit {
  fn default() -> Self {
    Self::Edition(Edition::default())
  }
}

impl<'de> Deserialize<'de> for EditionOrInherit {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    struct EditionOrInheritVisitor;

    impl<'de> Visitor<'de> for EditionOrInheritVisitor {
      type Value = EditionOrInherit;

      fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("edition")
      }

      fn visit_str<E>(self, string: &str) -> Result<Self::Value, E>
      where
        E: de::Error,
      {
        Edition::deserialize(StrDeserializer::new(string)).map(EditionOrInherit::Edition)
      }

      fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
      where
        M: de::MapAccess<'de>,
      {
        // Deserialize only to validate the `workspace = true` shape; the
        // value itself carries no data we need to keep.
        let _validated = InheritEdition::deserialize(MapAccessDeserializer::new(map))?;
        Ok(EditionOrInherit::Inherit)
      }
    }

    deserializer.deserialize_any(EditionOrInheritVisitor)
  }
}

impl Serialize for Dependency {
  fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
  where
    S: Serializer,
  {
    Self::serialize(self, serializer)
  }
}

impl<'de> Deserialize<'de> for Dependency {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    struct DependencyVisitor;

    impl<'de> Visitor<'de> for DependencyVisitor {
      type Value = Dependency;

      fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a version string like \"0.9.8\" or a dependency like { version = \"0.9.8\" }")
      }

      fn visit_str<E>(self, string: &str) -> Result<Self::Value, E>
      where
        E: de::Error,
      {
        Ok(Dependency {
          version:          Some(string.to_owned()),
          path:             None,
          optional:         false,
          default_features: Some(true),
          features:         Vec::new(),
          git:              None,
          branch:           None,
          tag:              None,
          rev:              None,
          workspace:        false,
          rest:             Map::new(),
        })
      }

      fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
      where
        M: de::MapAccess<'de>,
      {
        Dependency::deserialize(MapAccessDeserializer::new(map))
      }
    }

    deserializer.deserialize_any(DependencyVisitor)
  }
}
