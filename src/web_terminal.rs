//! Interactive web terminal: short-lived session tickets, PTY bridge, WebSocket I/O.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket};
use axum::http::{HeaderMap, StatusCode};
use futures_util::{SinkExt, StreamExt};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::background_shell::{in_docker, resolve_shell_workdir};
use crate::config::Config;

const TICKET_TTL_SECS: u64 = 30;
const AUTH_WAIT_SECS: u64 = 15;

#[derive(Clone, Default)]
pub struct TerminalHub {
    inner: Arc<TerminalHubInner>,
}

#[derive(Default)]
struct TerminalHubInner {
    pending: Mutex<HashMap<String, PendingSession>>,
    active: AtomicUsize,
}

struct PendingSession {
    ticket: String,
    cwd: PathBuf,
    expires_at: Instant,
}

impl TerminalHub {
    pub async fn create_session(
        &self,
        config: &Config,
    ) -> Result<serde_json::Value, (StatusCode, String)> {
        if !config.web_terminal_available() {
            return Err(terminal_unavailable_error(config));
        }
        let active = self.inner.active.load(Ordering::SeqCst);
        if active >= config.web_terminal_max_sessions {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                "maximum terminal sessions reached".into(),
            ));
        }

        self.prune_expired().await;

        let session_id = uuid::Uuid::new_v4().to_string();
        let ticket = uuid::Uuid::new_v4().to_string();
        let cwd = resolve_shell_workdir(config, std::path::Path::new("."));
        let expires_at = Instant::now() + Duration::from_secs(TICKET_TTL_SECS);

        self.inner.pending.lock().await.insert(
            session_id.clone(),
            PendingSession {
                ticket: ticket.clone(),
                cwd: cwd.clone(),
                expires_at,
            },
        );

        info!(
            session_id = %session_id,
            cwd = %cwd.display(),
            "web terminal session ticket issued"
        );

        Ok(json!({
            "ok": true,
            "session_id": session_id,
            "ws_ticket": ticket,
            "expires_in_secs": TICKET_TTL_SECS,
            "cwd": cwd.to_string_lossy(),
        }))
    }

    async fn prune_expired(&self) {
        let now = Instant::now();
        let mut guard = self.inner.pending.lock().await;
        guard.retain(|_, pending| pending.expires_at > now);
    }

    async fn consume_ticket(&self, session_id: &str, ticket: &str) -> Option<PathBuf> {
        let now = Instant::now();
        let mut guard = self.inner.pending.lock().await;
        let pending = guard.remove(session_id)?;
        if pending.ticket != ticket || pending.expires_at <= now {
            return None;
        }
        Some(pending.cwd)
    }

    fn try_acquire_active_slot(&self, max: usize) -> bool {
        loop {
            let current = self.inner.active.load(Ordering::SeqCst);
            if current >= max {
                return false;
            }
            if self
                .inner
                .active
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    fn release_active_slot(&self) {
        self.inner.active.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn capabilities_json(config: &Config) -> serde_json::Value {
    json!({
        "web_terminal_enabled": config.web_terminal_enabled,
        "web_terminal_available": config.web_terminal_available(),
        "web_terminal_blocked_reason": terminal_blocked_reason(config),
        "web_terminal_max_sessions": config.web_terminal_max_sessions,
        "web_terminal_idle_timeout_secs": config.web_terminal_idle_timeout_secs,
    })
}

fn terminal_blocked_reason(config: &Config) -> Option<&'static str> {
    if !config.web_terminal_enabled {
        return Some("disabled");
    }
    if in_docker() && !config.web_terminal_allow_in_docker {
        return Some("docker");
    }
    if config
        .web_auth_token
        .as_ref()
        .map(|t| t.trim().is_empty())
        .unwrap_or(true)
    {
        return Some("auth_token_required");
    }
    None
}

fn terminal_unavailable_error(config: &Config) -> (StatusCode, String) {
    match terminal_blocked_reason(config) {
        Some("disabled") => (StatusCode::FORBIDDEN, "web terminal is disabled".into()),
        Some("docker") => (
            StatusCode::FORBIDDEN,
            "web terminal is not available in Docker".into(),
        ),
        Some("auth_token_required") => (
            StatusCode::FORBIDDEN,
            "WEB_AUTH_TOKEN is required for web terminal".into(),
        ),
        _ => (
            StatusCode::FORBIDDEN,
            "web terminal is not available".into(),
        ),
    }
}

pub fn require_terminal_api_auth(
    headers: &HeaderMap,
    config: &Config,
) -> Result<(), (StatusCode, String)> {
    if !config.web_terminal_available() {
        return Err(terminal_unavailable_error(config));
    }
    let expected = config
        .web_auth_token
        .as_deref()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                "WEB_AUTH_TOKEN is required for web terminal".into(),
            )
        })?;

    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();

    if provided == expected {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "unauthorized".into()))
    }
}

#[derive(Debug, Deserialize)]
struct TerminalAuthMessage {
    #[serde(rename = "type")]
    msg_type: String,
    session_id: String,
    ticket: String,
}

#[derive(Debug, Deserialize)]
struct TerminalResizeMessage {
    #[serde(rename = "type")]
    msg_type: String,
    cols: u16,
    rows: u16,
}

struct PtySession {
    writer_tx: mpsc::UnboundedSender<Vec<u8>>,
    resize_tx: mpsc::UnboundedSender<PtySize>,
    output_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    child: Box<dyn portable_pty::Child + Send>,
}

fn build_shell_command(cwd: &PathBuf) -> CommandBuilder {
    let mut cmd = if cfg!(windows) {
        let mut builder = CommandBuilder::new("powershell.exe");
        builder.args(["-NoLogo"]);
        builder
    } else {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/bin/bash".to_string());
        CommandBuilder::new(shell)
    };
    cmd.cwd(cwd);
    cmd.env("TERM", "xterm-256color");
    cmd
}

fn spawn_pty_session(cwd: PathBuf, initial_size: PtySize) -> Result<PtySession, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(initial_size)
        .map_err(|e| format!("openpty failed: {e}"))?;

    let cmd = build_shell_command(&cwd);
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn shell failed: {e}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone pty reader failed: {e}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take pty writer failed: {e}"))?;

    let (out_tx, out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (resize_tx, mut resize_rx) = mpsc::unbounded_channel::<PtySize>();

    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if out_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    warn!("pty read error: {e}");
                    break;
                }
            }
        }
    });

    std::thread::spawn(move || {
        while let Some(bytes) = in_rx.blocking_recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
            let _ = writer.flush();
        }
    });

    let master = pair.master;
    std::thread::spawn(move || {
        while let Some(size) = resize_rx.blocking_recv() {
            let _ = master.resize(size);
        }
    });

    Ok(PtySession {
        writer_tx: in_tx,
        resize_tx,
        output_rx: out_rx,
        child,
    })
}

pub async fn handle_websocket(socket: WebSocket, hub: TerminalHub, config: Arc<Config>) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    if !config.web_terminal_available() {
        let _ = ws_tx
            .send(Message::Text(
                json!({"type":"error","message":"web terminal is not available"}).to_string(),
            ))
            .await;
        let _ = ws_tx.close().await;
        return;
    }

    let auth_deadline = tokio::time::Instant::now() + Duration::from_secs(AUTH_WAIT_SECS);
    let auth_msg = match tokio::time::timeout_at(auth_deadline, ws_rx.next()).await {
        Ok(Some(Ok(msg))) => Some(msg),
        _ => None,
    };

    let Some(Message::Text(text)) = auth_msg else {
        let _ = ws_tx
            .send(Message::Text(
                json!({"type":"error","message":"expected auth message"}).to_string(),
            ))
            .await;
        let _ = ws_tx.close().await;
        return;
    };

    let auth: TerminalAuthMessage = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            let _ = ws_tx
                .send(Message::Text(
                    json!({"type":"error","message":"invalid auth payload"}).to_string(),
                ))
                .await;
            let _ = ws_tx.close().await;
            return;
        }
    };

    if auth.msg_type != "auth" {
        let _ = ws_tx
            .send(Message::Text(
                json!({"type":"error","message":"expected auth message"}).to_string(),
            ))
            .await;
        let _ = ws_tx.close().await;
        return;
    }

    let Some(cwd) = hub.consume_ticket(&auth.session_id, &auth.ticket).await else {
        let _ = ws_tx
            .send(Message::Text(
                json!({"type":"error","message":"invalid or expired session ticket"}).to_string(),
            ))
            .await;
        let _ = ws_tx.close().await;
        return;
    };

    if !hub.try_acquire_active_slot(config.web_terminal_max_sessions) {
        let _ = ws_tx
            .send(Message::Text(
                json!({"type":"error","message":"maximum terminal sessions reached"}).to_string(),
            ))
            .await;
        let _ = ws_tx.close().await;
        return;
    }

    let session_id = auth.session_id.clone();
    info!(session_id = %session_id, cwd = %cwd.display(), "web terminal connected");

    let initial_size = PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };

    let pty = match spawn_pty_session(cwd.clone(), initial_size) {
        Ok(session) => session,
        Err(err) => {
            hub.release_active_slot();
            warn!(session_id = %session_id, error = %err, "failed to spawn pty");
            let _ = ws_tx
                .send(Message::Text(
                    json!({"type":"error","message": err}).to_string(),
                ))
                .await;
            let _ = ws_tx.close().await;
            return;
        }
    };

    let _ = ws_tx
        .send(Message::Text(
            json!({
                "type": "auth_ok",
                "session_id": session_id,
                "cwd": cwd.to_string_lossy(),
            })
            .to_string(),
        ))
        .await;

    let writer_tx = pty.writer_tx.clone();
    let resize_tx = pty.resize_tx.clone();
    let mut child = pty.child;
    let idle_timeout = Duration::from_secs(config.web_terminal_idle_timeout_secs);
    let mut last_activity = Instant::now();
    let mut output_rx = pty.output_rx;

    loop {
        let idle_remaining = idle_timeout.saturating_sub(last_activity.elapsed());
        tokio::select! {
            biased;

            maybe_out = output_rx.recv() => {
                match maybe_out {
                    Some(bytes) => {
                        last_activity = Instant::now();
                        if ws_tx.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }

            maybe_ws = ws_rx.next() => {
                match maybe_ws {
                    Some(Ok(Message::Binary(bytes))) => {
                        last_activity = Instant::now();
                        if writer_tx.send(bytes).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        last_activity = Instant::now();
                        if let Ok(msg) = serde_json::from_str::<TerminalResizeMessage>(&text) {
                            if msg.msg_type == "resize" && msg.cols > 0 && msg.rows > 0 {
                                let _ = resize_tx.send(PtySize {
                                    rows: msg.rows,
                                    cols: msg.cols,
                                    pixel_width: 0,
                                    pixel_height: 0,
                                });
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(payload))) => {
                        last_activity = Instant::now();
                        if ws_tx.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_activity = Instant::now();
                    }
                    Some(Err(_)) => break,
                }
            }

            _ = tokio::time::sleep(idle_remaining) => {
                if last_activity.elapsed() >= idle_timeout {
                    let _ = ws_tx
                        .send(Message::Text(
                            json!({"type":"error","message":"session idle timeout"}).to_string(),
                        ))
                        .await;
                    break;
                }
            }
        }

        if let Ok(Some(status)) = child.try_wait() {
            let code = status.exit_code();
            let _ = ws_tx
                .send(Message::Text(
                    json!({"type":"exit","code": code}).to_string(),
                ))
                .await;
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = ws_tx.close().await;
    hub.release_active_slot();
    info!(session_id = %session_id, "web terminal disconnected");
}
