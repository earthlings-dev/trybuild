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

use serde::de;
use serde::de::Deserialize;
use serde::de::Deserializer;
use serde::de::Visitor;
use serde::de::value::MapAccessDeserializer;
use serde::de::value::StrDeserializer;
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
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub(in crate::internal) struct GitSource {
  /// The git repository URL, kept under the manifest's `git` key.
  #[serde(rename = "git", skip_serializing_if = "Option::is_none")]
  pub repository: Option<String>,
  /// The git branch to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub branch:     Option<String>,
  /// The git tag to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub tag:        Option<String>,
  /// The git revision to use.
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rev:        Option<String>,
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
  /// A git dependency's source coordinates.
  #[serde(flatten)]
  pub git:              GitSource,
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
  pub path: Option<PathBuf>,
  /// A git override's source coordinates.
  #[serde(flatten)]
  pub git:  GitSource,
  /// Any other keys, preserved verbatim for re-serialization.
  #[serde(flatten)]
  pub rest: Map<String, Value>,
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
          git:              GitSource::default(),
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

#[cfg(test)]
mod tests {
  use std::fs;

  use serde::de::DeserializeOwned;
  use strict_test_support::TempDir;
  use strict_test_support::TestFailure;
  use strict_test_support::ensure;
  use strict_test_support::ensure_all;
  use strict_test_support::ensure_eq;
  use strict_test_support::ensure_ok_source;
  use strict_test_support::ensure_some;

  use super::*;

  const GIT_BRANCH_DEP: &str = r#"[dep]
git = "https://example.com/repo.git"
branch = "main"
"#;
  const UNKNOWN_KEY_DEP: &str = r#"[dep]
git = "https://example.com/repo.git"
package = "actual"
"#;
  const VERSION_DEP: &str = r#"[dep]
version = "1.0.0"
"#;
  const GIT_REV_PATCH: &str = r#"[dep]
git = "https://example.com/repo.git"
rev = "abc123"
"#;

  fn manifest_fixture(name: &'static str, manifest: &str) -> Result<(TempDir, Directory), TestFailure> {
    let fixture = TempDir::new(name)?;
    let manifest_dir = Directory::new(fixture.path().to_owned());
    ensure_ok_source(
      fs::write(manifest_dir.join("Cargo.toml"), manifest),
      "manifest fixture can be written",
    )?;
    Ok((fixture, manifest_dir))
  }

  fn ensure_round_trip<T>(input: &str, expected: &str) -> Result<Map<String, T>, TestFailure>
  where
    T: Serialize + DeserializeOwned,
  {
    let parsed = ensure_ok_source(toml::from_str::<Map<String, T>>(input), "manifest entries parse from TOML")?;
    let rendered = ensure_ok_source(toml::to_string(&parsed), "manifest entries serialize back to TOML")?;
    ensure_eq(&rendered.as_str(), &expected, "manifest entries round-trip through TOML unchanged")?;
    Ok(parsed)
  }

  fn ensure_unchanged<T>(input: &str) -> Result<Map<String, T>, TestFailure>
  where
    T: Serialize + DeserializeOwned,
  {
    ensure_round_trip(input, input)
  }

  fn dep_entry<'a, T>(parsed: &'a Map<String, T>, context: &'static str) -> Result<&'a T, TestFailure> {
    ensure_some(parsed.get("dep"), context)
  }

  fn dependency_from(input: &str, context: &'static str) -> Result<Dependency, TestFailure> {
    let parsed = ensure_unchanged::<Dependency>(input)?;
    Ok(dep_entry(&parsed, context)?.clone())
  }

  fn ensure_git_source(git: &GitSource, branch: Option<&str>, rev: Option<&str>, context: &'static str) -> Result<(), TestFailure> {
    ensure_all(&[
      (git.repository.as_deref() == Some("https://example.com/repo.git"), context),
      (git.branch.as_deref() == branch, context),
      (git.tag.is_none(), context),
      (git.rev.as_deref() == rev, context),
    ])
  }

  fn ensure_no_git_source(git: &GitSource, context: &'static str) -> Result<(), TestFailure> {
    ensure_all(&[
      (git.repository.is_none(), context),
      (git.branch.is_none(), context),
      (git.tag.is_none(), context),
      (git.rev.is_none(), context),
    ])
  }

  #[test]
  fn get_manifest_rewrites_dependency_paths_and_drops_self_dependency() -> Result<(), TestFailure> {
    let (_fixture, manifest_dir) = manifest_fixture(
      "manifest-deps",
      r#"[dependencies]
local = { path = "local-crate" }
registry = "1.0.0"
trybuild = { path = "self" }

[dev-dependencies]
dev_local = { path = "dev-crate" }

[target.'cfg(unix)'.dev-dependencies]
target_dev = { path = "target-crate" }
"#,
    )?;

    let manifest = ensure_ok_source(get_manifest(&manifest_dir), "crate manifest can be read")?;
    let local = ensure_some(manifest.dependencies.get("local"), "local dependency remains")?;
    let registry = ensure_some(manifest.dependencies.get("registry"), "registry dependency remains")?;
    let dev_local = ensure_some(manifest.dev_dependencies.get("dev_local"), "dev dependency remains")?;
    let target = ensure_some(manifest.target.get("cfg(unix)"), "target dependency table remains")?;
    let target_dev = ensure_some(target.dev_dependencies.get("target_dev"), "target dev dependency remains")?;

    ensure_all(&[
      (
        !manifest.dependencies.contains_key("trybuild"),
        "trybuild self-dependencies are dropped",
      ),
      (
        local.path.as_ref().is_some_and(|path| path.as_ref().is_absolute()),
        "normal dependency paths are rewritten to absolute paths",
      ),
      (
        dev_local.path.as_ref().is_some_and(|path| path.as_ref().is_absolute()),
        "dev dependency paths are rewritten to absolute paths",
      ),
      (
        target_dev.path.as_ref().is_some_and(|path| path.as_ref().is_absolute()),
        "target dev dependency paths are rewritten to absolute paths",
      ),
      (registry.path.is_none(), "registry dependencies keep path absent"),
    ])
  }

  #[test]
  fn workspace_manifest_rewrites_patch_replace_and_dependency_paths() -> Result<(), TestFailure> {
    let (_fixture, manifest_dir) = manifest_fixture(
      "workspace-manifest",
      r#"[workspace.package]
edition = "2024"

[workspace.dependencies]
shared = { path = "shared-crate" }
trybuild = { path = "self" }

[patch.crates-io]
patched = { path = "patched-crate" }
trybuild = { path = "patched-self" }

[replace]
"old:1.0.0" = { path = "replacement-crate" }
trybuild = { path = "replacement-self" }
"#,
    )?;

    let manifest = ensure_ok_source(try_get_workspace_manifest(&manifest_dir), "workspace manifest can be read")?;
    let shared = ensure_some(manifest.workspace.dependencies.get("shared"), "workspace dependency remains")?;
    let registry = ensure_some(manifest.patch.get("crates-io"), "registry patch table remains")?;
    let patched = ensure_some(registry.crates.get("patched"), "non-self patch remains")?;
    let replacement = ensure_some(manifest.replace.get("old:1.0.0"), "replacement remains")?;

    ensure_all(&[
      (
        shared.path.as_ref().is_some_and(|path| path.as_ref().is_absolute()),
        "workspace dependency paths are rewritten",
      ),
      (
        !manifest.workspace.dependencies.contains_key("trybuild"),
        "workspace self-dependencies are dropped",
      ),
      (
        patched.path.as_ref().is_some_and(|path| path.is_absolute()),
        "patch paths are rewritten",
      ),
      (!registry.crates.contains_key("trybuild"), "trybuild patches are dropped"),
      (
        replacement.path.as_ref().is_some_and(|path| path.is_absolute()),
        "replacement paths are rewritten",
      ),
      (!manifest.replace.contains_key("trybuild"), "trybuild replacements are dropped"),
    ])
  }

  #[test]
  fn missing_workspace_manifest_defaults_to_empty() -> Result<(), TestFailure> {
    let fixture = TempDir::new("workspace-missing")?;
    let manifest = get_workspace_manifest(&Directory::new(fixture.child("missing")));

    ensure_all(&[
      (
        manifest.workspace.dependencies.is_empty(),
        "missing workspace manifests have no inherited dependencies",
      ),
      (manifest.patch.is_empty(), "missing workspace manifests have no patches"),
      (manifest.replace.is_empty(), "missing workspace manifests have no replacements"),
    ])
  }

  #[test]
  fn package_edition_accepts_strings_and_rejects_invalid_inheritance() -> Result<(), TestFailure> {
    let direct = ensure_ok_source(
      toml::from_str::<Package>(
        r#"name = "demo"
edition = "2024""#,
      ),
      "direct package edition parses",
    )?;
    let inherited = ensure_ok_source(
      toml::from_str::<Package>(
        r#"name = "demo"
edition = { workspace = true }"#,
      ),
      "workspace package edition parses",
    )?;

    ensure_all(&[
      (
        format!("{:?}", direct.edition).contains("Edition"),
        "direct editions parse as explicit editions",
      ),
      (
        format!("{:?}", inherited.edition).contains("Inherit"),
        "workspace=true editions parse as inherited editions",
      ),
      (
        toml::from_str::<Package>(
          r#"name = "demo"
edition = { workspace = false }"#,
        )
        .is_err(),
        "workspace=false editions are rejected",
      ),
      (
        toml::from_str::<Package>(
          r#"name = "demo"
edition = { workspace = "yes" }"#,
        )
        .is_err(),
        "non-boolean workspace editions are rejected",
      ),
    ])
  }

  #[test]
  fn non_string_non_table_dependencies_are_rejected() -> Result<(), TestFailure> {
    ensure(
      toml::from_str::<Map<String, Dependency>>("dep = 1").is_err(),
      "dependency entries reject values that are neither strings nor tables",
    )
  }

  #[test]
  fn git_table_dependency_round_trips() -> Result<(), TestFailure> {
    let dep = dependency_from(GIT_BRANCH_DEP, "the git dependency is parsed")?;
    ensure_git_source(
      &dep.git,
      Some("main"),
      None,
      "git dependencies preserve repository and branch coordinates",
    )
  }

  #[test]
  fn string_shorthand_still_parses() -> Result<(), TestFailure> {
    let parsed = ensure_round_trip::<Dependency>(
      r#"dep = "1.0.0"
"#,
      r#"[dep]
version = "1.0.0"
default-features = true
"#,
    )?;
    let dep = dep_entry(&parsed, "the shorthand dependency is parsed")?;
    ensure_all(&[
      (
        dep.version.as_deref() == Some("1.0.0"),
        "string shorthand keeps its version semantics",
      ),
      (
        dep.default_features == Some(true),
        "string shorthand keeps its default-feature semantics",
      ),
    ])?;
    ensure_no_git_source(
      &dep.git,
      "string shorthand keeps its version/default-feature semantics without git coordinates",
    )
  }

  #[test]
  fn unknown_keys_flow_to_rest() -> Result<(), TestFailure> {
    let dep = dependency_from(UNKNOWN_KEY_DEP, "the dependency with an extra key is parsed")?;
    ensure_git_source(&dep.git, None, None, "known git keys land in GitSource")?;
    ensure(
      dep.rest.contains_key("package"),
      "known git keys land in GitSource while unknown keys remain in rest",
    )
  }

  #[test]
  fn absent_git_fields_stay_omitted() -> Result<(), TestFailure> {
    let dep = dependency_from(VERSION_DEP, "the version dependency is parsed")?;
    ensure_no_git_source(&dep.git, "absent git coordinates stay absent in parsed data and rendered TOML")
  }

  #[test]
  fn patch_entry_round_trips() -> Result<(), TestFailure> {
    let parsed = ensure_unchanged::<Patch>(GIT_REV_PATCH)?;
    let patch = dep_entry(&parsed, "the git patch is parsed")?;
    ensure_git_source(
      &patch.git,
      None,
      Some("abc123"),
      "patch entries preserve git repository and revision coordinates",
    )
  }
}
