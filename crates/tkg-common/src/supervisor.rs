//! Process hygiene and Linux orphan prevention.

use std::process::{Command, ExitStatus};

use command_group::{CommandGroup, GroupChild};

use crate::{
    error::{Result, TkgError},
    limits::PROCESS_KILL_GRACE_WAIT,
};

const SIGTERM_GRACE_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
const POLL_STEP: std::time::Duration = std::time::Duration::from_millis(25);

/// Drop guard ensuring child process groups are forcefully terminated if parent
/// drops.
pub struct ChildGuard {
    child: GroupChild,
}

impl ChildGuard {
    /// Constructs a new `ChildGuard` wrapping the spawned process group.
    #[must_use]
    pub const fn new(child: GroupChild) -> Self {
        Self { child }
    }

    /// Returns the process identifier.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Waits for the child process to complete.
    ///
    /// # Errors
    /// Returns an I/O error if the wait call fails.
    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            _ = self.child.kill();
            _ = self.child.wait();
        }
    }
}

/// Subprocess supervisor enforcing process hygiene and process group
/// termination.
pub struct SubprocessSupervisor;

impl SubprocessSupervisor {
    /// Spawns a command configured in its own isolated process group.
    ///
    /// # Errors
    /// Returns `TkgError::Io` if spawning the process fails.
    pub fn spawn_scoped(mut command: Command) -> Result<ChildGuard> {
        let child = command.group_spawn().map_err(TkgError::Io)?;
        Ok(ChildGuard::new(child))
    }

    /// Executes a command with process group supervision and an optional
    /// timeout deadline.
    ///
    /// # Errors
    /// Returns an error string if spawning fails or the command exceeds the
    /// timeout.
    pub fn run_supervised(
        mut command: Command,
        timeout_seconds: u64,
    ) -> std::result::Result<std::process::Output, String> {
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        let child = command
            .group_spawn()
            .map_err(|err| format!("Failed to spawn process: {err}"))?;

        let child_pid = child.id();

        if timeout_seconds > 0 {
            let timeout_dur = std::time::Duration::from_secs(timeout_seconds);
            #[cfg(unix)]
            {
                run_supervised_unix(child, child_pid, timeout_dur)
            }
            #[cfg(not(unix))]
            {
                run_supervised_thread(child, timeout_seconds, timeout_dur)
            }
        } else {
            child
                .wait_with_output()
                .map_err(|err| format!("Failed to read process output: {err}"))
        }
    }
}

#[cfg(unix)]
fn set_nonblocking<T: std::os::fd::AsRawFd>(pipe: &T) {
    use nix::fcntl::{FcntlArg, OFlag, fcntl};
    let fd = pipe.as_raw_fd();
    if let Ok(flags) = fcntl(fd, FcntlArg::F_GETFL) {
        let new_flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
        _ = fcntl(fd, FcntlArg::F_SETFL(new_flags));
    }
}

#[cfg(unix)]
fn drain_nonblocking<R: std::io::Read>(
    pipe: &mut Option<R>,
    buf: &mut Vec<u8>,
) -> std::io::Result<()> {
    let Some(reader) = pipe.as_mut() else {
        return Ok(());
    };
    let mut chunk = [0_u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => {
                *pipe = None;
                return Ok(());
            },
            Ok(n) => {
                if let Some(slice) = chunk.get(..n) {
                    buf.extend_from_slice(slice);
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {},
            Err(err) => {
                *pipe = None;
                return Err(err);
            },
        }
    }
}

#[cfg(unix)]
fn signal_process_group(child_pid: u32, signal: nix::sys::signal::Signal) {
    if let Ok(pgid) = i32::try_from(child_pid)
        && pgid > 0
    {
        let pid = nix::unistd::Pid::from_raw(pgid);
        _ = nix::sys::signal::killpg(pid, signal);
        _ = nix::sys::signal::kill(pid, signal);
    }
}

#[cfg(unix)]
fn terminate_process_group(child: &mut GroupChild, child_pid: u32) {
    // Stage 1: Graceful SIGTERM to the entire process group
    signal_process_group(child_pid, nix::sys::signal::Signal::SIGTERM);
    let term_deadline = std::time::Instant::now() + SIGTERM_GRACE_WAIT;
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let now = std::time::Instant::now();
        if now >= term_deadline {
            break;
        }
        std::thread::sleep(POLL_STEP.min(term_deadline.saturating_duration_since(now)));
    }

    // Stage 2: Forceful SIGKILL to the entire process group
    signal_process_group(child_pid, nix::sys::signal::Signal::SIGKILL);
    let kill_deadline = std::time::Instant::now() + PROCESS_KILL_GRACE_WAIT;
    while !matches!(child.try_wait(), Ok(Some(_))) {
        let now = std::time::Instant::now();
        if now >= kill_deadline {
            break;
        }
        std::thread::sleep(
            std::time::Duration::from_millis(5).min(kill_deadline.saturating_duration_since(now)),
        );
    }
}

#[cfg(unix)]
fn poll_pipes(
    stdout_pipe: Option<&std::process::ChildStdout>,
    stderr_pipe: Option<&std::process::ChildStderr>,
    wait: std::time::Duration,
) {
    use nix::poll::{PollFd, PollFlags, poll};

    let mut fds = Vec::new();
    if let Some(out) = stdout_pipe {
        fds.push(PollFd::new(out, PollFlags::POLLIN));
    }
    if let Some(err) = stderr_pipe {
        fds.push(PollFd::new(err, PollFlags::POLLIN));
    }
    if fds.is_empty() {
        std::thread::sleep(wait);
        return;
    }
    let timeout_ms = i32::try_from(wait.as_millis()).unwrap_or(25);
    _ = poll(&mut fds, timeout_ms);
}

#[cfg(unix)]
fn run_supervised_unix(
    mut child: GroupChild,
    child_pid: u32,
    timeout_dur: std::time::Duration,
) -> std::result::Result<std::process::Output, String> {
    let mut stdout_pipe = child.inner().stdout.take();
    let mut stderr_pipe = child.inner().stderr.take();
    if let Some(ref p) = stdout_pipe {
        set_nonblocking(p);
    }
    if let Some(ref p) = stderr_pipe {
        set_nonblocking(p);
    }

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    let deadline = std::time::Instant::now() + timeout_dur;

    loop {
        drain_nonblocking(&mut stdout_pipe, &mut stdout_buf)
            .map_err(|err| format!("Failed to read process stdout: {err}"))?;
        drain_nonblocking(&mut stderr_pipe, &mut stderr_buf)
            .map_err(|err| format!("Failed to read process stderr: {err}"))?;

        match child.try_wait() {
            Ok(Some(status)) => {
                // Drain remaining buffered data with a bounded grace window so
                // leaked grandchild FDs (e.g. via `setsid`) cannot hang EOF.
                let drain_deadline = std::time::Instant::now() + PROCESS_KILL_GRACE_WAIT;
                while (stdout_pipe.is_some() || stderr_pipe.is_some())
                    && std::time::Instant::now() < drain_deadline
                {
                    let prev_len = stdout_buf.len() + stderr_buf.len();
                    _ = drain_nonblocking(&mut stdout_pipe, &mut stdout_buf);
                    _ = drain_nonblocking(&mut stderr_pipe, &mut stderr_buf);
                    if stdout_pipe.is_none() && stderr_pipe.is_none() {
                        break;
                    }
                    if stdout_buf.len() + stderr_buf.len() == prev_len {
                        let rem =
                            drain_deadline.saturating_duration_since(std::time::Instant::now());
                        poll_pipes(
                            stdout_pipe.as_ref(),
                            stderr_pipe.as_ref(),
                            rem.min(std::time::Duration::from_millis(10)),
                        );
                    }
                }
                return Ok(std::process::Output {
                    status,
                    stdout: stdout_buf,
                    stderr: stderr_buf,
                });
            },
            Ok(None) => {},
            Err(err) => {
                terminate_process_group(&mut child, child_pid);
                return Err(format!("Failed while waiting on process: {err}"));
            },
        }

        let now = std::time::Instant::now();
        if now >= deadline {
            terminate_process_group(&mut child, child_pid);
            let secs = timeout_dur.as_secs();
            return Err(format!(
                "Execution timed out after {secs}s and was terminated."
            ));
        }

        let remaining = deadline.saturating_duration_since(now);
        poll_pipes(
            stdout_pipe.as_ref(),
            stderr_pipe.as_ref(),
            remaining.min(POLL_STEP),
        );
    }
}

#[cfg(not(unix))]
fn run_supervised_thread(
    mut child: GroupChild,
    timeout_seconds: u64,
    timeout_dur: std::time::Duration,
) -> std::result::Result<std::process::Output, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let res = child.wait_with_output();
        _ = tx.send(res);
    });

    if let Ok(res) = rx.recv_timeout(timeout_dur) {
        _ = worker.join();
        res.map_err(|err| format!("Failed to read process output: {err}"))
    } else {
        if rx.recv_timeout(PROCESS_KILL_GRACE_WAIT).is_ok() {
            _ = worker.join();
        }
        Err(format!(
            "Execution timed out after {timeout_seconds}s and was terminated."
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn test_timeout_kills_grandchildren_in_process_group() {
        let pid_file = std::env::temp_dir().join(format!("tkg_sup_gc_{}", std::process::id()));
        _ = std::fs::remove_file(&pid_file);

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep 60 & echo $! > {}; wait", pid_file.display()));
        let res = SubprocessSupervisor::run_supervised(cmd, 1);
        assert!(res.is_err(), "command must time out");

        let grandchild: u32 = std::fs::read_to_string(&pid_file)
            .expect("pid file written")
            .trim()
            .parse()
            .expect("numeric pid");
        let proc_path = std::path::PathBuf::from(format!("/proc/{grandchild}"));

        let mut gone = false;
        for _ in 0..50 {
            if !proc_path.exists() {
                gone = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        _ = std::fs::remove_file(&pid_file);
        assert!(gone, "grandchild process {grandchild} leaked after timeout");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn test_detached_grandchild_holding_pipes_does_not_block() {
        let pid_file = std::env::temp_dir().join(format!(
            "tkg_sup_setsid_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        _ = std::fs::remove_file(&pid_file);

        // Spawn a detached grandchild keeping stdout/stderr FDs open for 30s,
        // while the direct shell child prints "hello" and exits immediately.
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!(
            "setsid sleep 30 & echo $! > {}; echo hello",
            pid_file.display()
        ));

        let started = std::time::Instant::now();
        let res = SubprocessSupervisor::run_supervised(cmd, 5);
        let elapsed = started.elapsed();

        if let Ok(raw) = std::fs::read_to_string(&pid_file)
            && let Ok(pid_num) = raw.trim().parse::<i32>()
            && pid_num > 0
        {
            _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid_num),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        _ = std::fs::remove_file(&pid_file);

        let output = res.expect("must return without waiting for setsid grandchild EOF");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "took {elapsed:?}, hung on detached pipe"
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    }
}
