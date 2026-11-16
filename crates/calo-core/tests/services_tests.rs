use calo_core::services::{
    Column, Frame, Options, ServiceError, ServiceLayer, ServiceRegistry, StatsSummaryService,
    Service,
};
use calo_schema::suite::{CiConfig, OutlierConfig, ScalingKind, ServiceConfig};
use calo_schema::ScalarValue;
use std::collections::BTreeMap;

#[test]
fn stats_summary_known_values_deterministic_bootstrap_and_mad_outlier() {
    // Planted outlier: 1000.0 alongside [10.0, 11.0, 12.0, 13.0, 14.0]
    // Without outlier, median = 12.0, mean = 12.0
    let mut frame = Frame::new();
    frame
        .insert_column(
            "wall_s",
            Column::Float(vec![10.0, 11.0, 12.0, 13.0, 14.0, 1000.0]),
        )
        .unwrap();

    let opts = Options {
        metric: "wall_s".to_string(),
        estimator: "median".to_string(),
        ci: CiConfig {
            method: "bootstrap".to_string(),
            level: 0.95,
            resamples: 2000,
            seed: 42,
        },
        outliers: Some(OutlierConfig {
            method: "mad".to_string(),
            threshold: 3.5,
        }),
        ..Options::default()
    };

    let svc = StatsSummaryService;
    let out1 = svc.run(&frame, &opts).unwrap();
    let out2 = svc.run(&frame, &opts).unwrap();

    // Deterministic across runs with fixed seed 42
    assert_eq!(out1, out2);

    // 1000.0 was removed as an outlier, leaving 5 elements with median 12.0 and mean 12.0
    assert_eq!(
        out1.column("outliers_removed").unwrap(),
        &Column::Int(vec![1])
    );
    assert_eq!(out1.column("n").unwrap(), &Column::Int(vec![5]));
    assert_eq!(out1.column("median").unwrap(), &Column::Float(vec![12.0]));
    assert_eq!(out1.column("mean").unwrap(), &Column::Float(vec![12.0]));
    assert_eq!(
        out1.column("bootstrap_seed").unwrap(),
        &Column::Int(vec![42])
    );
    assert_eq!(out1.meta("bootstrap_seed"), Some(&ScalarValue::Int(42)));

    // Verify CI bounds bracket the median (10.0 <= ci_low <= 12.0 <= ci_high <= 14.0)
    let ci_low = out1.column("ci_low").unwrap().to_f64_vec("ci_low").unwrap()[0];
    let ci_high = out1
        .column("ci_high")
        .unwrap()
        .to_f64_vec("ci_high")
        .unwrap()[0];
    assert!((10.0..=12.0).contains(&ci_low));
    assert!((12.0..=14.0).contains(&ci_high));
}

#[test]
fn strong_and_weak_parallel_efficiency_and_speedup() {
    let reg = ServiceRegistry::new();

    // Strong scaling worked example:
    // ranks = [1, 2, 4]
    // wall_s = [100.0, 50.0, 40.0]
    // Speedup = [1.0, 2.0, 2.5]
    // Strong efficiency = T(base)*P(base)/(T*P) = [1.0, 100*1/(50*2)=1.0, 100*1/(40*4)=0.625]
    let mut strong_frame = Frame::new();
    strong_frame
        .insert_column("ranks", Column::Int(vec![1, 2, 4]))
        .unwrap();
    strong_frame
        .insert_column("wall_s", Column::Float(vec![100.0, 50.0, 40.0]))
        .unwrap();

    let strong_opts = Options {
        metric: "wall_s".to_string(),
        scaling_kind: ScalingKind::Strong,
        scaling_over: "ranks".to_string(),
        baseline: BTreeMap::from([("ranks".to_string(), ScalarValue::Int(1))]),
        ..Options::default()
    };

    let speedup_out = reg
        .get("speedup")
        .unwrap()
        .run(&strong_frame, &strong_opts)
        .unwrap();
    assert_eq!(
        speedup_out
            .column("speedup")
            .unwrap()
            .to_f64_vec("speedup")
            .unwrap(),
        vec![1.0, 2.0, 2.5]
    );

    let strong_eff_out = reg
        .get("parallel_efficiency")
        .unwrap()
        .run(&strong_frame, &strong_opts)
        .unwrap();
    assert_eq!(
        strong_eff_out
            .column("parallel_efficiency")
            .unwrap()
            .to_f64_vec("parallel_efficiency")
            .unwrap(),
        vec![1.0, 1.0, 0.625]
    );

    // Weak scaling worked example:
    // ranks = [1, 8, 64]
    // wall_s = [10.0, 10.0, 12.5]
    // Weak efficiency = T(base) / T = [1.0, 1.0, 0.8]
    let mut weak_frame = Frame::new();
    weak_frame
        .insert_column("ranks", Column::Int(vec![1, 8, 64]))
        .unwrap();
    weak_frame
        .insert_column("wall_s", Column::Float(vec![10.0, 10.0, 12.5]))
        .unwrap();

    let weak_opts = Options {
        metric: "wall_s".to_string(),
        scaling_kind: ScalingKind::Weak,
        scaling_over: "ranks".to_string(),
        baseline: BTreeMap::from([("ranks".to_string(), ScalarValue::Int(1))]),
        ..Options::default()
    };

    let weak_eff_out = reg
        .get("parallel_efficiency")
        .unwrap()
        .run(&weak_frame, &weak_opts)
        .unwrap();
    assert_eq!(
        weak_eff_out
            .column("parallel_efficiency")
            .unwrap()
            .to_f64_vec("parallel_efficiency")
            .unwrap(),
        vec![1.0, 1.0, 0.8]
    );
}

#[test]
fn ipc_cache_miss_rate_and_imbalance() {
    let reg = ServiceRegistry::new();

    let mut frame = Frame::new();
    frame
        .insert_column("instructions", Column::Int(vec![2000, 3000]))
        .unwrap();
    frame
        .insert_column("cycles", Column::Int(vec![1000, 1000]))
        .unwrap();
    frame
        .insert_column("cache-misses", Column::Int(vec![20, 60]))
        .unwrap();
    frame
        .insert_column("wall_s", Column::Float(vec![8.0, 12.0]))
        .unwrap();

    let frame = reg.run_service("ipc", &frame).unwrap();
    assert_eq!(
        frame.column("ipc").unwrap().to_f64_vec("ipc").unwrap(),
        vec![2.0, 3.0]
    );

    let frame = reg.run_service("cache_miss_rate", &frame).unwrap();
    assert_eq!(
        frame
            .column("cache_miss_rate")
            .unwrap()
            .to_f64_vec("cache_miss_rate")
            .unwrap(),
        vec![0.01, 0.02]
    );

    // Imbalance over ranks with wall_s = [8.0, 12.0] -> mean = 10.0, max = 12.0, imbalance = 1.2
    let imb_out = reg.run_service("imbalance", &frame).unwrap();
    assert_eq!(
        imb_out
            .column("imbalance")
            .unwrap()
            .to_f64_vec("imbalance")
            .unwrap(),
        vec![1.2]
    );
}

#[test]
fn layered_registry_expression_service_python_stub_and_cycle_rejection() {
    let mut reg = ServiceRegistry::new();

    // Suite layer defines `sites_per_second` (expression) and configures `stats.summary`
    let suite_services = BTreeMap::from([
        (
            "sites_per_second".to_string(),
            ServiceConfig {
                expr: Some("params.N**3 * reported.steps / phase.main".to_string()),
                unit: Some("sites/s".to_string()),
                ..ServiceConfig::default()
            },
        ),
        (
            "stats.summary".to_string(),
            ServiceConfig {
                estimator: Some("mean".to_string()),
                ..ServiceConfig::default()
            },
        ),
        (
            "my.fit".to_string(),
            ServiceConfig {
                python: Some("pkg.mod:fit".to_string()),
                inputs: vec!["params.N".to_string(), "wall_s".to_string()],
                produces: vec!["alpha".to_string()],
                ..ServiceConfig::default()
            },
        ),
    ]);
    reg.apply_layer(ServiceLayer::Suite, &suite_services)
        .unwrap();

    // Report layer overrides `stats.summary` estimator back to `median`
    let report_services = BTreeMap::from([(
        "stats.summary".to_string(),
        ServiceConfig {
            estimator: Some("median".to_string()),
            ..ServiceConfig::default()
        },
    )]);
    reg.apply_layer(ServiceLayer::Report, &report_services)
        .unwrap();

    assert_eq!(reg.layer_for("stats.summary"), Some(ServiceLayer::Report));
    assert_eq!(reg.options_for("stats.summary").estimator, "median");

    // Evaluate `sites_per_second`: N=10, steps=5, phase.main=2.0 -> 1000 * 5 / 2.0 = 2500.0
    let mut f = Frame::new();
    f.insert_column("params.N", Column::Int(vec![10])).unwrap();
    f.insert_column("reported.steps", Column::Float(vec![5.0]))
        .unwrap();
    f.insert_column("phase.main", Column::Float(vec![2.0]))
        .unwrap();

    let out = reg.run_service("sites_per_second", &f).unwrap();
    assert_eq!(
        out.column("sites_per_second").unwrap(),
        &Column::Float(vec![2500.0])
    );
    assert_eq!(
        out.meta("sites_per_second.unit"),
        Some(&ScalarValue::Str("sites/s".to_string()))
    );

    // Python service returns PythonUnsupported
    let py_err = reg.run_service("my.fit", &f).expect_err("python unsupported");
    assert!(matches!(py_err, ServiceError::PythonUnsupported { .. }));

    // Dependency cycle rejection
    let mut cycle_reg = ServiceRegistry::new();
    let cyclic = BTreeMap::from([
        (
            "a_svc".to_string(),
            ServiceConfig {
                expr: Some("b_svc + 1".to_string()),
                ..ServiceConfig::default()
            },
        ),
        (
            "b_svc".to_string(),
            ServiceConfig {
                expr: Some("a_svc * 2".to_string()),
                ..ServiceConfig::default()
            },
        ),
    ]);
    cycle_reg.apply_layer(ServiceLayer::Suite, &cyclic).unwrap();
    let err = cycle_reg
        .execution_order()
        .expect_err("must detect dependency cycle");
    assert!(matches!(err, ServiceError::DependencyCycle { .. }));
}
