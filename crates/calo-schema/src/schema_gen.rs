//! JSON Schema 2020-12 YAML generation from `calo-schema` types (spec §0.2).

use crate::{Discovery, Inventory, Manifest, Record, RunSpec, Suite, TableHeader};
use schemars::{schema_for, JsonSchema};

/// Generates `(filename, yaml_schema_content)` pairs for all Calorimeter schema files (spec §0.2).
pub fn generate_all_schemas() -> Vec<(&'static str, String)> {
    vec![
        ("inventory.schema.yaml", render_schema::<Inventory>()),
        ("suite.schema.yaml", render_schema::<Suite>()),
        ("runspec.schema.yaml", render_schema::<RunSpec>()),
        ("record.schema.yaml", render_schema::<Record>()),
        ("discovery.schema.yaml", render_schema::<Discovery>()),
        ("manifest.schema.yaml", render_schema::<Manifest>()),
        ("table.schema.yaml", render_schema::<TableHeader>()),
    ]
}

fn render_schema<T: JsonSchema>() -> String {
    let root = schema_for!(T);
    crate::yaml::to_string(&root).unwrap_or_default()
}
