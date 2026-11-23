use calo_channel::Channel;
use std::fs::OpenOptions;
use std::time::Instant;

fn main() {
    let sink = OpenOptions::new().write(true).open("/dev/null");
    let ch = match sink {
        Ok(f) => Channel::from_file(f),
        Err(_) => Channel::default(),
    };

    let iters: u64 = 100_000;
    let start = Instant::now();
    for _ in 0..iters {
        let _r = ch.region("bench_region");
    }
    let elapsed = start.elapsed();
    let ns_per_pair = (elapsed.as_nanos() as f64) / (iters as f64);
    println!("region_overhead: {ns_per_pair:.1} ns per region start+end pair ({iters} iters)");
}
