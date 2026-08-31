use std::path::PathBuf;

use strict_test_support::TestFailure;
use strict_test_support::ensure_all;
use strict_test_support::ensure_eq;

use super::normalize;
use super::normalize::Context;
use crate::internal::model::PathDependency;
use crate::internal::model::PathDependencyClass;
use crate::internal::sys::directory::Directory;

struct Fixture {
  source_dir: Directory,
  workspace:  Directory,
  target_dir: Directory,
  input_file: PathBuf,
  path_deps:  Vec<PathDependency>,
}

impl Fixture {
  fn new() -> Self {
    let dep_dir = Directory::new("/vendor/helper");
    Self {
      source_dir: Directory::new("/workspace/crate"),
      workspace:  Directory::new("/workspace"),
      target_dir: Directory::new("/workspace/target"),
      input_file: PathBuf::from("tests/ui/case.rs"),
      path_deps:  vec![PathDependency {
        name:            "helper".to_owned(),
        normalized_path: dep_dir,
        class:           PathDependencyClass::LegacyTopLevel,
      }],
    }
  }

  fn context(&self) -> Context<'_> {
    Context {
      krate:             "demo",
      source_dir:        &self.source_dir,
      workspace:         &self.workspace,
      input_file:        &self.input_file,
      target_dir:        &self.target_dir,
      path_dependencies: &self.path_deps,
    }
  }

  fn preferred(&self, input: &str) -> String {
    normalize::diagnostics(input, &self.context()).preferred().to_owned()
  }
}

#[test]
fn trim_variation_any_and_concat_cover_empty_and_nonempty_paths() -> Result<(), TestFailure> {
  let fixture = Fixture::new();
  let mut first = normalize::diagnostics("error: first\n", &fixture.context());
  let empty = normalize::diagnostics("", &fixture.context());
  let second = normalize::diagnostics("error: second\n", &fixture.context());

  first.concat(&empty);
  first.concat(&second);

  ensure_all(&[
    (normalize::trim("").is_empty(), "empty trim output stays empty"),
    (
      normalize::trim("text  \n\n") == "text\n",
      "trim removes trailing whitespace and keeps one newline",
    ),
    (
      first.any(|variation| variation.contains("error: second")),
      "concatenated variations can be searched",
    ),
    (first.preferred().contains("error: first"), "concat preserves the first diagnostic"),
    (
      first.preferred().contains("error: second"),
      "concat preserves the second diagnostic",
    ),
  ])
}

#[test]
fn location_rewrites_cover_source_workspace_path_deps_registry_and_out_dir() -> Result<(), TestFailure> {
  let fixture = Fixture::new();
  let rendered = fixture.preferred(
    "error: paths\n--> /workspace/crate/src/lib.rs:10:20\n::: /workspace/crate/src/other.rs:30:40\n--> generated from \
     /workspace/crate/src/inline.rs:50:60\n--> /vendor/helper/src/lib.rs:5:6\n--> \
     /home/me/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/serde_json-1.0.64/src/de.rs:1:2\n--> \
     /workspace/target/debug/build/build_helper-0123456789abcdef/out/generated.rs:7:8\n--> \
     /workspace/target/debug/build/not-an-out-dir/out/generated.rs:9:10\n",
  );

  ensure_all(&[
    (
      rendered.contains("--> src/lib.rs"),
      "source-dir locations at the path position become relative",
    ),
    (
      rendered.contains("::: src/other.rs"),
      "secondary source-dir locations at the path position become relative",
    ),
    (
      rendered.contains("--> generated from $DIR/src/inline.rs"),
      "embedded source-dir paths rewrite to $DIR",
    ),
    (
      rendered.contains("--> $HELPER/src/lib.rs"),
      "path dependencies rewrite to their uppercase placeholder",
    ),
    (
      rendered.contains("--> $CARGO/serde_json-$VERSION/src/de.rs"),
      "cargo registry locations rewrite crate versions",
    ),
    (
      rendered.contains("--> $OUT_DIR[build_helper]/generated.rs"),
      "build-script output directories rewrite to $OUT_DIR",
    ),
    (
      rendered.contains("not-an-out-dir/out/generated.rs"),
      "non-matching output directory components are left alone",
    ),
  ])
}

#[test]
fn custom_registry_normalization_is_appended_and_preserves_historical_output() -> Result<(), TestFailure> {
  let fixture = Fixture::new();
  let input = concat!(
    "error: custom registry\n",
    "--> /home/me/.cargo/registry/src/my-cdn.example.com-abcdef1234567890/demo-1.2.3/src/lib.rs:4:5\n",
  );
  let variations = normalize::diagnostics(input, &fixture.context());
  let preferred = variations.preferred();

  ensure_all(&[
    (
      preferred.contains("--> $CARGO/demo-$VERSION/src/lib.rs"),
      "hyphenated custom registry names normalize in the preferred variation",
    ),
    (
      variations.any(|candidate| candidate == input),
      "a variation from before custom-registry normalization still accepts the historical output",
    ),
    (
      !preferred.contains("my-cdn.example.com"),
      "the preferred variation removes the custom registry identity",
    ),
  ])
}

#[test]
fn malformed_registry_paths_remain_unchanged() -> Result<(), TestFailure> {
  let fixture = Fixture::new();
  let input = concat!(
    "error: malformed registries\n",
    "--> /cargo/registry/src/custom-abcdef123456789/demo/src/lib.rs:1:2\n",
    "--> /cargo/registry/src/custom-ABCDEF1234567890/demo/src/lib.rs:3:4\n",
    "--> /cargo/registry/src/custom-abcdef123456789g/demo/src/lib.rs:5:6\n",
    "--> /cargo/registry/src/-abcdef1234567890/demo/src/lib.rs:7:8\n",
    "--> /cargo/registry/src/custom-abcdef1234567890\n",
    "--> /cargo/registry/source/custom-abcdef1234567890/demo/src/lib.rs:9:10\n",
  );
  let rendered = fixture.preferred(input);

  ensure_eq(
    &rendered.as_str(),
    &input,
    "short, uppercase, nonhex, nameless, incomplete, and non-registry paths remain unchanged",
  )
}

#[test]
fn expanded_path_dependencies_choose_longest_match_and_keep_first_tie_and_historical_match() -> Result<(), TestFailure> {
  let mut fixture = Fixture::new();
  fixture.path_deps = vec![
    PathDependency {
      name:            "parent".to_owned(),
      normalized_path: Directory::new("/vendor/parent"),
      class:           PathDependencyClass::LegacyTopLevel,
    },
    PathDependency {
      name:            "nested_first".to_owned(),
      normalized_path: Directory::new("/vendor/parent/nested"),
      class:           PathDependencyClass::Additional,
    },
    PathDependency {
      name:            "nested_second".to_owned(),
      normalized_path: Directory::new("/vendor/parent/nested"),
      class:           PathDependencyClass::Additional,
    },
  ];
  let variations = normalize::diagnostics(
    "error: nested\n--> /vendor/parent/nested/src/lib.rs:1:2\n--> /vendor/parentish/src/lib.rs:3:4\n",
    &fixture.context(),
  );
  let preferred = variations.preferred();

  ensure_all(&[
    (
      preferred.contains("--> $NESTED_FIRST/src/lib.rs"),
      "the expanded stage chooses the first of the longest matching dependency roots",
    ),
    (
      !preferred.contains("$NESTED_SECOND"),
      "a later equal-length dependency root does not replace the first",
    ),
    (
      variations.any(|candidate| candidate.contains("--> $PARENT/nested/src/lib.rs")),
      "historical variations preserve first-match normalization over legacy root dependencies",
    ),
    (
      preferred.contains("/vendor/parentish/src/lib.rs"),
      "a shared textual prefix without a directory boundary does not match",
    ),
  ])
}

#[test]
fn rustlib_and_near_miss_registry_paths_take_opposite_branches() -> Result<(), TestFailure> {
  let fixture = Fixture::new();
  let rendered = fixture.preferred(
    "error: rust paths\n--> /rustc/0123456789abcdef0123456789abcdef01234567/library/core/src/lib.rs:1:2\n--> \
     /rustc/not-a-valid-hash/library/core/src/lib.rs:3:4\n--> \
     /home/me/.cargo/registry/src/index.crates.io-notvalid/serde_json-1.0.64/src/de.rs:5:6\n",
  );

  ensure_all(&[
    (
      rendered.contains("--> $RUST/core/src/lib.rs"),
      "valid rustc library hashes rewrite to $RUST",
    ),
    (
      rendered.contains("/rustc/not-a-valid-hash/library/core/src/lib.rs"),
      "invalid rustc library hashes are preserved",
    ),
    (
      rendered.contains("index.crates.io-notvalid"),
      "near-miss cargo registry hashes are preserved",
    ),
  ])
}

#[test]
fn strip_line_variants_remove_rustc_trailers_and_keep_near_misses() -> Result<(), TestFailure> {
  let fixture = Fixture::new();
  let rendered = fixture.preferred(
    "error: main\n= note: this compiler was built on 2026-01-01; consider upgrading it if it is out of date\nerror: Could not compile \
     `demo`.\nerror: could not compile `demo`.\nSome errors have detailed explanations: E0001, E0002.\nFor more information about an \
     error, try `rustc --explain E0001`.\nFor more information about this error, try `rustc --explain E0001`.\nTo learn more, run the \
     command again with --verbose.\n= note: this compiler was built yesterday; consider upgrading it if it is out of date\nwarning: kept\n",
  );

  ensure_all(&[
    (
      !rendered.contains("Could not compile"),
      "legacy uppercase could-not-compile trailers are stripped",
    ),
    (
      !rendered.contains("could not compile"),
      "lowercase could-not-compile trailers are stripped",
    ),
    (
      !rendered.contains("detailed explanations"),
      "rustc explanation summaries are stripped",
    ),
    (!rendered.contains("built on 2026"), "compiler build-date notes are stripped"),
    (rendered.contains("built yesterday"), "near-miss compiler build notes remain"),
    (rendered.contains("warning: kept"), "ordinary diagnostics remain"),
  ])
}

#[test]
fn list_and_note_normalizers_cover_digit_and_textual_variants() -> Result<(), TestFailure> {
  let fixture = Fixture::new();
  let rendered = fixture.preferred(concat!(
    "error: lists\n",
    "= help: the following types implement trait Example:\n",
    "            Type01\n",
    "            Type02\n",
    "            Type03\n",
    "            Type04\n",
    "            Type05\n",
    "            Type06\n",
    "            Type07\n",
    "            Type08\n",
    "            Type09\n",
    "            Type10\n",
    "            Type11\n",
    "and 12 others\n",
    "and many others\n",
    "= note: the full type name has been written to '/tmp/long-type.txt'\n",
    "= note: the full name for the type has been written to '/tmp/other-long-type.txt'\n",
  ));

  ensure_all(&[
    (rendered.contains("and $N others"), "numeric trailing and-others counts normalize"),
    (rendered.contains("and many others"), "textual and-others counts are preserved"),
    (!rendered.contains("long-type.txt"), "long type-name notes are stripped"),
    (
      !rendered.contains("Type11"),
      "verbose implementor lists drop entries after the summarized prefix",
    ),
  ])
}

#[test]
fn unindent_handles_heading_notes_suggestions_and_arrow_lookahead() -> Result<(), TestFailure> {
  let fixture = Fixture::new();
  let rendered = fixture.preferred(
    "note: standalone heading\n--> tests/ui/case.rs:1:1\n|\n1 | fn main() {\n| ^^^^^^^^^\n| + let value = 1;\n| - let value = 0;\n= note: \
     continuation\nmore detail\nwarning: nested\n::: tests/ui/case.rs:2:1\n|\n2 | helper();\n",
  );

  ensure_all(&[
    (
      rendered.contains("note: standalone heading"),
      "standalone notes can start an unindent block",
    ),
    (rendered.contains("| + let value = 1;"), "suggestion insertion lines are unindented"),
    (rendered.contains("| - let value = 0;"), "suggestion deletion lines are unindented"),
    (rendered.contains("more detail"), "multi-line note continuations are retained"),
    (rendered.contains("warning: nested"), "nested warning headings are retained"),
  ])
}
