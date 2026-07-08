//! The diagnostic normalizer: rewriting rustc's rendered output into the
//! stable, machine-independent form saved in `.stderr` snapshots.
//!
//! Normalization erases environment-specific detail — absolute paths, crate
//! disambiguator hashes, toolchain source locations, dependency versions — and
//! collapses indentation so a snapshot stays valid across machines and rustc
//! versions. Each step is an ordered `Normalization` variant; see
//! [`diagnostics`] for why those steps form an append-only history.

use std::cmp;
use std::mem;
use std::path::Path;
use std::str::Lines;

use self::Normalization::AndOthers;
use self::Normalization::AndOthersVerbose;
use self::Normalization::ArrowOtherCrate;
use self::Normalization::Basic;
use self::Normalization::CargoRegistry;
use self::Normalization::DependencyVersion;
use self::Normalization::HeadingNote;
use self::Normalization::LinesOutsideInputFile;
use self::Normalization::PathDependencies;
use self::Normalization::RelativeToDir;
use self::Normalization::RustLib;
use self::Normalization::StripCouldNotCompile;
use self::Normalization::StripCouldNotCompile2;
use self::Normalization::StripForMoreInformation;
use self::Normalization::StripForMoreInformation2;
use self::Normalization::StripLongTypeNameFiles;
use self::Normalization::TrimEnd;
use self::Normalization::TypeDirBackslash;
use self::Normalization::Unindent;
use self::Normalization::UnindentAfterHelp;
use self::Normalization::UnindentMultilineNote;
use self::Normalization::UnindentSuggestion;
use self::Normalization::WorkspaceLines;
use crate::internal::model::PathDependency;
use crate::internal::sys::directory::Directory;

/// The per-diagnostic context the normalizer needs to recognize and rewrite
/// environment-specific paths and names.
#[derive(Copy, Clone)]
pub(in crate::internal) struct Context<'a> {
  /// The generated test crate's name, rewritten to `$CRATE`.
  pub krate:             &'a str,
  /// The crate-under-test's manifest directory, rewritten to `$DIR`.
  pub source_dir:        &'a Directory,
  /// The workspace root directory, rewritten to `$WORKSPACE`.
  pub workspace:         &'a Directory,
  /// The test's own source file, whose line numbers are preserved.
  pub input_file:        &'a Path,
  /// The workspace target directory, recognized to rewrite `$OUT_DIR`.
  pub target_dir:        &'a Directory,
  /// Path dependencies of the crate under test, rewritten to `$<NAME>`.
  pub path_dependencies: &'a [PathDependency],
}

/// Declares the ordered `Normalization` enum from a list of step names.
///
/// Generates the enum, its `ALL` slice in declaration order, and the
/// [`Variations`] default holding one empty string per step. That order is
/// load-bearing — see [`diagnostics`].
macro_rules! normalizations {
    ($($name:ident,)*) => {
        #[derive(PartialOrd, PartialEq, Copy, Clone)]
        enum Normalization {
            $($name,)*
        }

        impl Normalization {
            const ALL: &'static [Self] = &[$($name),*];
        }

        impl Default for Variations {
            fn default() -> Self {
                Variations {
                    variations: [$(($name, String::new()).1),*],
                }
            }
        }
    };
}

normalizations! {
    Basic,
    StripCouldNotCompile,
    StripCouldNotCompile2,
    StripForMoreInformation,
    StripForMoreInformation2,
    TrimEnd,
    RustLib,
    TypeDirBackslash,
    WorkspaceLines,
    PathDependencies,
    CargoRegistry,
    ArrowOtherCrate,
    RelativeToDir,
    LinesOutsideInputFile,
    Unindent,
    AndOthers,
    StripLongTypeNameFiles,
    UnindentAfterHelp,
    AndOthersVerbose,
    UnindentMultilineNote,
    DependencyVersion,
    HeadingNote,
    UnindentSuggestion,
    // New normalization steps are to be inserted here at the end so that any
    // snapshots saved before your normalization change remain passing.
}

/// For a given compiler output, produces the set of saved outputs against which
/// the compiler's output would be considered correct. If the test's saved
/// stderr file is identical to any one of these variations, the test will pass.
///
/// This is a set rather than just one normalized output in order to avoid
/// breaking existing tests when introducing new normalization steps. Someone
/// may have saved stderr snapshots with an older version of trybuild, and those
/// tests need to continue to pass with newer versions of trybuild.
///
/// There is one "preferred" variation which is what we print when the stderr
/// file is absent or not a match.
#[allow(
  clippy::single_call_fn,
  reason = "the module's public normalizer entry point — also the fuzz target's and the unit tests' entry — so its call count varies by \
            build and an #[expect] would be unfulfilled under cfg(test)"
)]
pub(in crate::internal) fn diagnostics(output: &str, context: &Context<'_>) -> Variations {
  let normalized_input = output.replace("\r\n", "\n");

  let mut result = Variations::default();
  for (variation, normalization) in result.variations.iter_mut().zip(Normalization::ALL.iter().copied()) {
    *variation = apply(&normalized_input, normalization, context);
  }

  result
}

/// The set of normalized renderings of one diagnostic — one per `Normalization`
/// step — any of which a saved snapshot may legitimately match.
pub(in crate::internal) struct Variations {
  /// One normalized string per normalization step, in declaration order.
  variations: [String; Normalization::ALL.len()],
}

impl Variations {
  /// The preferred (most fully normalized) rendering, written out when
  /// creating or overwriting a snapshot.
  pub(in crate::internal) const fn preferred(&self) -> &str {
    // `variations` is a fixed-size array of `Normalization::ALL.len()` (> 0)
    // entries, so this slice pattern is irrefutable — no `unwrap`/`expect`
    // (both denied) and no panicking index. Matching the array place
    // `self.variations` (not a reference to it) with an explicit `ref`
    // binds `last: &String` without match ergonomics, clearing
    // `pattern_type_mismatch` without the borrowed-reference pattern that
    // `needless_borrowed_reference` would reject.
    let [.., ref last] = self.variations;
    last.as_str()
  }

  /// Whether any variation satisfies `f` — used to accept a saved snapshot
  /// that matches the output under any historical normalization.
  pub(in crate::internal) fn any<F: FnMut(&str) -> bool>(&self, mut f: F) -> bool {
    self.variations.iter().any(|stderr| f(stderr))
  }

  /// Appends another diagnostic's variations onto these step by step, so a
  /// file's successive diagnostics accumulate into one snapshot.
  pub(in crate::internal) fn concat(&mut self, other: &Self) {
    for (this, that) in self.variations.iter_mut().zip(&other.variations) {
      if !this.is_empty() && !that.is_empty() {
        this.push('\n');
      }
      this.push_str(that);
    }
  }
}

/// Trims trailing whitespace from (possibly non-UTF-8) output and ensures it
/// ends with exactly one newline, or is left empty.
pub(in crate::internal) fn trim<S: AsRef<[u8]>>(output: S) -> String {
  let bytes = output.as_ref();
  let mut normalized = String::from_utf8_lossy(bytes).into_owned();

  let len = normalized.trim_end().len();
  normalized.replace_range(len.., "");

  if !normalized.is_empty() {
    normalized.push('\n');
  }

  normalized
}

/// Applies a single `Normalization` step to one diagnostic, line by line, then
/// unindents and trims the result.
#[allow(
  clippy::single_call_fn,
  reason = "a named per-step normalization stage, separated from the variation-set driver in `diagnostics` that maps it over every \
            Normalization"
)]
fn apply(original: &str, normalization: Normalization, context: &Context<'_>) -> String {
  let mut normalized = String::new();

  let lines: Vec<&str> = original.lines().collect();
  let mut filter = Filter {
    all_lines: &lines,
    normalization,
    context: *context,
    hide_numbers: 0,
    other_types: None,
  };
  for i in 0..lines.len() {
    if let Some(line) = filter.apply(i) {
      normalized += &line;
      if !normalized.ends_with("\n\n") {
        normalized.push('\n');
      }
    }
  }

  normalized = unindent(normalized, normalization);

  trim(normalized)
}

/// Per-line normalization state for one `Normalization` step over one
/// diagnostic.
struct Filter<'a> {
  /// All lines of the diagnostic, kept for lookahead.
  all_lines:     &'a [&'a str],
  /// The normalization step being applied.
  normalization: Normalization,
  /// The paths and crate name to recognize and rewrite.
  context:       Context<'a>,
  /// Count of upcoming lines whose leading line numbers should be blanked.
  hide_numbers:  usize,
  /// State for collapsing a long "the following types implement..." list.
  other_types:   Option<usize>,
}

impl Filter<'_> {
  /// Normalizes the line at `index`, returning the rewritten line or `None` if
  /// it should be dropped from the snapshot.
  fn apply(&mut self, index: usize) -> Option<String> {
    let mut line = self.all_lines.get(index).copied()?.to_owned();

    if self.hide_numbers > 0 {
      hide_leading_numbers(&mut line);
      self.hide_numbers = self.hide_numbers.saturating_sub(1);
    }

    let trim_start = line.trim_start();
    let indent = line.len().saturating_sub(trim_start.len());
    let prefix = if trim_start.starts_with("--> ") {
      Some("--> ")
    } else if trim_start.starts_with("::: ") {
      Some("::: ")
    } else {
      None
    };

    if prefix == Some("--> ")
      && self.normalization < ArrowOtherCrate
      && let Some(cut_end) = line.rfind(&['/', '\\'][..])
    {
      let cut_start = indent.saturating_add(4);
      line.replace_range(cut_start..=cut_end, "$DIR/");
      return Some(line);
    }

    if prefix.is_some() {
      return Some(self.normalize_location(index, line, indent));
    }

    self.strip_line(index, line)
  }

  /// Rewrites a `--> `/`::: ` location line: erasing absolute paths down to
  /// `$DIR`/`$WORKSPACE`/`$RUST`/`$CARGO`/`$OUT_DIR`, and blanking the trailing
  /// line numbers of locations that point outside the input file.
  #[allow(
    clippy::single_call_fn,
    reason = "the location-line normalizer, lifted out of `apply` as one named phase so the `--> ` rewrite ladder reads as a sequence of \
              guarded steps within the nesting budget"
  )]
  fn normalize_location(&mut self, index: usize, mut line: String, indent: usize) -> String {
    line = line.replace('\\', "/");
    let line_lower = line.to_ascii_lowercase();
    let target_dir_pat = lower_slash(&self.context.target_dir.to_string_lossy());
    let source_dir_pat = lower_slash(&self.context.source_dir.to_string_lossy());

    let mut other_crate = false;
    if line_lower.find(&target_dir_pat) == Some(indent.saturating_add(4)) {
      other_crate = replace_out_dir(&mut line, indent, &target_dir_pat);
    } else if let Some(i) = line_lower.find(&source_dir_pat) {
      if self.replace_source_dir(&mut line, &line_lower, indent, i, &source_dir_pat) {
        return line;
      }
      other_crate = true;
    } else {
      let workspace_pat = lower_slash(&self.context.workspace.to_string_lossy());
      if let Some(i) = line_lower.find(&workspace_pat) {
        line.replace_range(i..i.saturating_add(workspace_pat.len()).saturating_sub(1), "$WORKSPACE");
        other_crate = true;
      }
    }

    if self.normalization >= PathDependencies && !other_crate {
      other_crate = self.replace_path_dependency(&mut line, &line_lower);
    }
    if self.normalization >= RustLib && !other_crate {
      other_crate = replace_rust_lib(&mut line, indent);
    }
    if self.normalization >= CargoRegistry && !other_crate {
      other_crate = self.replace_cargo_registry(&mut line, indent);
    }
    if other_crate && self.normalization >= WorkspaceLines {
      // Blank out line numbers for this particular error since rustc tends
      // to reach into code from outside of the test case. The test stderr
      // shouldn't need to be updated every time we touch those files.
      hide_trailing_numbers(&mut line);
      self.hide_numbers = 1;
      self.extend_hidden_numbers(index);
    }

    line
  }

  /// Rewrites a source-dir-relative location to `$DIR`. Returns `true` when
  /// the caller should emit the line unchanged — either line-number erasure is
  /// not yet enabled, or the path is the input file itself (whose line numbers
  /// are preserved) — and `false` once `$DIR` was substituted and the line is
  /// to be treated as belonging to another crate.
  #[allow(
    clippy::single_call_fn,
    reason = "a named branch of `normalize_location`, isolating the source-dir vs input-file decision and its early-emit cases from the \
              surrounding ladder"
  )]
  fn replace_source_dir(&self, line: &mut String, line_lower: &str, indent: usize, i: usize, source_dir_pat: &str) -> bool {
    if self.normalization >= RelativeToDir && i == indent.saturating_add(4) {
      line.replace_range(i..i.saturating_add(source_dir_pat.len()), "");
      if self.normalization < LinesOutsideInputFile {
        return true;
      }
      let input_file_pat = lower_slash(&self.context.input_file.to_string_lossy());
      let after = line_lower.get(i.saturating_add(source_dir_pat.len())..).unwrap_or("");
      if after.starts_with(&input_file_pat) {
        // Keep line numbers only within the input file (the path passed
        // to our `fn compile_fail`). All other source files get line
        // numbers erased below.
        return true;
      }
    } else {
      line.replace_range(i..i.saturating_add(source_dir_pat.len()).saturating_sub(1), "$DIR");
      if self.normalization < LinesOutsideInputFile {
        return true;
      }
    }
    false
  }

  /// Rewrites the first matching path dependency to `$<NAME>`, returning
  /// whether a rewrite occurred.
  #[allow(
    clippy::single_call_fn,
    reason = "a named branch of `normalize_location`, keeping the path-dependency scan and its name-uppercasing out of the main ladder"
  )]
  fn replace_path_dependency(&self, line: &mut String, line_lower: &str) -> bool {
    for path_dep in self.context.path_dependencies {
      let path_dep_pat = lower_slash(&path_dep.normalized_path.to_string_lossy());
      if let Some(i) = line_lower.find(&path_dep_pat) {
        let var = format!("${}", path_dep.name.to_uppercase().replace('-', "_"));
        line.replace_range(i..i.saturating_add(path_dep_pat.len()).saturating_sub(1), &var);
        return true;
      }
    }
    false
  }

  /// Rewrites a cargo registry path to `$CARGO` (and the version to `$VERSION`
  /// once [`DependencyVersion`] is enabled), returning whether a rewrite
  /// occurred.
  #[allow(
    clippy::single_call_fn,
    reason = "a named branch of `normalize_location`, isolating the registry hash/version index arithmetic from the surrounding ladder"
  )]
  fn replace_cargo_registry(&self, line: &mut String, indent: usize) -> bool {
    let Some(pos) = line
      .find("/registry/src/github.com-")
      .or_else(|| line.find("/registry/src/index.crates.io-"))
    else {
      return false;
    };
    let Some(dash) = line.get(pos..).unwrap_or("").find('-') else {
      return false;
    };
    let hash_start = pos.saturating_add(dash).saturating_add(1);
    let hash_end = hash_start.saturating_add(16);
    if !line.get(hash_start..hash_end).is_some_and(is_ascii_lowercase_hex) || !line.get(hash_end..).unwrap_or("").starts_with('/') {
      return false;
    }

    // --> /home/.cargo/registry/src/github.com-1ecc6299db9ec823/serde_json-1.0.64/src/de.rs:2584:8
    // --> $CARGO/serde_json-1.0.64/src/de.rs:2584:8
    line.replace_range(indent.saturating_add(4)..hash_end, "$CARGO");
    if self.normalization >= DependencyVersion {
      replace_dependency_version(line, indent);
    }
    true
  }

  /// Advances [`hide_numbers`](Self::hide_numbers) over the border and
  /// line-number rows that follow an other-crate location, so they are blanked
  /// along with it.
  #[allow(
    clippy::single_call_fn,
    reason = "a named helper for the WorkspaceLines lookahead, keeping the bordered-row scan out of `normalize_location`"
  )]
  fn extend_hidden_numbers(&mut self, index: usize) {
    while let Some(next_line) = self.all_lines.get(index.saturating_add(self.hide_numbers)) {
      match next_line.trim_start().chars().next().unwrap_or_default() {
        '0'..='9' | '|' | '.' => {
          self.hide_numbers = self.hide_numbers.saturating_add(1);
        }
        _ => break,
      }
    }
  }

  /// Applies the non-location normalization steps: dropping rustc boilerplate,
  /// trimming, collapsing `and N others`, the verbose-list cap, and the final
  /// `$CRATE`/`$DIR`/`$WORKSPACE` substitutions. Returns `None` when the line
  /// is dropped from the snapshot.
  #[allow(
    clippy::single_call_fn,
    reason = "the non-location half of `apply`, lifted out so the line-dropping guard clauses read as one sequence and keep `apply` short"
  )]
  fn strip_line(&mut self, index: usize, mut line: String) -> Option<String> {
    let trim_start = line.trim_start();
    let indent = line.len().saturating_sub(trim_start.len());

    if line.starts_with("error: aborting due to ") {
      return None;
    }

    if line == "To learn more, run the command again with --verbose." {
      return None;
    }

    if trim_start.starts_with("= note: this compiler was built on 2")
      && trim_start.ends_with("; consider upgrading it if it is out of date")
    {
      return None;
    }

    if self.normalization >= StripCouldNotCompile && line.starts_with("error: Could not compile `") {
      return None;
    }

    if self.normalization >= StripCouldNotCompile2 && line.starts_with("error: could not compile `") {
      return None;
    }

    if self.normalization >= StripForMoreInformation && line.starts_with("For more information about this error, try `rustc --explain") {
      return None;
    }

    if self.normalization >= StripForMoreInformation2 {
      if line.starts_with("Some errors have detailed explanations:") {
        return None;
      }
      if line.starts_with("For more information about an error, try `rustc --explain") {
        return None;
      }
    }

    if self.normalization >= TrimEnd {
      let trimmed_len = line.trim_end().len();
      line.replace_range(trimmed_len.., "");
    }

    if self.normalization >= TypeDirBackslash
      && line
        .trim_start()
        .starts_with("= note: required because it appears within the type")
    {
      line = line.replace('\\', "/");
    }

    if self.normalization >= AndOthers {
      normalize_and_others(&mut line);
    }

    if self.normalization >= StripLongTypeNameFiles && is_long_type_name_note(&line) {
      return None;
    }

    if self.normalization >= AndOthersVerbose && self.collapse_other_types(index, &mut line, indent) == Collapse::Drop {
      return None;
    }

    line = line.replace(self.context.krate, "$CRATE");
    line = replace_case_insensitive(&line, &self.context.source_dir.to_string_lossy(), "$DIR/");
    line = replace_case_insensitive(&line, &self.context.workspace.to_string_lossy(), "$WORKSPACE/");

    Some(line)
  }

  /// Collapses a long verbose "the following types implement trait" listing to
  /// nine entries plus `and $N others`, tracking progress in
  /// [`other_types`](Self::other_types). Returns whether this line is dropped.
  #[allow(
    clippy::single_call_fn,
    reason = "a named branch of `strip_line`, isolating the verbose-list counter state machine and its lookahead from the guard-clause \
              sequence"
  )]
  fn collapse_other_types(&mut self, index: usize, line: &mut String, indent: usize) -> Collapse {
    let trim_start = line.trim_start();
    if trim_start.starts_with("= help: the following types implement trait ")
      || trim_start.starts_with("= help: the following other types implement trait ")
    {
      self.other_types = Some(0);
      return Collapse::Keep;
    }

    let Some(seen) = self.other_types else {
      return Collapse::Keep;
    };
    if indent < 12 || trim_start == "and $N others" {
      self.other_types = None;
      return Collapse::Keep;
    }

    let count = seen.saturating_add(1);
    self.other_types = Some(count);
    if count > 9 {
      return Collapse::Drop;
    }
    if count == 9
      && let Some(next) = self.all_lines.get(index.saturating_add(1))
    {
      let next_trim_start = next.trim_start();
      let next_indent = next.len().saturating_sub(next_trim_start.len());
      if indent == next_indent {
        line.replace_range(indent.saturating_sub(2).., "and $N others");
      }
    }
    Collapse::Keep
  }
}

/// Whether a verbose-list line survives into the snapshot or is dropped, as
/// decided by [`Filter::collapse_other_types`].
#[derive(PartialEq, Eq)]
enum Collapse {
  /// Keep the line.
  Keep,
  /// Drop the line from the snapshot.
  Drop,
}

/// Lowercases `text` and normalizes backslashes to forward slashes — the form
/// in which the normalizer matches environment paths against a line.
fn lower_slash(text: &str) -> String {
  text.to_ascii_lowercase().replace('\\', "/")
}

/// Rewrites a build-script `OUT_DIR` path to `$OUT_DIR[<crate>]` in place,
/// returning whether a rewrite occurred. The target dir has already been matched
/// at `indent + 4`; this scans the trailing components for `<crate>-<hash>/out`.
#[allow(
  clippy::single_call_fn,
  reason = "a named branch of `normalize_location`, lifted out so the OUT_DIR component scan reads as one step and stays within the \
            nesting budget"
)]
fn replace_out_dir(line: &mut String, indent: usize, target_dir_pat: &str) -> bool {
  let mut offset = indent.saturating_add(4).saturating_add(target_dir_pat.len());
  let mut out_dir_crate_name = None;
  while let Some(slash) = line.get(offset..).unwrap_or("").find('/') {
    let component = line.get(offset..offset.saturating_add(slash)).unwrap_or("");
    if component == "out"
      && let Some(name) = out_dir_crate_name
    {
      let replacement = format!("$OUT_DIR[{name}]");
      line.replace_range(indent.saturating_add(4)..offset.saturating_add(3), &replacement);
      return true;
    }
    if is_out_dir_crate(component) {
      out_dir_crate_name = component.get(..component.len().saturating_sub(17));
    } else {
      out_dir_crate_name = None;
    }
    offset = offset.saturating_add(slash).saturating_add(1);
  }
  false
}

/// Whether `component` is a cargo `OUT_DIR` crate directory: a crate name, a
/// `-`, then a 16-character lowercase hex disambiguator.
#[allow(
  clippy::single_call_fn,
  reason = "a named predicate for the <crate>-<16-hex> OUT_DIR directory shape, lifted out of replace_out_dir's component-scan loop so \
            that loop reads as one step"
)]
fn is_out_dir_crate(component: &str) -> bool {
  component.len() > 17
    && component.rfind('-') == Some(component.len().saturating_sub(17))
    && component
      .get(component.len().saturating_sub(16)..)
      .is_some_and(is_ascii_lowercase_hex)
}

/// Rewrites a rustup/rustc standard-library source path to `$RUST`, returning
/// whether a rewrite occurred.
#[allow(
  clippy::single_call_fn,
  reason = "a named branch of `normalize_location`, lifted out so the three `$RUST` path shapes read as one decision with a final else"
)]
fn replace_rust_lib(line: &mut String, indent: usize) -> bool {
  if let Some(pos) = line.find("/rustlib/src/rust/src/") {
    // --> /home/.rustup/toolchains/nightly/lib/rustlib/src/rust/src/libstd/net/ip.rs:83:1
    // --> $RUST/src/libstd/net/ip.rs:83:1
    line.replace_range(indent.saturating_add(4)..pos.saturating_add(17), "$RUST");
    true
  } else if let Some(pos) = line.find("/rustlib/src/rust/library/") {
    // --> /home/.rustup/toolchains/nightly/lib/rustlib/src/rust/library/std/src/net/ip.rs:83:1
    // --> $RUST/std/src/net/ip.rs:83:1
    line.replace_range(indent.saturating_add(4)..pos.saturating_add(25), "$RUST");
    true
  } else if is_rustc_hash_library(line, indent) {
    // --> /rustc/c5c7d2b37780dac1092e75f12ab97dd56c30861e/library/std/src/net/ip.rs:83:1
    // --> $RUST/std/src/net/ip.rs:83:1
    line.replace_range(indent.saturating_add(4)..indent.saturating_add(59), "$RUST");
    true
  } else {
    false
  }
}

/// Whether `line` holds a `/rustc/<40 hex>/library/` standard-library path at
/// `indent + 4`.
#[allow(
  clippy::single_call_fn,
  reason = "a named predicate for the /rustc/<40-hex>/library/ standard-library path shape, keeping replace_rust_lib's three-branch $RUST \
            ladder readable"
)]
fn is_rustc_hash_library(line: &str, indent: usize) -> bool {
  line.get(indent.saturating_add(4)..).unwrap_or("").starts_with("/rustc/")
    && line
      .get(indent.saturating_add(11)..indent.saturating_add(51))
      .is_some_and(is_ascii_lowercase_hex)
    && line.get(indent.saturating_add(51)..).unwrap_or("").starts_with("/library/")
}

/// Rewrites the version in a `$CARGO/<crate>-<version>/…` path to `$VERSION`.
#[allow(
  clippy::single_call_fn,
  reason = "a named sub-step of `replace_cargo_registry`, isolating the crate-name/version boundary arithmetic from the `$CARGO` rewrite"
)]
fn replace_dependency_version(line: &mut String, indent: usize) {
  let rest = line.get(indent.saturating_add(11)..).unwrap_or("");
  if let Some(end_of_version) = rest.find('/')
    && let Some(dot) = rest.get(..end_of_version).and_then(|head| head.find('.'))
    && let Some(end_of_crate_name) = rest.get(..dot).and_then(|head| head.rfind('-'))
  {
    let start = indent.saturating_add(end_of_crate_name).saturating_add(12);
    let end = indent.saturating_add(end_of_version).saturating_add(11);
    line.replace_range(start..end, "$VERSION");
  }
}

/// Collapses a trailing `and <N> others` count to `and $N others`.
#[allow(
  clippy::single_call_fn,
  reason = "a named step of `strip_line`, keeping the `and N others` digit-span check in its own scope"
)]
fn normalize_and_others(line: &mut String) {
  let trim_start = line.trim_start();
  if !(trim_start.starts_with("and ") && line.ends_with(" others")) {
    return;
  }
  let indent = line.len().saturating_sub(trim_start.len());
  let num_start = indent.saturating_add("and ".len());
  let num_end = line.len().saturating_sub(" others".len());
  let all_digits = num_start < num_end
    && line
      .get(num_start..num_end)
      .unwrap_or("")
      .bytes()
      .all(|byte| byte.is_ascii_digit());
  if all_digits {
    line.replace_range(num_start..num_end, "$N");
  }
}

/// Whether `line` is rustc's "the full type name has been written to …" note,
/// which is dropped from snapshots.
#[allow(
  clippy::single_call_fn,
  reason = "a named predicate matching rustc's 'full type name has been written to' note that StripLongTypeNameFiles drops, kept out of \
            strip_line's guard-clause sequence"
)]
fn is_long_type_name_note(line: &str) -> bool {
  let trimmed = line.trim_start();
  let note = trimmed.strip_prefix("= note: ").unwrap_or(trimmed);
  note.starts_with("the full type name has been written to") || note.starts_with("the full name for the type has been written to")
}

/// Whether every byte of `s` is a lowercase hexadecimal digit.
fn is_ascii_lowercase_hex(text: &str) -> bool {
  text.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// Replaces a line's leading run of digits and spaces with spaces, blanking the
/// line number while keeping the column alignment: `"10 | T: Send,"` becomes
/// `"   | T: Send,"`.
#[allow(
  clippy::single_call_fn,
  reason = "a named line-mutation helper documenting the leading-number-blanking transform, kept out of the dense per-line filter body"
)]
fn hide_leading_numbers(line: &mut String) {
  let n = line
    .bytes()
    .take_while(|byte: &u8| *byte == b' ' || byte.is_ascii_digit())
    .count();
  for i in 0..n {
    line.replace_range(i..=i, " ");
  }
}

/// Strips up to two trailing `:line[:column]` number groups from a path:
/// `"main.rs:22:29"` becomes `"main.rs"`.
#[allow(
  clippy::single_call_fn,
  reason = "a named line-mutation helper documenting the trailing-number-stripping transform, kept out of the dense per-line filter body"
)]
fn hide_trailing_numbers(line: &mut String) {
  for _ in 0..2 {
    let digits = line.bytes().rev().take_while(u8::is_ascii_digit).count();
    let without_digits = line.len().saturating_sub(digits);
    if digits == 0 || !line.get(..without_digits).unwrap_or("").ends_with(':') {
      return;
    }
    line.replace_range(without_digits.saturating_sub(1).., "");
  }
}

/// Replaces every case-insensitive, slash-insensitive occurrence of `pattern`
/// in `line` with `replacement`.
///
/// Matching is performed on lowercased, forward-slash-normalized copies, but the
/// surviving text is taken from the original `line`; backslashes within a
/// matched path are normalized to forward slashes. A match that immediately
/// follows an alphanumeric character is treated as not a real path boundary and
/// left in place.
fn replace_case_insensitive(line: &str, pattern: &str, replacement: &str) -> String {
  let line_lowered = line.to_ascii_lowercase().replace('\\', "/");
  let pattern_lower = pattern.to_ascii_lowercase().replace('\\', "/");
  let mut replaced = String::with_capacity(line.len());

  let line_lower = line_lowered.as_str();
  let mut split = line_lower.split(&pattern_lower);
  let mut pos: usize = 0;
  let mut insert_replacement = false;
  while let Some(segment) = split.next() {
    if insert_replacement {
      replaced.push_str(replacement);
      pos = pos.saturating_add(pattern.len());
    }
    let mut keep = line.get(pos..pos.saturating_add(segment.len())).unwrap_or("");
    if insert_replacement {
      let end_of_maybe_path = keep.find(&[' ', ':'][..]).unwrap_or(keep.len());
      replaced.push_str(&keep.get(..end_of_maybe_path).unwrap_or("").replace('\\', "/"));
      pos = pos.saturating_add(end_of_maybe_path);
      keep = keep.get(end_of_maybe_path..).unwrap_or("");
    }
    replaced.push_str(keep);
    pos = pos.saturating_add(keep.len());
    insert_replacement = true;
    if replaced.ends_with(|ch: char| ch.is_ascii_alphanumeric())
      && let Some(ch) = line.get(pos..).unwrap_or("").chars().next()
    {
      replaced.push(ch);
      pos = pos.saturating_add(ch.len_utf8());
      split = line_lower.get(pos..).unwrap_or("").split(&pattern_lower);
      insert_replacement = false;
    }
  }

  replaced
}

/// Classification of a diagnostic line for the unindent pass.
#[derive(PartialEq)]
enum IndentedLineKind {
  /// A diagnostic heading line such as `error[...]:` or `warning:`.
  Heading,

  /// A bordered source/code line, carrying the maximum number of leading
  /// spaces that can be cut based on this line: `   --> foo` = 2,
  /// `    | foo` = 3, `   ::: foo` = 2, `10  | foo` = 1.
  Code(usize),

  /// A `note:`/`help:`/`...` continuation line.
  Note,

  /// Any other line, carrying its number of leading spaces.
  Other(usize),
}

/// Removes the common left indentation from each `--> ` code block so snapshots
/// do not depend on the width of rustc's right-aligned line-number column.
#[allow(
  clippy::single_call_fn,
  reason = "a named, documented normalization pass kept distinct from the per-line filter, run once as the final stage of `apply`"
)]
fn unindent(diag: String, normalization: Normalization) -> String {
  if normalization < Unindent {
    return diag;
  }

  let mut normalized = String::new();
  let mut lines = diag.lines();

  while let Some(line) = lines.next() {
    normalized.push_str(line);
    normalized.push('\n');

    if indented_line_kind(line, true, &mut false, normalization) != IndentedLineKind::Heading {
      continue;
    }

    let mut ahead = lines.clone();
    let Some(next_line) = ahead.next() else {
      continue;
    };
    let IndentedLineKind::Code(indent) = indented_line_kind(next_line, false, &mut false, normalization) else {
      continue;
    };
    if !next_line.get(indent.saturating_add(1)..).unwrap_or("").starts_with("--> ") {
      continue;
    }

    let (lines_in_block, least_indent) = measure_unindent_block(indent, ahead, normalization);
    emit_unindent_block(&mut normalized, &mut lines, lines_in_block, least_indent, normalization);
  }

  normalized
}

/// Counts the lines belonging to one `--> ` code block (the rows following a
/// heading) and the least cuttable indentation across them.
#[allow(
  clippy::single_call_fn,
  reason = "a named phase of `unindent`, giving the block-measuring loop its own scope so its `line`/`indent` bindings don't shadow the \
            outer pass"
)]
fn measure_unindent_block(first_indent: usize, ahead: Lines<'_>, normalization: Normalization) -> (usize, usize) {
  let mut lines_in_block: usize = 1;
  let mut least_indent = first_indent;
  let mut previous_line_is_note = false;
  for line in ahead {
    match indented_line_kind(line, false, &mut previous_line_is_note, normalization) {
      IndentedLineKind::Heading => break,
      IndentedLineKind::Code(indent) => {
        lines_in_block = lines_in_block.saturating_add(1);
        least_indent = cmp::min(least_indent, indent);
      }
      IndentedLineKind::Note => lines_in_block = lines_in_block.saturating_add(1),
      IndentedLineKind::Other(spaces) => {
        if spaces > 10 {
          lines_in_block = lines_in_block.saturating_add(1);
        } else {
          break;
        }
      }
    }
  }
  (lines_in_block, least_indent)
}

/// Emits `lines_in_block` lines from `lines`, cutting `least_indent` spaces
/// after the border on each bordered row so snapshots stay column-stable.
#[allow(
  clippy::single_call_fn,
  reason = "a named phase of `unindent`, giving the block-emitting loop its own scope so its `line` binding doesn't shadow the outer pass"
)]
fn emit_unindent_block(
  normalized: &mut String,
  lines: &mut Lines<'_>,
  lines_in_block: usize,
  least_indent: usize,
  normalization: Normalization,
) {
  let mut previous_line_is_note = false;
  for _ in 0..lines_in_block {
    let Some(line) = lines.next() else {
      break;
    };
    if let IndentedLineKind::Code(_) | IndentedLineKind::Other(_) =
      indented_line_kind(line, false, &mut previous_line_is_note, normalization)
    {
      let space = line.find(' ').unwrap_or(line.len());
      normalized.push_str(line.get(..space).unwrap_or(""));
      normalized.push_str(line.get(space.saturating_add(least_indent)..).unwrap_or(""));
    } else {
      normalized.push_str(line);
    }
    normalized.push('\n');
  }
}

/// Classifies one line for [`unindent`], updating `previous_line_is_note` so
/// multi-line note continuations are recognized.
fn indented_line_kind(
  line: &str,
  first_line_in_block: bool,
  previous_line_is_note: &mut bool,
  normalization: Normalization,
) -> IndentedLineKind {
  let previous_line_was_note = mem::replace(previous_line_is_note, false);

  if is_diagnostic_heading(line) || is_heading_note(line, first_line_in_block, normalization) {
    return IndentedLineKind::Heading;
  }

  if is_note_continuation(line, previous_line_was_note, normalization) {
    *previous_line_is_note = true;
    return IndentedLineKind::Note;
  }

  if let Some(spaces) = ellipsis_code_indent(line) {
    return IndentedLineKind::Code(spaces);
  }

  let source_line = SourceLine::parse(line);
  if let Some(indent) = source_line.code_indent(normalization) {
    return IndentedLineKind::Code(indent);
  }

  IndentedLineKind::Other(source_line.other_indent())
}

/// Whether `line` starts a diagnostic heading.
#[allow(
  clippy::single_call_fn,
  reason = "heading recognition is a named part of indented-line classification, separated from note and source-line parsing"
)]
fn is_diagnostic_heading(line: &str) -> bool {
  let heading_len = if line.starts_with("error") {
    Some("error".len())
  } else if line.starts_with("warning") {
    Some("warning".len())
  } else {
    None
  };
  heading_len.is_some_and(|len| line.get(len..).unwrap_or("").starts_with(&[':', '['][..]))
}

/// Whether a top-of-block note is promoted to a heading by the current
/// normalization step.
#[allow(
  clippy::single_call_fn,
  reason = "heading-note promotion is a distinct historical normalization rule within indented-line classification"
)]
fn is_heading_note(line: &str, first_line_in_block: bool, normalization: Normalization) -> bool {
  first_line_in_block && normalization >= HeadingNote && line.starts_with("note: ")
}

/// Whether `line` is a note/help/ellipsis continuation in an unindent block.
#[allow(
  clippy::single_call_fn,
  reason = "note-continuation recognition names the stateful previous-note rule outside the main classifier"
)]
fn is_note_continuation(line: &str, previous_line_was_note: bool, normalization: Normalization) -> bool {
  line.starts_with("note:")
    || line == "..."
    || normalization >= UnindentAfterHelp && line.starts_with("help:")
    || normalization >= UnindentMultilineNote && previous_line_was_note && line.starts_with("      ")
}

/// The cuttable indentation of an ellipsis-prefixed code line.
#[allow(
  clippy::single_call_fn,
  reason = "ellipsis-code indentation is a rustc-rendering special case separated from numbered source-line parsing"
)]
fn ellipsis_code_indent(line: &str) -> Option<usize> {
  let is_space = |byte: &u8| *byte == b' ';
  line.strip_prefix("... ").map(|rest| rest.bytes().take_while(is_space).count())
}

/// Parsed shape of a bordered rustc source line.
struct SourceLine<'a> {
  /// The number of spaces before the source-border marker, excluding digits.
  source_indent:   usize,
  /// Whether the line carried a right-aligned source line number.
  has_line_number: bool,
  /// The line content after indentation, optional digits, and following spaces.
  rest:            &'a str,
}

impl<'a> SourceLine<'a> {
  /// Parses the indentation, optional line-number column, and content marker.
  #[allow(
    clippy::single_call_fn,
    reason = "source-line parsing names the column decomposition consumed by the code and ordinary-text classifiers"
  )]
  fn parse(line: &'a str) -> Self {
    let leading_spaces = line.bytes().take_while(|byte| *byte == b' ').count();
    let digits = line
      .get(leading_spaces..)
      .unwrap_or("")
      .bytes()
      .take_while(u8::is_ascii_digit)
      .count();
    let spaces_after_digits = line
      .get(leading_spaces.saturating_add(digits)..)
      .unwrap_or("")
      .bytes()
      .take_while(|byte| *byte == b' ')
      .count();
    let source_indent = leading_spaces.saturating_add(spaces_after_digits);
    let content_start = leading_spaces.saturating_add(digits).saturating_add(spaces_after_digits);
    Self {
      source_indent,
      has_line_number: digits > 0,
      rest: line.get(content_start..).unwrap_or(""),
    }
  }

  /// The unindent block's cuttable source indentation, if this is a code row.
  #[allow(
    clippy::single_call_fn,
    reason = "code-indent classification is the source-line object's primary query and keeps the marker rules grouped"
  )]
  fn code_indent(&self, normalization: Normalization) -> Option<usize> {
    if self.source_indent == 0 {
      return None;
    }
    if self.is_source_border() || self.is_suggestion(normalization) || !self.has_line_number && self.is_source_location_or_note() {
      return Some(self.source_indent.saturating_sub(1));
    }
    None
  }

  /// The indentation used when this row is ordinary text rather than code.
  #[allow(
    clippy::single_call_fn,
    reason = "ordinary-text indentation is the paired fallback query to code_indent in the unindent classifier"
  )]
  const fn other_indent(&self) -> usize {
    if self.has_line_number { 0 } else { self.source_indent }
  }

  /// Whether the row is a normal `|` source-code border line.
  #[allow(
    clippy::single_call_fn,
    reason = "source-border recognition names one marker family inside the source-line code classifier"
  )]
  fn is_source_border(&self) -> bool {
    self.rest == "|" || self.rest.starts_with("| ")
  }

  /// Whether the row is a rustc suggestion insertion/deletion/replacement line.
  #[allow(
    clippy::single_call_fn,
    reason = "suggestion recognition names the historical unindent rule for numbered rustc suggestion rows"
  )]
  fn is_suggestion(&self, normalization: Normalization) -> bool {
    normalization >= UnindentSuggestion
      && self.has_line_number
      && (self.rest == "~"
        || self.rest.starts_with("~ ")
        || self.rest == "+"
        || self.rest.starts_with("+ ")
        || self.rest == "-"
        || self.rest.starts_with("- "))
  }

  /// Whether the row is a location, secondary location, or note marker line.
  #[allow(
    clippy::single_call_fn,
    reason = "location-marker recognition names the unnumbered rustc marker family inside the source-line classifier"
  )]
  fn is_source_location_or_note(&self) -> bool {
    self.rest.starts_with("--> ") || self.rest.starts_with("::: ") || self.rest.starts_with("= ")
  }
}
