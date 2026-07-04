//! Deserialization shims for `package.edition = { workspace = true }`, used to
//! recognize edition inheritance without pulling in any extra data.

use std::fmt;

use serde::de;
use serde::de::Deserialize;
use serde::de::Deserializer;
use serde::de::Visitor;
use serde_derive::Deserialize;

/// The `{ workspace = true }` table form of an inherited `package.edition`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InheritEdition {
  /// Must be `true`; deserialized only to validate the `workspace = true` shape.
  #[allow(
    dead_code,
    reason = "deserialized only to validate the `workspace = true` shape; the field's value is never read"
  )]
  pub workspace: True,
}

/// A boolean that deserializes only from `true`, rejecting `false`.
pub(super) struct True;

impl<'de> Deserialize<'de> for True {
  fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
  where
    D: Deserializer<'de>,
  {
    deserializer.deserialize_bool(Self)
  }
}

impl Visitor<'_> for True {
  type Value = Self;

  fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str("bool")
  }

  fn visit_bool<E>(self, boolean: bool) -> Result<Self::Value, E>
  where
    E: de::Error,
  {
    if boolean {
      Ok(Self)
    } else {
      Err(de::Error::custom("workspace=false is unsupported for package.edition"))
    }
  }
}
