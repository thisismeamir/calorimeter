use calo_core::services::{Column, Frame, Options, ServiceError, ServiceRegistry};
use calo_schema::suite::ScalingKind;

#[test]
fn scaling_fit_exact_strong_and_weak_data() {
    let reg = ServiceRegistry::new();

    // Strong: T = 2.0 + 8.0 / P -> P = [1, 2, 4, 8] -> T = [10.0, 6.0, 4.0, 3.0]
    let mut f_strong = Frame::new();
    f_strong
        .insert_column("ranks", Column::Int(vec![1, 2, 4, 8]))
        .unwrap();
    f_strong
        .insert_column("wall_s", Column::Float(vec![10.0, 6.0, 4.0, 3.0]))
        .unwrap();

    let strong_opts = Options {
        scaling_kind: ScalingKind::Strong,
        scaling_over: "ranks".to_string(),
        ..Options::default()
    };
    let out_s = reg
        .get("scaling_fit")
        .unwrap()
        .run(&f_strong, &strong_opts)
        .unwrap();
    let a = out_s.column("a").unwrap().to_f64_vec("a").unwrap()[0];
    let b = out_s.column("b").unwrap().to_f64_vec("b").unwrap()[0];
    let r2 = out_s.column("r2").unwrap().to_f64_vec("r2").unwrap()[0];
    let sf = out_s
        .column("serial_fraction")
        .unwrap()
        .to_f64_vec("serial_fraction")
        .unwrap()[0];

    assert!((a - 2.0).abs() < 1e-9);
    assert!((b - 8.0).abs() < 1e-9);
    assert!((r2 - 1.0).abs() < 1e-9);
    assert!((sf - 0.2).abs() < 1e-9);

    // Weak: T = 5.0 + 2.0 * ln(P)
    let p_weak = [1.0, 2.0, 4.0, 8.0];
    let t_weak: Vec<f64> = p_weak.iter().map(|&p: &f64| 5.0 + 2.0 * p.ln()).collect();
    let mut f_weak = Frame::new();
    f_weak
        .insert_column("ranks", Column::Float(p_weak.to_vec()))
        .unwrap();
    f_weak
        .insert_column("wall_s", Column::Float(t_weak))
        .unwrap();

    let weak_opts = Options {
        scaling_kind: ScalingKind::Weak,
        scaling_over: "ranks".to_string(),
        ..Options::default()
    };
    let out_w = reg
        .get("scaling_fit")
        .unwrap()
        .run(&f_weak, &weak_opts)
        .unwrap();
    assert!((out_w.column("a").unwrap().to_f64_vec("a").unwrap()[0] - 5.0).abs() < 1e-9);
    assert!((out_w.column("b").unwrap().to_f64_vec("b").unwrap()[0] - 2.0).abs() < 1e-9);
    assert!((out_w.column("r2").unwrap().to_f64_vec("r2").unwrap()[0] - 1.0).abs() < 1e-9);
}

#[test]
fn compare_service_seeded_determinism_and_effect_size() {
    let reg = ServiceRegistry::new();
    let mut f = Frame::new();
    f.insert_column("baseline", Column::Float(vec![10.0, 10.2, 9.8, 10.1]))
        .unwrap();
    f.insert_column("observed", Column::Float(vec![12.0, 12.1, 11.9, 12.2]))
        .unwrap();

    let o1 = reg.run_service("compare", &f).unwrap();
    let o2 = reg.run_service("compare", &f).unwrap();
    assert_eq!(o1, o2);

    // Observed is strictly > baseline in all pairs -> Cliff's delta = +1.0
    let delta = o1
        .column("cliffs_delta")
        .unwrap()
        .to_f64_vec("cliffs_delta")
        .unwrap()[0];
    assert!((delta - 1.0).abs() < 1e-9);
}

#[test]
fn mpi_comm_matrix_comm_fraction_and_message_size_profile() {
    let reg = ServiceRegistry::new();

    // 3-rank peer matrix
    let mut peers = Frame::new();
    peers
        .insert_column("src_rank", Column::Int(vec![0, 1, 2]))
        .unwrap();
    peers
        .insert_column("peer_rank", Column::Int(vec![1, 2, 0]))
        .unwrap();
    peers
        .insert_column("msgs", Column::Int(vec![10, 20, 30]))
        .unwrap();
    peers
        .insert_column("bytes", Column::Int(vec![320, 20480, 30_000_000]))
        .unwrap();

    let mat = reg.run_service("comm_matrix", &peers).unwrap();
    // 3x3 dense matrix = 9 rows
    assert_eq!(mat.num_rows(), 9);
    let b = mat.column("bytes").unwrap().to_f64_vec("bytes").unwrap();
    // (0->1) at index 1 = 320, (1->2) at index 5 = 20480, (2->0) at index 6 = 30_000_000
    assert_eq!(b[1], 320.0);
    assert_eq!(b[5], 20480.0);
    assert_eq!(b[6], 30_000_000.0);

    let prof = reg.run_service("message_size_profile", &peers).unwrap();
    assert_eq!(prof.num_rows(), 5);

    // M0 comm_fraction
    let mut cf_m0 = Frame::new();
    cf_m0
        .insert_column("mpi_samples", Column::Int(vec![25]))
        .unwrap();
    cf_m0
        .insert_column("total_samples", Column::Int(vec![100]))
        .unwrap();
    let cf_out = reg.run_service("comm_fraction", &cf_m0).unwrap();
    assert_eq!(
        cf_out
            .column("comm_fraction")
            .unwrap()
            .to_f64_vec("comm_fraction")
            .unwrap(),
        vec![0.25]
    );
}

#[test]
fn gpu_services_kernels_transfers_utilization_and_stubs() {
    let reg = ServiceRegistry::new();

    let mut kernels = Frame::new();
    kernels
        .insert_column(
            "name",
            Column::Str(vec![
                "fft".to_string(),
                "update".to_string(),
                "fft".to_string(),
            ]),
        )
        .unwrap();
    kernels
        .insert_column("start_ns", Column::Int(vec![0, 1000, 3000]))
        .unwrap();
    kernels
        .insert_column("end_ns", Column::Int(vec![1000, 1500, 5000]))
        .unwrap();

    let kh = reg.run_service("kernel_hotspots", &kernels).unwrap();
    assert_eq!(kh.num_rows(), 2);
    // "fft" has 3000ns total > "update" 500ns
    assert_eq!(
        kh.column("name").unwrap(),
        &Column::Str(vec!["fft".to_string(), "update".to_string()])
    );
    assert_eq!(kh.column("calls").unwrap(), &Column::Int(vec![2, 1]));

    let mut memcpy = Frame::new();
    memcpy
        .insert_column("kind", Column::Int(vec![0, 1]))
        .unwrap();
    memcpy
        .insert_column("bytes", Column::Int(vec![1_000_000_000, 500_000_000]))
        .unwrap();
    memcpy
        .insert_column("start_ns", Column::Int(vec![0, 0]))
        .unwrap();
    memcpy
        .insert_column("end_ns", Column::Int(vec![1_000_000_000, 500_000_000]))
        .unwrap();
    let ts = reg.run_service("transfer_summary", &memcpy).unwrap();
    assert_eq!(
        ts.column("bandwidth_gbs")
            .unwrap()
            .to_f64_vec("bandwidth_gbs")
            .unwrap(),
        vec![1.0, 1.0]
    );

    let mut state = Frame::new();
    state
        .insert_column("t_ns", Column::Int(vec![0, 100, 300]))
        .unwrap();
    state
        .insert_column("util_pct", Column::Float(vec![50.0, 100.0, f64::NAN]))
        .unwrap();
    let gu = reg.run_service("gpu_utilization", &state).unwrap();
    assert!(
        (gu.column("mean_util_pct")
            .unwrap()
            .to_f64_vec("mean_util_pct")
            .unwrap()[0]
            - 50.0)
            .abs()
            < 1e-9
    );

    // Empty input returns clean error, never NaN
    let empty = Frame::new();
    assert!(matches!(
        reg.run_service("kernel_hotspots", &empty),
        Err(ServiceError::MissingColumn { .. })
    ));

    // Engineer-owned stubs return NotYetImplemented
    for stub in [
        "hotspots",
        "profile_diff",
        "collective_wait",
        "time_budget",
        "boundness",
        "host_device_overlap",
        "gpu_idle_gaps",
        "overlap",
        "occupancy",
    ] {
        assert!(matches!(
            reg.run_service(stub, &empty),
            Err(ServiceError::NotYetImplemented { .. })
        ));
    }
}
