use calo_core::plot::{
    nice_ticks, render_line, render_plot, render_region_tree, PlotError, PlotSpec, ScaleKind,
};
use calo_core::services::{Column, Frame};
use calo_schema::record::RegionNode;
use calo_schema::ScalarValue;

#[test]
fn line_plot_faceting_units_ci95_and_deterministic_svg() {
    let mut frame = Frame::new();
    frame
        .insert_column("N", Column::Int(vec![64, 128, 256, 64, 128, 256]))
        .unwrap();
    frame
        .insert_column(
            "sites_per_second",
            Column::Float(vec![100.0, 200.0, 400.0, 150.0, 300.0, 600.0]),
        )
        .unwrap();
    frame
        .insert_column(
            "ci_low",
            Column::Float(vec![90.0, 180.0, 360.0, 140.0, 280.0, 560.0]),
        )
        .unwrap();
    frame
        .insert_column(
            "ci_high",
            Column::Float(vec![110.0, 220.0, 440.0, 160.0, 320.0, 640.0]),
        )
        .unwrap();
    frame
        .insert_column(
            "patch",
            Column::Str(vec![
                "none".into(),
                "none".into(),
                "none".into(),
                "fast".into(),
                "fast".into(),
                "fast".into(),
            ]),
        )
        .unwrap();
    frame
        .insert_column(
            "machine",
            Column::Str(vec![
                "hpc-a".into(),
                "hpc-a".into(),
                "hpc-a".into(),
                "hpc-b".into(),
                "hpc-b".into(),
                "hpc-b".into(),
            ]),
        )
        .unwrap();
    frame.set_meta(
        "sites_per_second.unit",
        ScalarValue::Str("sites/s".to_string()),
    );

    let spec = PlotSpec {
        plot_type: "line".to_string(),
        x: Some("N".to_string()),
        y: Some("sites_per_second".to_string()),
        line_by: Some("patch".to_string()),
        facet_by: Some("machine".to_string()),
        x_scale: ScaleKind::Log2,
        y_scale: ScaleKind::Log,
        errorbars: Some("ci95".to_string()),
        title: Some("Throughput Scaling".to_string()),
        color_scale: ScaleKind::Linear,
    };

    let svg1 = render_line(&spec, &frame).unwrap();
    let svg2 = render_line(&spec, &frame).unwrap();
    assert_eq!(svg1, svg2);
    assert!(svg1.contains("sites_per_second (sites/s)"));
    assert!(svg1.contains("hpc-a"));
    assert!(svg1.contains("hpc-b"));
    assert!(svg1.contains("viewBox=\"0 0 840"));
}

#[test]
fn log_scale_rejects_non_positive_and_empty_renders_no_data() {
    let mut bad = Frame::new();
    bad.insert_column("x", Column::Float(vec![1.0, 0.0]))
        .unwrap();
    bad.insert_column("y", Column::Float(vec![10.0, 20.0]))
        .unwrap();

    let spec = PlotSpec {
        plot_type: "line".to_string(),
        x: Some("x".to_string()),
        y: Some("y".to_string()),
        x_scale: ScaleKind::Log,
        ..PlotSpec::default()
    };
    let err = render_plot(&spec, &bad).expect_err("zero on log scale must fail");
    assert!(matches!(err, PlotError::NonPositiveLogValue { axis: "x", .. }));

    let empty = Frame::new();
    let empty_svg = render_plot(&spec, &empty).unwrap();
    assert!(empty_svg.contains("no data"));
}

#[test]
fn other_plot_types_and_tick_edge_cases() {
    // Tick edge cases: single value, tiny range, negative values
    assert_eq!(nice_ticks(5.0, 5.0, 5), vec![5.0]);
    assert!(!nice_ticks(1e-12, 5e-12, 4).is_empty());
    assert!(!nice_ticks(-10.0, -2.0, 4).is_empty());

    // scaling_plot, bar, comm_matrix, time_budget_bar, region_tree, and deferred stubs
    let mut sf = Frame::new();
    sf.insert_column("ranks", Column::Int(vec![1, 2, 4])).unwrap();
    sf.insert_column("parallel_efficiency", Column::Float(vec![1.0, 0.95, 0.85]))
        .unwrap();
    let sc_svg = render_plot(
        &PlotSpec {
            plot_type: "scaling_plot".to_string(),
            ..PlotSpec::default()
        },
        &sf,
    )
    .unwrap();
    assert!(sc_svg.contains("ideal (1.0)"));

    let mut tb = Frame::new();
    tb.insert_column("cpu_compute_s", Column::Float(vec![10.0]))
        .unwrap();
    tb.insert_column("mpi_wait_s", Column::Float(vec![4.0]))
        .unwrap();
    let tb_svg = render_plot(
        &PlotSpec {
            plot_type: "time_budget_bar".to_string(),
            title: Some("Time Budget".to_string()),
            ..PlotSpec::default()
        },
        &tb,
    )
    .unwrap();
    assert!(tb_svg.contains("MPI wait"));
    assert!(tb_svg.contains("#7017FF"));

    let reg_svg = render_region_tree(
        Some("Regions"),
        &[RegionNode {
            name: "step".to_string(),
            calls: 10,
            incl_s: 5.0,
            excl_s: 2.0,
            children: vec![RegionNode {
                name: "fft".to_string(),
                calls: 10,
                incl_s: 3.0,
                excl_s: 3.0,
                children: vec![],
            }],
        }],
    );
    assert!(reg_svg.contains("step"));
    assert!(reg_svg.contains("fft"));

    // Deferred plot types return NotYetImplemented
    assert!(matches!(
        render_plot(
            &PlotSpec {
                plot_type: "roofline".to_string(),
                ..PlotSpec::default()
            },
            &sf
        ),
        Err(PlotError::NotYetImplemented { .. })
    ));
}
