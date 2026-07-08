//! Fuzz target exercising `normalize::diagnostics`.
#![no_main]

/// White-box reconstruction of the slice of the library's private
/// `crate::internal` tree that `normalize.rs` needs. The target includes the
/// real normalizer but supplies tiny adapter types for `PathDependency` and
/// `Directory`, avoiding broad library-module inclusion and the dead surfaces
/// that come with it.
#[path = "../../src/internal"]
mod internal {
    /// Shared value types used by the included normalizer.
    pub(in crate::internal) mod model {
        use crate::internal::sys::directory::Directory;

        /// A path dependency whose on-disk location is normalized out of diagnostics.
        pub(in crate::internal) struct PathDependency {
            /// The dependency's crate name.
            pub(in crate::internal) name: String,
            /// The canonicalized path to the dependency on disk.
            pub(in crate::internal) normalized_path: Directory,
        }
    }

    /// Host-system types used by the included normalizer.
    pub(in crate::internal) mod sys {
        /// Directory path wrapper matching the normalizer-facing library contract.
        pub(in crate::internal) mod directory {
            use std::borrow::Cow;
            use std::path::PathBuf;

            /// A filesystem directory path.
            pub(in crate::internal) struct Directory {
                /// The displayed path, always carrying a trailing separator.
                display: String,
            }

            impl Directory {
                /// Wraps `path`, appending a trailing separator so it reads as a directory.
                pub(in crate::internal) fn new<P: Into<PathBuf>>(input: P) -> Self {
                    let mut path = input.into();
                    path.push("");
                    let display = path.to_string_lossy().into_owned();
                    Self {
                        display,
                    }
                }

                /// The path as a possibly-lossy UTF-8 string.
                pub(in crate::internal) fn to_string_lossy(&self) -> Cow<'_, str> {
                    Cow::Borrowed(&self.display)
                }
            }
        }
    }

    /// The diagnostic normalizer under test.
    #[path = "diagnostics"]
    pub(in crate::internal) mod diagnostics {
        #[path = "normalize.rs"]
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
        let mut variations = normalize::diagnostics(string, &context);
        let preferred = variations.preferred();
        let _matches_preferred = variations.any(|candidate| candidate == preferred);
        let empty = normalize::diagnostics("", &context);
        variations.concat(&empty);
        drop(normalize::trim(string.as_bytes()));
    });
}
