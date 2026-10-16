use std::path::Path;
use std::process::{Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{CommandWrap, KillOnDrop};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::time::{sleep, timeout};

pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

pub fn shell_command(command: &str) -> CommandSpec {
    if cfg!(target_os = "windows") {
        CommandSpec {
            program: "powershell".to_string(),
            args: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                command.to_string(),
            ],
        }
    } else {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string());
        CommandSpec {
            program: shell,
            args: vec!["-c".to_string(), command.to_string()],
        }
    }
}

pub fn build_command(spec: &CommandSpec, working_dir: Option<&Path>) -> tokio::process::Command {
    build_command_with_env(spec, working_dir, false, None)
}

pub fn build_command_with_env(
    spec: &CommandSpec,
    working_dir: Option<&Path>,
    tool_output_debug: bool,
    git_ceiling_workspace: Option<&Path>,
) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(&spec.program);
    cmd.args(&spec.args);
    if let Some(dir) = working_dir {
        cmd.current_dir(dir);
    }
    apply_tool_output_debug_env(&mut cmd, tool_output_debug);
    if let Some(workspace_root) = git_ceiling_workspace {
        crate::self_repo::apply_git_ceiling_env(&mut cmd, workspace_root);
    }
    cmd
}

/// Sets `TOOL_OUTPUT_DEBUG=1` for workspace PZ/ComfyUI scripts when debug logging is enabled.
pub fn apply_tool_output_debug_env(cmd: &mut tokio::process::Command, tool_output_debug: bool) {
    if tool_output_debug {
        cmd.env("TOOL_OUTPUT_DEBUG", "1");
    }
}

#[derive(Debug)]
pub enum ManagedCommandOutcome {
    Completed(Output),
    TimedOut,
    Cancelled,
    SpawnError(std::io::Error),
}

/// Run a command in a process group / job object so timeout, cancel, or drop
/// terminates the entire descendant tree (not only the shell leader).
pub async fn run_managed_command(
    mut cmd: tokio::process::Command,
    timeout_secs: u64,
    cancel: Option<&Arc<AtomicBool>>,
) -> ManagedCommandOutcome {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut wrap = CommandWrap::from(cmd);
    #[cfg(unix)]
    {
        wrap.wrap(ProcessGroup::leader());
    }
    #[cfg(windows)]
    {
        wrap.wrap(JobObject);
    }
    wrap.wrap(KillOnDrop);

    let child = match wrap.spawn() {
        Ok(c) => c,
        Err(e) => return ManagedCommandOutcome::SpawnError(e),
    };
    // Ensure process-group SIGKILL / job terminate even if this future is dropped
    // by an outer `select!` (MCP cancel) before our timeout/cancel arms run.
    let mut child = KillOnDropGroup { inner: Some(child) };

    let mut stdout_pipe = child.inner.as_mut().expect("child present").stdout().take();
    let mut stderr_pipe = child.inner.as_mut().expect("child present").stderr().take();

    let stdout_fut = read_to_end_opt(&mut stdout_pipe);
    let stderr_fut = read_to_end_opt(&mut stderr_pipe);

    let wait_and_collect = async {
        let child_ref = child.inner.as_mut().expect("child present");
        let (status, stdout, stderr) = tokio::try_join!(child_ref.wait(), stdout_fut, stderr_fut)?;
        Ok::<Output, std::io::Error>(Output {
            status,
            stdout,
            stderr,
        })
    };

    let stop_reason = tokio::select! {
        biased;
        _ = wait_until_cancelled(cancel) => Some(ManagedCommandOutcome::Cancelled),
        _ = sleep(Duration::from_secs(timeout_secs.max(1))) => {
            Some(ManagedCommandOutcome::TimedOut)
        }
        result = wait_and_collect => {
            // Successful completion: disarm kill-on-drop so we don't SIGKILL a dead tree.
            let _ = child.inner.take();
            return match result {
                Ok(output) => ManagedCommandOutcome::Completed(output),
                Err(e) => ManagedCommandOutcome::SpawnError(e),
            };
        }
    };

    if let Some(child_ref) = child.inner.as_mut() {
        let _ = child_ref.start_kill();
        let kill_wait = timeout(Duration::from_secs(5), async {
            let _ = child_ref.wait().await;
            let _ = read_to_end_opt(&mut stdout_pipe).await;
            let _ = read_to_end_opt(&mut stderr_pipe).await;
        })
        .await;
        if kill_wait.is_err() {
            tracing::warn!("managed command did not exit within 5s after kill");
        }
        let _ = child.inner.take();
    }

    stop_reason.unwrap_or(ManagedCommandOutcome::TimedOut)
}

/// Calls `start_kill()` (process-group / job) when dropped without being disarmed.
struct KillOnDropGroup {
    inner: Option<Box<dyn process_wrap::tokio::ChildWrapper>>,
}

impl Drop for KillOnDropGroup {
    fn drop(&mut self) {
        if let Some(mut child) = self.inner.take() {
            let _ = child.start_kill();
        }
    }
}

async fn read_to_end_opt<A: AsyncRead + Unpin>(io: &mut Option<A>) -> std::io::Result<Vec<u8>> {
    let mut vec = Vec::new();
    if let Some(reader) = io.as_mut() {
        reader.read_to_end(&mut vec).await?;
    }
    Ok(vec)
}

async fn wait_until_cancelled(cancel: Option<&Arc<AtomicBool>>) {
    let Some(flag) = cancel else {
        std::future::pending::<()>().await;
        return;
    };
    while !flag.load(Ordering::SeqCst) {
        sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_command_shape() {
        let spec = shell_command("echo hello");
        assert!(!spec.program.is_empty());
        assert!(!spec.args.is_empty());
    }

    #[tokio::test]
    async fn managed_command_kills_descendant_on_timeout() {
        if cfg!(windows) {
            return;
        }
        let marker =
            std::env::temp_dir().join(format!("favb_managed_cmd_{}", uuid::Uuid::new_v4()));
        let marker_str = marker.to_string_lossy().replace('\'', "");
        // Child sleeps longer than the timeout; parent shell waits on it.
        let cmd_str = format!("sleep 30 & CHILD=$!; echo $CHILD > '{marker_str}'; wait $CHILD");
        let spec = shell_command(&cmd_str);
        let cmd = build_command(&spec, None);
        let started = std::time::Instant::now();
        let outcome = run_managed_command(cmd, 1, None).await;
        assert!(matches!(outcome, ManagedCommandOutcome::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(8));

        // Give the kernel a moment, then ensure the recorded child is gone.
        sleep(Duration::from_millis(200)).await;
        if let Ok(pid_text) = std::fs::read_to_string(&marker) {
            if let Ok(pid) = pid_text.trim().parse::<i32>() {
                let still_alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
                assert!(
                    !still_alive,
                    "descendant pid {pid} should be dead after process-group kill"
                );
            }
        }
        let _ = std::fs::remove_file(&marker);
    }

    #[tokio::test]
    async fn managed_command_respects_cancel_flag() {
        if cfg!(windows) {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let spec = shell_command("sleep 30");
        let cmd = build_command(&spec, None);
        let cancel_clone = cancel.clone();
        let join =
            tokio::spawn(async move { run_managed_command(cmd, 60, Some(&cancel_clone)).await });
        sleep(Duration::from_millis(100)).await;
        cancel.store(true, Ordering::SeqCst);
        let outcome = timeout(Duration::from_secs(5), join)
            .await
            .expect("join timed out")
            .expect("task panicked");
        assert!(matches!(outcome, ManagedCommandOutcome::Cancelled));
    }
}
