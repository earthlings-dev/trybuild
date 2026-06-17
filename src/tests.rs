macro_rules! test_normalize {
    // Select an overriding literal when one is supplied, otherwise fall back to
    // the default. Returning a single literal avoids leaving the unused default
    // as a bare expression statement, which `-D unused-results` rejects.
    (@pick $default:literal,) => {
        $default
    };
    (@pick $default:literal, $value:literal) => {
        $value
    };
    (
        $(DIR=$dir:literal)?
        $(WORKSPACE=$workspace:literal)?
        $(INPUT=$input:literal)?
        $(TARGET=$target:literal)?
        $original:literal
        $expected:literal
    ) => {
        #[test]
        fn test() -> Result<(), strict_test_support::TestFailure> {
            let context = crate::internal::diagnostics::normalize::Context {
                krate: "trybuild000",
                input_file: std::path::Path::new(test_normalize!(@pick "tests/ui/error.rs", $($input)?)),
                source_dir: &crate::internal::sys::directory::Directory::new(test_normalize!(@pick "/git/trybuild/test_suite", $($dir)?)),
                workspace: &crate::internal::sys::directory::Directory::new(test_normalize!(@pick "/git/trybuild", $($workspace)?)),
                target_dir: &crate::internal::sys::directory::Directory::new(test_normalize!(@pick "/git/trybuild/target", $($target)?)),
                path_dependencies: &[crate::internal::model::PathDependency {
                    name: String::from("diesel"),
                    normalized_path: crate::internal::sys::directory::Directory::new("/home/user/documents/rust/diesel/diesel"),
                }],
            };
            let original = $original;
            let variations = crate::internal::diagnostics::normalize::diagnostics(original, &context);
            let preferred = variations.preferred();
            let expected = $expected;
            strict_test_support::ensure_eq(
                &preferred,
                &expected,
                "normalized diagnostic matches the expected snapshot",
            )?;
            Ok(())
        }
    };
}

mod tests {
    automod::dir!("src/tests");
}
