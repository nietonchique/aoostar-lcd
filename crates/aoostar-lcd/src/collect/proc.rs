//! Subprocess execution with a hard timeout. Every external command the
//! collectors run goes through [`run`], so a wedged tool can never stall the
//! daemon.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Upper bound for captured stdout; every tool used here prints far less.
const MAX_OUTPUT: u64 = 8 * 1024 * 1024;

pub struct Captured {
    /// Exit status was zero. Some tools (smartctl) use non-zero bit masks while
    /// still printing usable output, so callers decide what to do with it.
    pub success: bool,
    pub stdout: String,
}

/// Runs `program`, captures stdout, discards stderr, and kills the child when
/// it does not exit within `timeout`.
pub fn run<S: AsRef<OsStr>>(program: &str, args: &[S], timeout: Duration) -> io::Result<Captured> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("stdout pipe missing"));
    };
    // Drain concurrently so a chatty child cannot block on a full pipe.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = Vec::new();
        let result = (&mut stdout)
            .take(MAX_OUTPUT)
            .read_to_end(&mut buffer)
            .map(|_| buffer);
        let _ = tx.send(result);
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(timed_out(program));
        }
        thread::sleep(Duration::from_millis(5));
    };

    let grace = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(250));
    match rx.recv_timeout(grace) {
        Ok(Ok(bytes)) => Ok(Captured {
            success: status.success(),
            stdout: String::from_utf8_lossy(&bytes).into_owned(),
        }),
        Ok(Err(error)) => Err(error),
        Err(_) => Err(timed_out(program)),
    }
}

fn timed_out(program: &str) -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, format!("{program} timed out"))
}
