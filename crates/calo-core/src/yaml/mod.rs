//! YAML loading, include resolution, anchor/merge-key expansion, and layered configuration (spec §0.1, §5.1).

pub mod error;
pub mod include;
pub mod layered;

pub use error::LoadError;
pub use include::{load_str_with_includes, load_with_includes};
pub use layered::{to_manifest_resolved, Layer, Layered, LayeredBuilder, Origin};
