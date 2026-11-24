use calo_core::yaml::{
    load_str_with_includes, load_with_includes, to_manifest_resolved, Layer, LayeredBuilder,
    LoadError,
};
use calo_schema::suite::Suite;
use calo_schema::yaml::Value;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_test_dir(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("calo_test_{prefix}_{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn include_order_map_merge_and_list_replace() {
    let dir = temp_test_dir("include_order");
    let first = dir.join("first.yaml");
    let second = dir.join("second.yaml");
    let main = dir.join("suite.yaml");

    fs::write(
        &first,
        r#"
options:
  strict_hardware: false
  order: sequential
  redact: [TOKEN_A, TOKEN_B]
services:
  throughput:
    unit: events/s
"#,
    )
    .unwrap();

    fs::write(
        &second,
        r#"
options:
  order: random
  redact: [TOKEN_C]
services:
  sites:
    unit: sites/s
"#,
    )
    .unwrap();

    fs::write(
        &main,
        r#"
schema: calo.suite/1
include: [first.yaml, second.yaml]
suite:
  name: test_suite
options:
  strict_hardware: true
"#,
    )
    .unwrap();

    let loaded = load_with_includes(&main).expect("load_with_includes must succeed");
    let suite: Suite = calo_schema::yaml::from_value(loaded).expect("must deserialize into Suite");

    let opts = suite.options.expect("options present");
    // Overridden by main suite.yaml
    assert_eq!(opts.strict_hardware, Some(true));
    // Overridden by second.yaml over first.yaml
    assert_eq!(opts.order, Some(calo_schema::suite::RunOrder::Random));
    // Lists replace (second.yaml replaced first.yaml's list)
    assert_eq!(opts.redact, vec!["TOKEN_C".to_string()]);
    // Maps merge recursively (both throughput from first.yaml and sites from second.yaml present)
    assert!(suite.services.contains_key("throughput"));
    assert!(suite.services.contains_key("sites"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn include_cycle_is_detected() {
    let dir = temp_test_dir("include_cycle");
    let a = dir.join("a.yaml");
    let b = dir.join("b.yaml");

    fs::write(&a, "include: [b.yaml]\nsuite:\n  name: a\n").unwrap();
    fs::write(&b, "include: [a.yaml]\nsuite:\n  name: b\n").unwrap();

    let err = load_with_includes(&a).expect_err("cycle must fail");
    match err {
        LoadError::IncludeCycle { chain } => {
            assert!(chain.contains("a.yaml"));
            assert!(chain.contains("b.yaml"));
        }
        other => panic!("expected IncludeCycle, got {other:?}"),
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn anchors_aliases_and_merge_keys_expand() {
    let yaml_src = r#"
schema: calo.suite/1
suite:
  name: anchor_suite
builds:
  base_release: &base_build
    run: "cmake --build build"
    flags: "-O3"
    backend: cuda
  profile:
    <<: *base_build
    flags: "-O3 -g -fno-omit-frame-pointer"
    debug_info: required
"#;

    let dummy_path = PathBuf::from("virtual_suite.yaml");
    let expanded = load_str_with_includes(yaml_src, &dummy_path).expect("must expand anchors");
    let serialized = calo_schema::yaml::to_string(&expanded).expect("must serialize cleanly");

    // Generated YAML must contain no anchors, aliases, or merge keys (spec §0.1)
    assert!(!serialized.contains("&base_build"));
    assert!(!serialized.contains("*base_build"));
    assert!(!serialized.contains("<<:"));

    let suite: Suite = calo_schema::yaml::from_value(expanded).expect("must parse as Suite");
    let profile_build = suite.builds.get("profile").expect("profile build exists");
    assert_eq!(profile_build.run, "cmake --build build");
    assert_eq!(
        profile_build.flags.as_deref(),
        Some("-O3 -g -fno-omit-frame-pointer")
    );
    assert_eq!(
        profile_build.backend,
        Some(calo_schema::suite::BuildBackend::Cuda)
    );
}

#[test]
fn layered_precedence_cli_set_and_origin_tracking() {
    let defaults: Value = calo_schema::yaml::from_str(
        r#"
schema: calo.suite/1
suite:
  name: default_name
options:
  strict_hardware: false
  order: interleaved
"#,
    )
    .unwrap();

    let suite_layer: Value = calo_schema::yaml::from_str(
        r#"
suite:
  name: cosmo
benchmarks:
  - name: scaling
    repeat: 1
    params:
      N: 64
"#,
    )
    .unwrap();

    let overlay_layer: Value = calo_schema::yaml::from_str(
        r#"
options:
  strict_hardware: true
"#,
    )
    .unwrap();

    let mut builder = LayeredBuilder::new();
    builder
        .add_layer(Layer::Default, defaults)
        .add_layer(Layer::Suite, suite_layer)
        .add_layer(Layer::Overlay, overlay_layer)
        .add_cli_set("benchmarks.0.repeat=3")
        .add_cli_set("benchmarks.0.params.N=128");

    let resolved = builder
        .resolve::<Suite>()
        .expect("layered resolve must succeed");
    assert_eq!(resolved.value.suite.name, "cosmo");
    assert_eq!(
        resolved.value.options.as_ref().unwrap().strict_hardware,
        Some(true)
    );
    assert_eq!(resolved.value.benchmarks[0].repeat, Some(3));
    assert_eq!(
        resolved.value.benchmarks[0].params.get("N"),
        Some(&calo_schema::ScalarValue::Int(128))
    );

    let manifest_origins = to_manifest_resolved(&resolved);
    assert_eq!(
        manifest_origins.get("options.order").unwrap().layer,
        Layer::Default
    );
    assert_eq!(
        manifest_origins.get("suite.name").unwrap().layer,
        Layer::Suite
    );
    assert_eq!(
        manifest_origins
            .get("options.strict_hardware")
            .unwrap()
            .layer,
        Layer::Overlay
    );
    assert_eq!(
        manifest_origins.get("benchmarks.0.repeat").unwrap().layer,
        Layer::CliSet
    );
    assert_eq!(
        manifest_origins.get("benchmarks.0.params.N").unwrap().layer,
        Layer::CliSet
    );
}
