//! Background `meido start`. The gateway service and `--foreground` stay attached.

use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

const DETACHED_ENV: &str = "MEIDO_DETACHED";
const RESTART_PID_ENV: &str = "MEIDO_RESTART_PID";
const RESTART_EXE_ENV: &str = "MEIDO_RESTART_EXE";
/// 150 × 200 ms. The helper gives up and starts anyway if the old pid is still visible.
const RESTART_WAIT_POLLS: u32 = 150;
const PID_FILE: &str = "meido.pid";
const LISTEN_FILE: &str = "meido.listen";
const DAEMON_LOG: &str = "meido-daemon.log";
const BIND_WAIT: Duration = Duration::from_secs(90);

#[cfg(unix)]
const SIGTERM: i32 = 15;
#[cfg(unix)]
const SIGKILL: i32 = 9;

#[cfg(unix)]
extern "C" {
    fn setsid() -> i32;
    fn kill(pid: i32, sig: i32) -> i32;
    fn flock(fd: i32, op: i32) -> i32;
}

#[cfg(unix)]
const LOCK_EX: i32 = 2;
#[cfg(unix)]
const LOCK_NB: i32 = 4;
#[cfg(unix)]
const LOCK_UN: i32 = 8;

pub enum StartRole {
    /// This process should run the app.
    Run,
    /// A background process was just spawned.
    Background { pid: u32 },
    /// A live pid file already points at Meido.
    AlreadyRunning { pid: u32 },
}

pub fn pid_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(PID_FILE)
}

pub fn parse_pid(text: &str) -> Option<u32> {
    let pid = text.split_whitespace().next()?.parse::<u32>().ok()?;
    (pid > 0).then_some(pid)
}

pub fn web_ui_line(host: &str, port: u16, enabled: bool) -> String {
    if !enabled {
        return "Web UI is disabled.".to_string();
    }
    let host = host.trim();
    let host = if host.is_empty() { "127.0.0.1" } else { host };
    if host == "0.0.0.0" || host == "::" || host == "[::]" {
        format!("Web UI listens on {host}:{port}. On this machine open http://127.0.0.1:{port}.")
    } else if host.contains(':') && !host.starts_with('[') {
        format!("Web UI: http://[{host}]:{port}")
    } else {
        format!("Web UI: http://{host}:{port}")
    }
}

pub enum BoundWait {
    Ready(u16),
    Failed,
    Exited,
    TimedOut,
}

fn listen_file(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(LISTEN_FILE)
}

/// Record the port the web UI actually bound, including a fallback when the
/// configured port was already taken.
pub fn write_bound_port(runtime_dir: &Path, port: u16) -> Result<()> {
    write_listen_record(runtime_dir, &format!("port={port}\n"))
}

pub fn write_bind_failed(runtime_dir: &Path) -> Result<()> {
    write_listen_record(runtime_dir, "failed=1\n")
}

fn write_listen_record(runtime_dir: &Path, extra: &str) -> Result<()> {
    fs::create_dir_all(runtime_dir)
        .with_context(|| format!("Failed to create {}", runtime_dir.display()))?;
    let path = listen_file(runtime_dir);
    let tmp = runtime_dir.join(format!(".{LISTEN_FILE}.tmp"));
    fs::write(&tmp, format!("pid={}\n{extra}", std::process::id()))
        .with_context(|| format!("Failed to write {}", tmp.display()))?;
    fs::rename(&tmp, &path).with_context(|| format!("Failed to record {}", path.display()))?;
    Ok(())
}

pub fn parse_bound_port(text: &str, pid: u32) -> Option<u16> {
    match parse_listen_record(text, pid) {
        Some(ListenRecord::Port(port)) => Some(port),
        _ => None,
    }
}

enum ListenRecord {
    Port(u16),
    Failed,
}

fn parse_listen_record(text: &str, pid: u32) -> Option<ListenRecord> {
    let mut file_pid = None;
    let mut port = None;
    let mut failed = false;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("pid=") {
            file_pid = value.trim().parse::<u32>().ok();
        } else if let Some(value) = line.strip_prefix("port=") {
            port = value.trim().parse::<u16>().ok().filter(|p| *p > 0);
        } else if line.trim() == "failed=1" {
            failed = true;
        }
    }
    if file_pid != Some(pid) {
        return None;
    }
    if let Some(port) = port {
        return Some(ListenRecord::Port(port));
    }
    failed.then_some(ListenRecord::Failed)
}

pub fn bound_port_for_pid(runtime_dir: &Path, pid: u32) -> Option<u16> {
    let text = fs::read_to_string(listen_file(runtime_dir)).ok()?;
    parse_bound_port(&text, pid)
}

pub fn wait_for_bound_port(runtime_dir: &Path, pid: u32) -> BoundWait {
    let started = std::time::Instant::now();
    loop {
        if let Ok(text) = fs::read_to_string(listen_file(runtime_dir)) {
            match parse_listen_record(&text, pid) {
                Some(ListenRecord::Port(port)) => return BoundWait::Ready(port),
                Some(ListenRecord::Failed) => return BoundWait::Failed,
                None => {}
            }
        }
        if !process_alive(pid) {
            return BoundWait::Exited;
        }
        if started.elapsed() >= BIND_WAIT {
            return BoundWait::TimedOut;
        }
        thread::sleep(Duration::from_millis(200));
    }
}

pub fn is_detached_child() -> bool {
    std::env::var(DETACHED_ENV).ok().as_deref() == Some("1")
}

/// Restart this process after a short delay so an in-flight HTTP response can flush.
///
/// A login-service process (`MEIDO_GATEWAY` with the unit installed) is restarted through
/// that service. Any other process exits after a detached helper runs `meido start`.
pub fn schedule_app_restart() {
    thread::spawn(|| {
        thread::sleep(Duration::from_millis(300));
        if supervised_by_login_service() {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            if let Err(e) = crate::gateway::restart_user_gateway_now() {
                eprintln!("meido: app restart via login service failed: {e}");
            }
            return;
        }
        match spawn_restart_helper() {
            Ok(()) => std::process::exit(0),
            Err(e) => eprintln!("meido: app restart failed: {e}"),
        }
    });
}

fn supervised_by_login_service() -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        crate::meido_env::is_present(crate::meido_env::GATEWAY)
            && crate::gateway::user_gateway_service_installed()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        false
    }
}

fn spawn_restart_helper() -> Result<()> {
    let exe = std::env::current_exe().context("Failed to resolve the meido executable")?;
    let cwd = std::env::current_dir().context("Failed to read the working directory")?;
    let pid = std::process::id();
    let mut cmd = restart_helper_command();
    cmd.current_dir(cwd)
        .env(RESTART_PID_ENV, pid.to_string())
        .env(RESTART_EXE_ENV, &exe)
        .env_remove(DETACHED_ENV)
        .env_remove(crate::meido_env::GATEWAY.preferred)
        .env_remove(crate::meido_env::GATEWAY.legacy)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    detach_session(&mut cmd);
    cmd.spawn()
        .context("Failed to spawn the Meido restart helper")?;
    Ok(())
}

#[cfg(unix)]
fn restart_helper_command() -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c").arg(restart_helper_sh());
    cmd
}

#[cfg(unix)]
fn restart_helper_sh() -> String {
    format!(
        "i=0; while kill -0 \"${pid}\" 2>/dev/null && [ \"$i\" -lt {polls} ]; do i=$($(i+1)); sleep 0.2; done; exec \"${exe}\" start",
        pid = RESTART_PID_ENV,
        polls = RESTART_WAIT_POLLS,
        exe = RESTART_EXE_ENV,
    )
}

#[cfg(windows)]
fn restart_helper_command() -> Command {
    let script = restart_helper_ps();
    let mut cmd = Command::new("powershell");
    cmd.arg("-NoProfile").arg("-Command").arg(script);
    cmd
}

#[cfg(windows)]
fn restart_helper_ps() -> String {
    format!(
        "$i = 0; while ($i -lt {RESTART_WAIT_POLLS}) {{ $p = Get-Process -Id $env:{RESTART_PID_ENV} -ErrorAction SilentlyContinue; if (-not $p) {{ break }}; Start-Sleep -Milliseconds 200; $i++ }}; & $env:{RESTART_EXE_ENV} start"
    )
}

/// Decide whether this process runs the app or hands off to a background child.
/// The gateway unit sets `MEIDO_GATEWAY` and must stay in the foreground.
pub fn resolve_start(runtime_dir: &Path, foreground: bool) -> Result<StartRole> {
    if foreground || is_detached_child() || crate::meido_env::is_present(crate::meido_env::GATEWAY)
    {
        return Ok(StartRole::Run);
    }
    match spawn_detached(runtime_dir)? {
        Spawned::Started(pid) => Ok(StartRole::Background { pid }),
        Spawned::Already(pid) => Ok(StartRole::AlreadyRunning { pid }),
    }
}

enum Spawned {
    Started(u32),
    Already(u32),
}

fn spawn_detached(runtime_dir: &Path) -> Result<Spawned> {
    fs::create_dir_all(runtime_dir)
        .with_context(|| format!("Failed to create {}", runtime_dir.display()))?;
    let _ = fs::remove_file(listen_file(runtime_dir));
    let log_dir = runtime_dir.join("logs");
    fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create {}", log_dir.display()))?;

    let path = pid_path(runtime_dir);
    let mut slot = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("Failed to open {}", path.display()))?;
    lock_pid_slot(&slot)?;

    let mut existing = String::new();
    slot.read_to_string(&mut existing)?;
    if let Some(pid) = parse_pid(&existing) {
        if process_alive(pid) {
            unlock_pid_slot(&slot);
            return Ok(Spawned::Already(pid));
        }
    }

    let log_path = log_dir.join(DAEMON_LOG);
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("Failed to open {}", log_path.display()))?;
    let stderr = stdout.try_clone().context("Failed to share daemon log")?;

    let exe = std::env::current_exe().context("Failed to resolve the meido executable")?;
    let cwd = std::env::current_dir().context("Failed to read the working directory")?;
    let mut cmd = Command::new(exe);
    cmd.args(std::env::args().skip(1))
        .env(DETACHED_ENV, "1")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    detach_session(&mut cmd);

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            unlock_pid_slot(&slot);
            return Err(e).context("Failed to start Meido in the background");
        }
    };
    let pid = child.id();
    if let Err(e) = write_pid(&mut slot, pid) {
        let _ = terminate(pid);
        unlock_pid_slot(&slot);
        return Err(e);
    }
    unlock_pid_slot(&slot);
    Ok(Spawned::Started(pid))
}

fn write_pid(slot: &mut std::fs::File, pid: u32) -> Result<()> {
    slot.seek(SeekFrom::Start(0))?;
    slot.set_len(0)?;
    writeln!(slot, "{pid}")?;
    slot.sync_all()?;
    Ok(())
}

/// Stop the pid-file process and any Meido process from this workspace that is
/// still listening on the web UI port (including the default-port fallback range).
/// Returns the pids that were stopped. Prints nothing when none were running.
pub fn stop(runtime_dir: &Path, web_port: u16) -> Result<Vec<u32>> {
    let path = pid_path(runtime_dir);
    let mut pids = Vec::new();
    match fs::read_to_string(&path) {
        Ok(text) => {
            if let Some(pid) = parse_pid(&text) {
                if process_alive(pid) {
                    pids.push(pid);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
    for pid in meido_web_pids(runtime_dir, web_port) {
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    for pid in &pids {
        stop_process(*pid).with_context(|| format!("Failed to stop Meido (pid {pid})"))?;
        println!("Meido stopped (pid {pid}).");
    }
    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(listen_file(runtime_dir));
    Ok(pids)
}

fn stop_process(pid: u32) -> Result<()> {
    terminate(pid).with_context(|| format!("Failed to signal pid {pid}"))?;
    for _ in 0..50 {
        if !process_alive(pid) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));
    }
    force_kill(pid).with_context(|| format!("Failed to kill pid {pid}"))?;
    thread::sleep(Duration::from_millis(200));
    if process_alive(pid) {
        anyhow::bail!("Meido (pid {pid}) is still running.");
    }
    Ok(())
}

fn candidate_web_ports(web_port: u16) -> Vec<u16> {
    let mut ports = vec![web_port];
    if web_port != crate::config::default_web_port() {
        return ports;
    }
    let mut candidate = web_port;
    while candidate.saturating_sub(web_port) < crate::config::WEB_PORT_FALLBACK_SPAN {
        let Some(next) = candidate.checked_add(1) else {
            break;
        };
        ports.push(next);
        candidate = next;
    }
    ports
}

/// Inodes of TCP listeners on `ports`, from a `/proc/net/tcp` table.
fn listen_inodes_in_proc_net(text: &str, ports: &[u16]) -> Vec<u64> {
    let mut inodes = Vec::new();
    for line in text.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 || cols[3] != "0A" {
            continue;
        }
        let Some(port_hex) = cols[1].rsplit_once(':').map(|(_, port)| port) else {
            continue;
        };
        let Ok(port) = u16::from_str_radix(port_hex, 16) else {
            continue;
        };
        if !ports.contains(&port) {
            continue;
        }
        if let Ok(inode) = cols[9].parse::<u64>() {
            if inode != 0 {
                inodes.push(inode);
            }
        }
    }
    inodes
}

fn meido_web_pids(runtime_dir: &Path, web_port: u16) -> Vec<u32> {
    let ports = candidate_web_ports(web_port);
    #[cfg(unix)]
    {
        unix_meido_web_pids(runtime_dir, &ports)
    }
    #[cfg(windows)]
    {
        let _ = runtime_dir;
        windows_meido_web_pids(&ports)
    }
}

#[cfg(unix)]
fn unix_meido_web_pids(runtime_dir: &Path, ports: &[u16]) -> Vec<u32> {
    let mut inodes = listen_inodes_in_proc_net(
        &fs::read_to_string("/proc/net/tcp").unwrap_or_default(),
        ports,
    );
    inodes.extend(listen_inodes_in_proc_net(
        &fs::read_to_string("/proc/net/tcp6").unwrap_or_default(),
        ports,
    ));
    if inodes.is_empty() {
        return Vec::new();
    }
    let mut pids = Vec::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return pids;
    };
    let self_pid = std::process::id();
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == 0 || pid == self_pid {
            continue;
        }
        if !process_owns_socket_inode(pid, &inodes) {
            continue;
        }
        if !is_meido_process(pid) || !process_serves_runtime(pid, runtime_dir) {
            continue;
        }
        pids.push(pid);
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(unix)]
fn process_owns_socket_inode(pid: u32, inodes: &[u64]) -> bool {
    let Ok(entries) = fs::read_dir(format!("/proc/{pid}/fd")) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        let text = target.to_string_lossy();
        let Some(rest) = text.strip_prefix("socket:[") else {
            continue;
        };
        let Some(num) = rest.strip_suffix(']') else {
            continue;
        };
        if let Ok(inode) = num.parse::<u64>() {
            if inodes.contains(&inode) {
                return true;
            }
        }
    }
    false
}

#[cfg(unix)]
fn is_meido_process(pid: u32) -> bool {
    let Ok(bytes) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let first = bytes.split(|byte| *byte == 0).next().unwrap_or_default();
    let Ok(arg) = std::str::from_utf8(first) else {
        return false;
    };
    Path::new(arg)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "meido" || name == "meido.exe")
}

#[cfg(unix)]
fn process_serves_runtime(pid: u32, runtime_dir: &Path) -> bool {
    let Ok(cwd) = fs::read_link(format!("/proc/{pid}/cwd")) else {
        return false;
    };
    let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
    let runtime = fs::canonicalize(runtime_dir).unwrap_or_else(|_| runtime_dir.to_path_buf());
    runtime.starts_with(&cwd)
}

#[cfg(windows)]
fn windows_meido_web_pids(ports: &[u16]) -> Vec<u32> {
    let Ok(output) = Command::new("netstat")
        .args(["-ano", "-p", "TCP"])
        .stdin(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut pids = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 || !cols[3].eq_ignore_ascii_case("LISTENING") {
            continue;
        }
        let Some(port_text) = cols[1].rsplit_once(':').map(|(_, port)| port) else {
            continue;
        };
        let Ok(port) = port_text.parse::<u16>() else {
            continue;
        };
        if !ports.contains(&port) {
            continue;
        }
        let Ok(pid) = cols[4].parse::<u32>() else {
            continue;
        };
        if pid != 0 && pid != std::process::id() && windows_process_is_meido(pid) {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(windows)]
fn windows_process_is_meido(pid: u32) -> bool {
    let Ok(output) = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .stdin(Stdio::null())
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout)
        .to_ascii_lowercase()
        .contains("meido.exe")
}

/// Removes the pid file when this process exits, if it still names us.
pub struct PidGuard {
    path: PathBuf,
    pid: u32,
}

impl PidGuard {
    pub fn install(runtime_dir: &Path) -> Result<Self> {
        fs::create_dir_all(runtime_dir)
            .with_context(|| format!("Failed to create {}", runtime_dir.display()))?;
        let path = pid_path(runtime_dir);
        let pid = std::process::id();
        fs::write(&path, format!("{pid}\n"))
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(Self { path, pid })
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return;
        };
        if parse_pid(&text) == Some(self.pid) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn lock_pid_slot(slot: &std::fs::File) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let rc = unsafe { flock(slot.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }
    let err = std::io::Error::last_os_error();
    if err.kind() == std::io::ErrorKind::WouldBlock {
        anyhow::bail!("Meido is already starting.");
    }
    Err(err).context("Failed to lock the Meido pid file")
}

#[cfg(not(unix))]
fn lock_pid_slot(_slot: &std::fs::File) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn unlock_pid_slot(slot: &std::fs::File) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        let _ = flock(slot.as_raw_fd(), LOCK_UN);
    }
}

#[cfg(not(unix))]
fn unlock_pid_slot(_slot: &std::fs::File) {}

#[cfg(unix)]
fn detach_session(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            if setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn detach_session(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let rc = unsafe { kill(pid as i32, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(output) = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .stdin(Stdio::null())
        .output()
    else {
        return false;
    };
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(unix)]
fn terminate(pid: u32) -> std::io::Result<()> {
    signal(pid, SIGTERM)
}

#[cfg(unix)]
fn force_kill(pid: u32) -> std::io::Result<()> {
    signal(pid, SIGKILL)
}

#[cfg(unix)]
fn signal(pid: u32, sig: i32) -> std::io::Result<()> {
    let rc = unsafe { kill(pid as i32, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn terminate(pid: u32) -> std::io::Result<()> {
    taskkill(pid, false)
}

#[cfg(windows)]
fn force_kill(pid: u32) -> std::io::Result<()> {
    taskkill(pid, true)
}

#[cfg(windows)]
fn taskkill(pid: u32, force: bool) -> std::io::Result<()> {
    let mut cmd = Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string()]);
    if force {
        cmd.arg("/F");
    }
    let status = cmd.stdin(Stdio::null()).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("taskkill exited with {status}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::parse_pid;

    #[test]
    fn parse_pid_accepts_a_single_line() {
        assert_eq!(parse_pid("4242\n"), Some(4242));
        assert_eq!(parse_pid("  7  \n"), Some(7));
    }

    #[test]
    fn parse_pid_rejects_empty_and_zero() {
        assert_eq!(parse_pid(""), None);
        assert_eq!(parse_pid("0\n"), None);
        assert_eq!(parse_pid("pending\n"), None);
    }

    #[test]
    fn parse_bound_port_requires_the_same_pid() {
        let text = "pid=9\nport=10962\n";
        assert_eq!(super::parse_bound_port(text, 9), Some(10962));
        assert_eq!(super::parse_bound_port(text, 8), None);
        assert_eq!(super::parse_bound_port("pid=9\nfailed=1\n", 9), None);
    }

    #[test]
    fn listen_inodes_match_only_listening_web_ports() {
        let table = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:2AD2 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 42 1 0000000000000000 100 0 0 10 0
   1: 00000000:2AD1 00000000:0000 01 00000000:00000000 00:00000000 00000000     0        0 99 1 0000000000000000 100 0 0 10 0
   2: 0100007F:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 7 1 0000000000000000 100 0 0 10 0
";
        let ports = super::candidate_web_ports(10961);
        assert!(ports.contains(&10961));
        assert!(ports.contains(&10962));
        assert!(!ports.contains(&80));
        assert_eq!(super::listen_inodes_in_proc_net(table, &ports), vec![42]);
    }

    #[test]
    fn restart_helper_script_reads_the_exe_from_the_environment() {
        #[cfg(unix)]
        let script = super::restart_helper_sh();
        #[cfg(windows)]
        let script = super::restart_helper_ps();
        assert!(script.contains(super::RESTART_EXE_ENV));
        assert!(script.contains(super::RESTART_PID_ENV));
        assert!(script.contains(&super::RESTART_WAIT_POLLS.to_string()));
        assert!(script.contains("start"));
    }

    #[test]
    fn web_ui_line_uses_loopback_for_every_interface() {
        let line = super::web_ui_line("0.0.0.0", 10961, true);
        assert!(line.contains("http://127.0.0.1:10961"));
        assert_eq!(
            super::web_ui_line("192.168.1.20", 10961, true),
            "Web UI: http://192.168.1.20:10961"
        );
        assert_eq!(
            super::web_ui_line("127.0.0.1", 10961, false),
            "Web UI is disabled."
        );
    }
}
