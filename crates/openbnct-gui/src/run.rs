//! Bounded child-process execution for the run panel.
//!
//! Native spawns `openbnct` (or a user-chosen binary) with piped output
//! drained on reader threads, a wall-clock deadline, and kill+reap
//! cancellation. The web target keeps the same API as an honest stub —
//! browsers cannot spawn processes.

use std::time::Duration;

/// Default wall-clock bound for a spawned job: 10 minutes.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// A spawned job: output lines stream through the receiver; `poll`
/// reports liveness and enforces the deadline; `cancel` kills + reaps.
pub struct Job {
    #[cfg(not(target_arch = "wasm32"))]
    child: std::process::Child,
    receiver: std::sync::mpsc::Receiver<String>,
    #[cfg(not(target_arch = "wasm32"))]
    deadline: std::time::Instant,
    /// Set once the child exits or is killed — carries the exit code.
    pub exit_code: Option<i32>,
    /// True when the deadline (not the user) ended the job.
    pub timed_out: bool,
}

impl Job {
    /// Spawn `program args…` with stdout+stderr merged into one stream.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn(program: &str, args: &[String], timeout: Duration) -> Result<Self, String> {
        use std::io::{BufRead, BufReader, Read};
        use std::process::{Command, Stdio};

        let mut child = Command::new(program)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("spawn {program:?} failed: {error}"))?;
        let (sender, receiver) = std::sync::mpsc::channel();
        // stdout and stderr are distinct pipe types — one reader thread each.
        let mut streams: Vec<Box<dyn Read + Send>> = Vec::new();
        if let Some(out) = child.stdout.take() {
            streams.push(Box::new(out));
        }
        if let Some(err) = child.stderr.take() {
            streams.push(Box::new(err));
        }
        for stream in streams {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines() {
                    if sender
                        .send(line.unwrap_or_else(|_| "<unreadable line>".into()))
                        .is_err()
                    {
                        break;
                    }
                }
            });
        }
        Ok(Self {
            child,
            receiver,
            deadline: std::time::Instant::now() + timeout,
            exit_code: None,
            timed_out: false,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub fn spawn(_program: &str, _args: &[String], _timeout: Duration) -> Result<Self, String> {
        Err("process execution requires the native build".into())
    }

    /// Drain available output lines, enforce the deadline, and reap a
    /// finished child. Returns true while the job is still running.
    pub fn poll(&mut self, output: &mut Vec<String>, cap: usize) -> bool {
        while let Ok(line) = self.receiver.try_recv() {
            output.push(line);
        }
        if output.len() > cap {
            let drop = output.len() - cap;
            output.drain(..drop);
        }
        if self.exit_code.is_some() {
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            if std::time::Instant::now() >= self.deadline {
                self.timed_out = true;
                self.terminate();
                return false;
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.exit_code = Some(status.code().unwrap_or(-1));
                    // Let reader threads flush the remaining lines.
                    while let Ok(line) = self.receiver.try_recv() {
                        output.push(line);
                    }
                    false
                }
                Ok(None) => true,
                Err(_) => {
                    self.exit_code = Some(-1);
                    false
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// Kill and reap the child — idempotent, safe after natural exit.
    pub fn cancel(&mut self) {
        self.terminate();
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn terminate(&mut self) {
        if self.exit_code.is_none() {
            let _ = self.child.kill();
        }
        match self.child.wait() {
            Ok(status) => {
                self.exit_code = Some(status.code().unwrap_or(-1));
            }
            Err(_) => self.exit_code = Some(-1),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn terminate(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cancellation actually kills + reaps the child — the AGENTS.md
    /// race guard. `sleep` is a bounded, innocuous target.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn cancel_terminates_child() {
        let mut job = Job::spawn("sleep", &["60".to_string()], DEFAULT_TIMEOUT).unwrap();
        assert!(job.poll(&mut Vec::new(), 16));
        job.cancel();
        // kill() → signal termination → no exit code → -1.
        assert_eq!(job.exit_code, Some(-1));
        assert!(!job.poll(&mut Vec::new(), 16));
    }

    /// A finishing child reports its exit status — bounded `true` run.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn finished_job_reports_code() {
        let mut job = Job::spawn("true", &[], DEFAULT_TIMEOUT).unwrap();
        let mut output = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while job.poll(&mut output, 16) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(job.exit_code, Some(0));
    }
}
