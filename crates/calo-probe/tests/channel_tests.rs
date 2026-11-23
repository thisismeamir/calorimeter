//! Integration tests for Task P5: real `calo-channel` client, shell `$CALO_CHANNEL`, and
//! 8-thread × 20,000-line concurrent pipe atomicity.

#![forbid(unsafe_code)]

use std::{
    ffi::CString,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use calo_channel::Channel;

use calo_probe::{
    channel::{ChannelAccumulator, ChannelFifo, RegionMode},
    spawn::{spawn_gated, ChildSetup, NumaPolicy, Redirect},
};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn temp_dir(label: &str) -> PathBuf {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("calo_p5_{}_{}_{label}", std::process::id(), id));
    fs::create_dir_all(&p).expect("create temp dir");
    p
}

#[test]
fn real_calo_channel_client_round_trip_with_four_threads() {
    let dir = temp_dir("real_client");
    let mut fifo = ChannelFifo::create(&dir).expect("create FIFO");
    let ch = Channel::from_path(fifo.path()).expect("Channel::from_path");

    {
        let _phase = ch.phase("solve");
        ch.metric("iters", 42.0, Some("count"));
        ch.tag("solver", "cg");

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let c = ch.clone();
                thread::spawn(move || {
                    let _outer = c.region("outer");
                    let _inner = c.region("inner");
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }
    drop(ch);

    let mut events = Vec::new();
    fifo.close_and_drain(1_000_000_000, &mut events).unwrap();

    let mut acc = ChannelAccumulator::new(RegionMode::Events);
    for ev in events {
        acc.apply(ev);
    }
    acc.finalize_at_exit(2_000_000_000);

    assert_eq!(acc.protocol_errors, 0);
    assert_eq!(acc.phases["solve"].count, 1);
    assert_eq!(acc.reported["iters"].value, 42.0);
    assert_eq!(acc.reported["iters"].unit.as_deref(), Some("count"));
    assert_eq!(acc.tags["solver"], "cg");
    assert_eq!(acc.region_trees.len(), 4);
    for tree in acc.region_trees.values() {
        assert_eq!(tree["outer"].calls, 1);
        assert_eq!(tree["outer"].children["inner"].calls, 1);
    }

    drop(fifo);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn shell_workload_writes_phases_via_calo_channel_env() {
    let dir = temp_dir("shell_channel");
    let mut fifo = ChannelFifo::create(&dir).expect("create FIFO");

    let mut env = calo_probe::sys::spawn::current_env_cstrings();
    env.push(
        CString::new(format!("CALO_CHANNEL={}", fifo.path().display())).expect("CALO_CHANNEL"),
    );
    let setup = ChildSetup {
        argv: vec![
            CString::new("sh").unwrap(),
            CString::new("-c").unwrap(),
            CString::new(
                "printf 'phase init start\\n' > \"$CALO_CHANNEL\"; printf 'phase init end\\n' > \"$CALO_CHANNEL\"",
            )
            .unwrap(),
        ],
        env,
        cwd: None,
        cpu_list: None,
        numa: NumaPolicy::None,
        rlimits: Vec::new(),
        stdout: Redirect::Null,
        stderr: Redirect::Null,
    };

    let mut child = spawn_gated(&setup).unwrap();
    child.go().unwrap();
    let (status, _ru) = child.wait4().unwrap();
    assert_eq!(status.code, Some(0));

    let mut events = Vec::new();
    fifo.close_and_drain(1_000_000_000, &mut events).unwrap();
    let mut acc = ChannelAccumulator::new(RegionMode::Events);
    for ev in events {
        acc.apply(ev);
    }
    assert_eq!(acc.protocol_errors, 0);
    assert_eq!(acc.phases["init"].count, 1);

    drop(fifo);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn eight_threads_twenty_thousand_lines_atomic_concurrent_drain() {
    let dir = temp_dir("atomicity");
    let mut fifo = ChannelFifo::create(&dir).expect("create FIFO");
    let fifo_path = fifo.path().to_path_buf();
    let done = Arc::new(AtomicBool::new(false));

    let writers: Vec<_> = (1..=8u64)
        .map(|tid| {
            let p = fifo_path.clone();
            thread::spawn(move || {
                let mut f = OpenOptions::new().write(true).open(&p).expect("open fifo");
                for i in 0..10_000u64 {
                    let s = format!("tid={tid} region step_{:05}_padding_for_100b_line_length_check_0123456789 start\n", i);
                    f.write_all(s.as_bytes()).unwrap();
                    let e = format!("tid={tid} region step_{:05}_padding_for_100b_line_length_check_0123456789 end\n", i);
                    f.write_all(e.as_bytes()).unwrap();
                }
            })
        })
        .collect();

    let done_clone = done.clone();
    let join_writers = thread::spawn(move || {
        for w in writers {
            w.join().unwrap();
        }
        done_clone.store(true, Ordering::Release);
    });

    let mut events = Vec::with_capacity(160_000);
    while !done.load(Ordering::Acquire) {
        let n = fifo.drain_ready(1_000, &mut events).unwrap();
        if n == 0 {
            thread::sleep(Duration::from_micros(200));
        }
    }
    join_writers.join().unwrap();
    fifo.close_and_drain(2_000, &mut events).unwrap();

    let stats = fifo.stats();
    assert_eq!(stats.protocol_errors, 0);
    assert_eq!(stats.bad_utf8, 0);
    assert_eq!(stats.too_long, 0);
    assert_eq!(events.len(), 160_000);

    drop(fifo);
    let _ = fs::remove_dir_all(&dir);
}
