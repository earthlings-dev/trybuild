//! Assembling the rustflags passed to the test crates: trybuild's own cfgs,
//! ignored lints, and any coverage flags forwarded from the environment.

use std::env;

/// Lints silenced in the test crates, where warnings would be noise.
const IGNORED_LINTS: &[&str] = &["dead_code"];

/// Builds the rustflags array for the generated project as a TOML value.
///
/// Always passes `--cfg trybuild --verbose`, allows [`IGNORED_LINTS`], forwards
/// `-C instrument-coverage` from `RUSTFLAGS` when present, and appends
/// `extra_rustflags`.
#[allow(
    clippy::single_call_fn,
    reason = "the rustflags assembly is a named, documented construction step kept separate from the cargo-command builders in `build::cargo` that consume it"
)]
pub(in crate::internal) fn toml(extra_rustflags: &[&'static str]) -> toml::Value {
    let mut rustflags = vec!["--cfg", "trybuild", "--verbose"];

    for &lint in IGNORED_LINTS {
        rustflags.push("-A");
        rustflags.push(lint);
    }

    if let Some(flags) = env::var_os("RUSTFLAGS") {
        // TODO: could parse this properly and allowlist or blocklist certain
        // flags. This is good enough to at least support cargo-llvm-cov.
        if flags.to_string_lossy().contains("-C instrument-coverage") {
            rustflags.extend(["-C", "instrument-coverage"]);
        }
    }

    rustflags.extend(extra_rustflags);

    toml::Value::Array(
        rustflags
            .into_iter()
            .map(|flag| toml::Value::String(flag.to_owned()))
            .collect(),
    )
}
