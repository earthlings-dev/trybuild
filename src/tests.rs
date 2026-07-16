use std::path::Path;

macro_rules! test_normalize {
    // Select an overriding literal when one is supplied, otherwise fall back to
    // the default. Returning a single literal avoids leaving the unused default
    // as a bare expression statement, which `-D unused-results` rejects.
    (@pick $default:literal,) => {
        $default
    };
    (@pick $default:literal, $override_value:literal) => {
        $override_value
    };
    (
        $(DIR=$dir:literal)?
        $(WORKSPACE=$workspace:literal)?
        $(INPUT=$input:literal)?
        $(TARGET=$target:literal)?
        $name:literal
    ) => {
        #[test]
        fn test() -> Result<(), strict_test_support::TestFailure> {
            let context = crate::internal::diagnostics::normalize::Context {
                krate: "trybuild000",
                input_file: Path::new(test_normalize!(@pick "tests/ui/error.rs", $($input)?)),
                source_dir: &crate::internal::sys::directory::Directory::new(test_normalize!(@pick "/git/trybuild/test_suite", $($dir)?)),
                workspace: &crate::internal::sys::directory::Directory::new(test_normalize!(@pick "/git/trybuild", $($workspace)?)),
                target_dir: &crate::internal::sys::directory::Directory::new(test_normalize!(@pick "/git/trybuild/target", $($target)?)),
                path_dependencies: &[crate::internal::model::PathDependency {
                    name: String::from("diesel"),
                    normalized_path: crate::internal::sys::directory::Directory::new("/home/user/documents/rust/diesel/diesel"),
                }],
            };
            let original = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/tests/inputs/", $name, ".stderr"));
            let variations = crate::internal::diagnostics::normalize::diagnostics(original, &context);
            let preferred = variations.preferred();
            strict_test_support::ensure_snapshot(
                preferred,
                Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src/tests/snapshots/", $name, ".snap")),
                "normalized diagnostic matches the expected snapshot",
            )
        }
    };
}

mod tests {
  automod::dir!("src/tests");
}
