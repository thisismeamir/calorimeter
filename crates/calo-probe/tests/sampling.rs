mod common;
use calo_probe::sys::{record::Event, sample::{Sampler, Source}};
use common::{go, spawn_gated, wait};

#[test]
fn samples_a_busy_child() {
    let (pid, wr) = spawn_gated("i=0; while [ $i -lt 400000 ]; do i=$((i+1)); done");
    let mut s = match Sampler::open(pid, Source::Cycles, 499, 6)
        .or_else(|_| Sampler::open(pid, Source::CpuClock, 499, 6))
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: {e}");
            go(wr);
            wait(pid);
            return;
        }
    };
    go(wr);
    wait(pid);

    let (mut samples, mut lost, mut maps, mut deep) = (0, 0u64, 0, 0);
    let (n, bad) = s
        .drain(|e| match e {
            Event::Sample(x) => {
                assert_ne!(x.ip, 0);
                samples += 1;
                if x.callchain.len() > 1 { deep += 1; }
            }
            Event::Lost { count } => lost += count,
            Event::Mmap2(m) if !m.path.is_empty() => maps += 1,
            _ => {}
        })
        .unwrap();
    eprintln!("events={n} undecodable={bad} samples={samples} deep={deep} maps={maps} lost={lost}");
    assert_eq!(bad, 0);
    assert_eq!(lost, 0);
    assert!(samples >= 50, "samples={samples}");
    assert!(maps > 0, "no MMAP2 records");
}
