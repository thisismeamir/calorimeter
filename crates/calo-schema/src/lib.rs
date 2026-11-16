#![forbid(unsafe_code)]

pub mod common;
pub mod discovery;
pub mod error;
pub mod inventory;
pub mod manifest;
pub mod record;
pub mod runspec;
pub mod suite;
pub mod table;

pub use common::{
    CallgraphMethod, Confidence, CpuTier, GpuTier, Mode, MpiTier, RegionStorageMode, ScalarValue,
    ShimInject, Status, Tier, TiersActive, Warning, WarningCode,
};
pub use discovery::Discovery;
pub use error::SchemaError;
pub use inventory::Inventory;
pub use manifest::Manifest;
pub use record::{Counter, Record, RegionNode, Regions, ThreadRegions};
pub use runspec::RunSpec;
pub use suite::Suite;
pub use table::{ColumnDef, ColumnType, Sidecar, TableHeader};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Isolated YAML backend wrapper (spec §0.1).
pub mod yaml {
    pub use serde_yaml_ng::{from_str, to_string, to_value, from_value, Error, Mapping, Value};
}

/// Trait implemented by every top-level Calorimeter YAML document (spec §0.2).
pub trait SchemaDoc: Sized {
    /// Expected `schema:` identifier string (e.g., `"calo.record/2"`).
    const SCHEMA: &'static str;

    /// Returns the `schema` field stored on this document instance.
    fn schema_str(&self) -> &str;

    /// Validates the `schema:` version header and structural invariants (spec §0.2).
    fn check(&self) -> Result<(), SchemaError> {
        if self.schema_str() != Self::SCHEMA {
            return Err(SchemaError::VersionMismatch {
                expected: Self::SCHEMA.to_string(),
                found: self.schema_str().to_string(),
            });
        }
        Ok(())
    }
}

/// Minimal probe header used to validate `schema:` prior to full deserialization (spec §0.2).
#[derive(Deserialize)]
struct RawHeader {
    schema: Option<String>,
}

/// Parses a YAML string into a typed `SchemaDoc`, checking `schema:` before full deserialization (spec §0.2).
pub fn from_yaml_str<T: SchemaDoc + DeserializeOwned>(s: &str) -> Result<T, SchemaError> {
    let header: RawHeader = yaml::from_str(s).map_err(|e| SchemaError::Yaml(e.to_string()))?;
    let found = header.schema.ok_or(SchemaError::MissingSchemaHeader)?;
    if found != T::SCHEMA {
        return Err(SchemaError::VersionMismatch {
            expected: T::SCHEMA.to_string(),
            found,
        });
    }
    let doc: T = yaml::from_str(s).map_err(|e| SchemaError::Yaml(e.to_string()))?;
    doc.check()?;
    Ok(doc)
}

/// Serializes a `SchemaDoc` to a YAML string (spec §0.1, §0.2).
pub fn to_yaml_string<T: SchemaDoc + Serialize>(doc: &T) -> Result<String, SchemaError> {
    doc.check()?;
    yaml::to_string(doc).map_err(|e| SchemaError::Yaml(e.to_string()))
}
