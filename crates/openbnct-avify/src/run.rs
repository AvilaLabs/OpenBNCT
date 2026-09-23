// SPDX-License-Identifier: MIT

//! Bounded subprocess orchestration for the Avify engine.
//!
//! Corner evaluations legitimately run tens of minutes each, so the
//! bound here is generous and configurable rather than tight — but it
//! is a bound: on timeout the child is killed *and reaped* (no orphan),
//! stdout/stderr are captured to files in the run directory, and the
//! cancellation path is explicit rather than a dropped handle.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::certificate::AvifyCertificate;
use crate::error::AvifyError;

const DEFAULT_TIMEOUT_S: u64 = 6 * 3600;
const POLL_MS: u64 = 500;

/// `~/…` → `$HOME/…` (Command does no shell expansion).
fn expand_tilde(arg: &str) -> String {
    if let Some(rest) = arg.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return format!("{home}/{rest}");
    }
    arg.to_string()
}

/// How to invoke the engine. Default: `avify-dose` on PATH; override
/// with e.g. `["/path/to/venv/bin/python", "-m", "avify"]`.
#[derive(Debug, Clone)]
pub struct EngineInvocation {
    pub argv0: Vec<String>,
    pub timeout_s: u64,
}

impl Default for EngineInvocation {
    fn default() -> Self {
        Self {
            argv0: vec!["avify-dose".to_string()],
            timeout_s: DEFAULT_TIMEOUT_S,
        }
    }
}

impl EngineInvocation {
    /// `<argv0> --version` — first stdout line, bounded to 10 s.
    /// Returns "unknown" rather than failing: an engine build without
    /// a version flag still verifies.
    pub fn engine_version(&self) -> String {
        let argv0: Vec<String> = self.argv0.iter().map(|a| expand_tilde(a)).collect();
        let mut cmd = Command::new(&argv0[0]);
        cmd.args(&argv0[1..])
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut run = || -> Result<String, AvifyError> {
            let mut child = cmd.spawn()?;
            let status = wait_bounded(&mut child, Duration::from_secs(10))?;
            if !status.success() {
                return Ok("unknown".to_string());
            }
            let mut buf = String::new();
            if let Some(mut out) = child.stdout.take() {
                let _ = out.read_to_string(&mut buf);
            }
            Ok(buf.lines().next().unwrap_or("unknown").trim().to_string())
        };
        run().unwrap_or_else(|_| "unknown".to_string())
    }

    /// `avify-dose verify` (or `<argv0> verify`) — run the corner
    /// evaluations and return the parsed certificate.
    pub fn verify(
        &self,
        voxel_prefix: &Path,
        plan_json: &Path,
        outdir: &Path,
        threads: Option<u32>,
    ) -> Result<VerifyOutcome, AvifyError> {
        std::fs::create_dir_all(outdir)?;
        let argv0: Vec<String> = self.argv0.iter().map(|a| expand_tilde(a)).collect();
        let mut cmd = Command::new(&argv0[0]);
        cmd.args(&argv0[1..])
            .arg("verify")
            .arg(voxel_prefix)
            .arg(plan_json)
            .arg("--outdir")
            .arg(outdir);
        if let Some(t) = threads {
            cmd.arg("--threads").arg(t.to_string());
        }
        let stdout_log = outdir.join("engine-stdout.log");
        let stderr_log = outdir.join("engine-stderr.log");
        cmd.stdout(Stdio::from(std::fs::File::create(&stdout_log)?))
            .stderr(Stdio::from(std::fs::File::create(&stderr_log)?));

        let started = Instant::now();
        let mut child = cmd.spawn()?;
        let status = wait_bounded(&mut child, Duration::from_secs(self.timeout_s))?;
        let elapsed = started.elapsed();

        let code = status.code().unwrap_or(-1);
        if !status.success() {
            let mut tail = String::new();
            if let Ok(mut f) = std::fs::File::open(&stderr_log) {
                let mut buf = String::new();
                let _ = f.read_to_string(&mut buf);
                tail = buf.chars().rev().take(2000).collect::<String>();
                tail = tail.chars().rev().collect();
            }
            return Err(AvifyError::EngineFailed { code, stderr: tail });
        }

        let cert_path = outdir.join("certificate.json");
        if !cert_path.is_file() {
            return Err(AvifyError::Invalid(format!(
                "engine exited 0 but {} was not produced",
                cert_path.display()
            )));
        }
        Ok(VerifyOutcome {
            certificate: AvifyCertificate::load(&cert_path)?,
            certificate_path: cert_path,
            elapsed,
            stdout_log,
            stderr_log,
        })
    }
}

/// Poll the child with a bounded wait; on timeout, kill and reap it so
/// no orphan OpenMC child outlives the connector.
fn wait_bounded(
    child: &mut Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, AvifyError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait(); // reap — no zombie
            return Err(AvifyError::Timeout {
                seconds: timeout.as_secs(),
            });
        }
        std::thread::sleep(Duration::from_millis(POLL_MS));
    }
}

/// What a successful verify returns.
#[derive(Debug)]
pub struct VerifyOutcome {
    pub certificate: AvifyCertificate,
    pub certificate_path: PathBuf,
    pub elapsed: Duration,
    pub stdout_log: PathBuf,
    pub stderr_log: PathBuf,
}
