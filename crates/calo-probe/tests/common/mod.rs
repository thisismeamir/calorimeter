#![allow(dead_code)]
use calo_probe::sys::perf::Counter;
use perf_event_open_sys::bindings::PERF_COUNT_HW_CPU_CYCLES;
use std::ffi::CString;

/// Fork a child that blocks on a go-pipe, then execs `sh -c <script>`.
/// Returns (pid, go_write_fd). Uses raw fork: Command::spawn() would
/// deadlock because it waits for the child's exec to succeed.
pub fn spawn_gated(script: &str) -> (libc::pid_t, i32) {
    let mut fds = [0i32; 2];
    // SAFETY: fds is a valid 2-element array.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let (rd, wr) = (fds[0], fds[1]);

    // Allocate everything before fork: the child may only call
    // async-signal-safe functions.
    let sh = CString::new("sh").unwrap();
    let a1 = CString::new("-c").unwrap();
    let a2 = CString::new(script).unwrap();
    let argv = [sh.as_ptr(), a1.as_ptr(), a2.as_ptr(), std::ptr::null()];

    // SAFETY: child path below uses only close/read/execvp/_exit.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        unsafe {
            libc::close(wr);
            let mut b = 0u8;
            libc::read(rd, (&mut b as *mut u8).cast(), 1); // wait for go
            libc::close(rd);
            libc::execvp(sh.as_ptr(), argv.as_ptr());
            libc::_exit(127);
        }
    }
    unsafe { libc::close(rd) };
    (pid, wr)
}

pub fn go(wr: i32) {
    // SAFETY: wr is a valid open fd owned by the caller.
    unsafe {
        libc::write(wr, [1u8].as_ptr().cast(), 1);
        libc::close(wr);
    }
}

pub fn wait(pid: libc::pid_t) {
    let mut st = 0;
    // SAFETY: pid is our child.
    unsafe { libc::waitpid(pid, &mut st, 0) };
}

pub fn cycles_for(iters: u64) -> Option<u64> {
    let script = format!("i=0; while [ $i -lt {iters} ]; do i=$((i+1)); done");
    let (pid, wr) = spawn_gated(&script);
    let c = match Counter::open_hw(PERF_COUNT_HW_CPU_CYCLES as u64, pid) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SKIP: perf_event_open failed: {e}");
            go(wr);
            wait(pid);
            return None;
        }
    };
    go(wr);
    wait(pid);
    let r = c.read().expect("read");
    eprintln!("iters={iters} {r:?}");
    assert!(r.time_enabled_ns >= r.time_running_ns);
    assert!(r.time_running_ns > 0);
    Some(r.value)
}
