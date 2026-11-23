//! Hardware fingerprinting, `inventory.yaml` merge rules, and node-class grouping (spec §6.3–§6.5).

pub mod fingerprint;
pub mod merge;

pub use fingerprint::{fingerprint, verify_fingerprint_diff, FingerprintDiff};
pub use merge::{merge_inventory, node_classes, MergeNote, MergeNoteKind};
