use calo_channel::Channel;
use std::io::Read;

#[test]
fn noop_when_unset() {
    std::env::remove_var("CALO_CHANNEL");
    let ch = Channel::from_env();
    assert!(ch.is_noop());
    {
        let _p = ch.phase("main");
        let _r = ch.region("fft");
        ch.metric("steps", 10.0, Some("steps"));
        ch.tag("k", "v");
    }
}

#[test]
#[cfg(unix)]
fn protocol_lines_nested_regions_sanitization_and_concurrent_threads() {
    use std::os::unix::io::FromRawFd;

    let mut fds = [0i32; 2];
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    assert_eq!(rc, 0);

    let mut read_end = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    let write_end = unsafe { std::fs::File::from_raw_fd(fds[1]) };

    let ch = Channel::from_file(write_end);
    {
        let _p = ch.phase("main");
        {
            let _r1 = ch.region("outer region");
            {
                let _r2 = ch.region("inner\nline");
            }
        }
        ch.metric("steps", 100.0, Some("steps"));
        ch.tag("mode", "fast fft");
    }

    // Spawn 4 concurrent threads emitting region events simultaneously
    let mut handles = Vec::new();
    for t in 0..4 {
        let ch_c = ch.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..20 {
                let _r = ch_c.region(&format!("t{t}_r{i}"));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    drop(ch);

    let mut output = String::new();
    read_end.read_to_string(&mut output).unwrap();
    let lines: Vec<&str> = output.lines().collect();

    assert_eq!(lines[0], "phase main start");
    assert!(lines[1].starts_with("tid=") && lines[1].ends_with(" region outer_region start"));
    assert!(lines[2].starts_with("tid=") && lines[2].ends_with(" region inner_line start"));
    assert!(lines[3].starts_with("tid=") && lines[3].ends_with(" region inner_line end"));
    assert!(lines[4].starts_with("tid=") && lines[4].ends_with(" region outer_region end"));
    assert_eq!(lines[5], "metric steps=100 steps");
    assert_eq!(lines[6], "tag mode=fast_fft");
    assert_eq!(lines[7], "phase main end");

    // Verify all 4*20*2 = 160 concurrent region lines are intact without interleaving
    for line in &lines[8..] {
        assert!(line.starts_with("tid="));
        assert!(line.contains(" region "));
        assert!(line.ends_with(" start") || line.ends_with(" end"));
    }
    assert_eq!(lines.len(), 8 + 160);
}
