//! Developer-facing tools for an Orivo plugin author: a manifest validator, a
//! host simulator, and (in `tests/wit_compatibility.rs`) a guard against a
//! breaking change to the published `orivo-plugin@1` contract.
//!
//! Everything here calls into `orivo` (the `orivo_lib` crate) rather than
//! re-implementing its rules. `manifest_check` reuses
//! `orivo_lib::plugin_manifest` verbatim, and `simulate` reuses
//! `orivo_lib::plugin_runtime`'s public API. Neither module is copied here —
//! only read from — so the SDK and the host cannot quietly drift apart.

pub mod manifest_check;
pub mod simulate;
