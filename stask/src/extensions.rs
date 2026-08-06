//! Consumer-owned extension registry for `just x <name>` commands.

use template_core::cli::command::CommandSet;

/// Build this repository's intentionally empty extension registry.
///
/// # Errors
///
/// Returns a typed registration error if the controlled `x` router metadata
/// is invalid.
#[allow(
  clippy::single_call_fn,
  reason = "the repository extension registry is the named composition boundary that supplies the guarded stask runner with its \
            controlled x surface"
)]
pub fn commands() -> template_stask::Result<CommandSet> {
  template_stask::empty_registry("strict-trybuild extensions")
}
