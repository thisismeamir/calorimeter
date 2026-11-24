use calo_core::flame::{
    from_folded, render_diff_flamegraph, render_flamegraph, to_folded, xml_escape, FlameOpts,
    FlameTree, StackTable,
};
use calo_core::symbols::Frame;

fn mk_frame(name: &str, class: Option<&str>) -> Frame {
    Frame {
        name: name.to_string(),
        class: class.map(str::to_string),
        file: None,
        line: None,
    }
}

#[test]
fn folded_round_trip_and_identical_stack_merge() {
    let mut tbl = StackTable::new();
    tbl.push(
        vec![
            mk_frame("main", None),
            mk_frame("MPI_Allreduce", Some("mpi")),
        ],
        10,
        Some(0),
    );
    tbl.push(
        vec![
            mk_frame("main", None),
            mk_frame("MPI_Allreduce", Some("mpi")),
        ],
        15,
        Some(0),
    );
    tbl.push(
        vec![mk_frame("main", None), mk_frame("step_evolve", None)],
        75,
        Some(0),
    );

    let folded = to_folded(&tbl);
    assert_eq!(folded, "main;MPI_Allreduce 25\nmain;step_evolve 75\n");

    let reparsed = from_folded(&folded).unwrap();
    assert_eq!(to_folded(&reparsed), folded);
}

#[test]
fn pruning_preserves_root_total_and_deterministic_svg() {
    let folded = "main;hot 999\nmain;tiny 1\n";
    let tbl = from_folded(folded).unwrap();
    // 1 / 1000 = 0.1% -> with min_pct = 0.5%, `tiny` is pruned, `root.total_weight` remains 1000
    let tree = FlameTree::from_table(&tbl, 0.5, false);
    assert_eq!(tree.root.total_weight, 1000);
    assert!(!tree.root.children["main"].children.contains_key("tiny"));

    let opts = FlameOpts {
        highlight: Some("hot".to_string()),
        title: Some("Profile".to_string()),
        ..FlameOpts::default()
    };
    let svg1 = render_flamegraph(&tree, &opts).unwrap();
    let svg2 = render_flamegraph(&tree, &opts).unwrap();
    assert_eq!(svg1, svg2);
    assert!(svg1.contains("viewBox="));
    assert!(!svg1.contains("<script"));
    assert!(svg1.contains("#7017FF"));
}

#[test]
fn diff_flamegraph_added_removed_changed_and_xml_escaping() {
    let before_tbl = from_folded("main;old_only 50\nmain;shared 50\n").unwrap();
    let after_tbl = from_folded("main;shared 80\nmain;new_added 20\n").unwrap();

    let before_tree = FlameTree::from_table(&before_tbl, 0.0, false);
    let after_tree = FlameTree::from_table(&after_tbl, 0.0, false);

    let svg = render_diff_flamegraph(&before_tree, &after_tree, &FlameOpts::default()).unwrap();
    assert!(svg.contains("Removed (only in before): old_only"));
    assert!(svg.contains("shared"));
    assert!(svg.contains("new_added"));
    assert!(svg.contains("#7017FF"));

    // XML escaping check
    assert_eq!(xml_escape("foo<T & U>\"bar\""), "foo<T & U>\"bar\"");

    // Golden structure check on tiny fixed tree
    let tiny_tbl = from_folded("root;leaf 10\n").unwrap();
    let tiny_tree = FlameTree::from_table(&tiny_tbl, 0.0, false);
    let tiny_svg = render_flamegraph(&tiny_tree, &FlameOpts::default()).unwrap();
    assert!(tiny_svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 1200 "));
    assert!(tiny_svg.contains("IBM Plex Sans"));
    assert!(tiny_svg.ends_with("</svg>\n"));
}
