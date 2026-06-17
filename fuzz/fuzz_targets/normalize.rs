//! Fuzz target exercising `normalize::diagnostics`.
#![no_main]
#![allow(
    unknown_lints,
    mismatched_lifetime_syntaxes,
    reason = "the white-box-included normalize.rs is shared with the library build"
)]

/// White-box reconstruction of the slice of the library's private
/// `crate::internal` tree that the included engine files need. Placing each file
/// at its real module path keeps the engine's `pub(in crate::internal)`
/// visibilities and `crate::internal::…` use-paths resolving exactly as they do
/// under `lib.rs`, so `src/` needs no fuzz-only edits. Declarations mirror
/// `src/internal.rs`, `src/internal/sys.rs`, and `src/internal/diagnostics.rs`. The
/// `fuzz_target!` entry lives inside this module so it can reach the engine's
/// `pub(in crate::internal)` `Context`/`diagnostics` directly, with no crate-root
/// wrapper whose visibility could satisfy neither `unreachable_pub` nor
/// `redundant_pub_crate`.
#[path = "../../src/internal"]
mod internal {
    // Each inline module carries a `#[path]` pointing at the matching real
    // directory under `src/internal/`, so the nested file includes resolve
    // through directories that physically exist; a bare `../`-chain would instead
    // traverse the non-existent `fuzz_targets/internal/…` directories that inline
    // modules otherwise imply, and fail.
    #[path = "model.rs"]
    #[allow(
        dead_code,
        unreachable_pub,
        reason = "the fuzz target uses only PathDependency from this module, so its other items look dead; and `Expected` is `pub` for the library's public API (re-exported at the crate root) yet unreachable in this white-box fuzz binary, which has no public API"
    )]
    pub(in crate::internal) mod model;

    /// Host-system types — only `Directory` is exercised here.
    #[path = "sys"]
    pub(in crate::internal) mod sys {
        #[path = "directory.rs"]
        #[allow(dead_code, reason = "only Directory::new is used by the fuzz target")]
        pub(in crate::internal) mod directory;
    }

    /// The diagnostic normalizer under test.
    #[path = "diagnostics"]
    pub(in crate::internal) mod diagnostics {
        #[path = "normalize.rs"]
        #[allow(
            dead_code,
            clippy::single_call_fn,
            reason = "the fuzz target white-box-includes only normalize.rs, so its unexercised items look dead and `trim` looks single-call even though the library build also calls it from `report::message`"
        )]
        pub(in crate::internal) mod normalize;
    }

    use self::diagnostics::normalize::{self, Context};
    use self::model::PathDependency;
    use self::sys::directory::Directory;
    use libfuzzer_sys::fuzz_target;
    use std::path::Path;

    fuzz_target!(|string: &str| {
        if string.len() > 500 {
            return;
        }
        let context = Context {
            krate: "trybuild000",
            input_file: Path::new("tests/ui/error.rs"),
            source_dir: &Directory::new("/git/trybuild/test_suite"),
            workspace: &Directory::new("/git/trybuild"),
            target_dir: &Directory::new("/git/trybuild/target"),
            path_dependencies: &[PathDependency {
                name: String::from("diesel"),
                normalized_path: Directory::new("/home/user/documents/rust/diesel/diesel"),
            }],
        };
        drop(normalize::diagnostics(string, &context));
    });
}
