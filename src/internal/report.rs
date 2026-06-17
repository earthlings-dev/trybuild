//! Terminal reporting: the owned [`Reporter`](reporter::Reporter) that owns all
//! output, the high-level test-progress messages, and the optional
//! expected-vs-actual diff.

mod diff;
pub(in crate::internal) mod message;
pub(in crate::internal) mod reporter;
