//! Typed schema validation and serialization errors (spec §0.2, §10, §18).

use thiserror::Error;

/// Errors produced when parsing, serializing, or checking Calorimeter schema documents (spec §0.2).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// The top-level `schema:` key was missing from the YAML document (spec §0.2).
    #[error("missing top-level `schema:` header")]
    MissingSchemaHeader,

    /// The `schema:` header did not match the expected document type/major version (spec §0.2).
    #[error("schema version mismatch: expected `{expected}`, found `{found}`")]
    VersionMismatch {
        /// Expected schema identifier.
        expected: String,
        /// Actual schema identifier found in input.
        found: String,
    },

    /// Required field in a document was empty or missing (spec §10 R-REC-1).
    #[error("missing required field: `{field}`")]
    MissingRequiredField {
        /// Dotted path of the missing or empty required field.
        field: String,
    },

    /// An observation block is present whose required capability tier is not active in `provenance.tiers_active` (spec §10 R-REC-8).
    #[error("observation block `{block}` present but required tier `{required_tier}` is not active in `provenance.tiers_active`")]
    InactiveTierObservation {
        /// Name of the offending observation block.
        block: &'static str,
        /// Required tier name.
        required_tier: &'static str,
    },

    /// Underlying YAML syntax or type deserialization failure (spec §0.1, §0.2).
    #[error("YAML error: {0}")]
    Yaml(String),
}
