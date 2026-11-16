mod common;

#[test]
fn counts_cycles_across_exec_and_scales() {
    let Some(a) = common::cycles_for(200_000) else { return };
    let Some(b) = common::cycles_for(400_000) else { return };
    assert!(a > 100_000, "cycles too low: {a}");
    let ratio = b as f64 / a as f64;
    eprintln!("ratio = {ratio:.2} (expect about 2)");
    assert!((1.6..2.4).contains(&ratio), "ratio {ratio}");
}
