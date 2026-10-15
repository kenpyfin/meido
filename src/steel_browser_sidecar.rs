//! Auto-start the Steel browser Docker container when `BROWSER_MANAGED=true`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tracing::{info, warn};

use crate::config::Config;

const HEALTH_POLL_INTERVAL_MS: u64 = 500;
const HEALTH_WAIT_MAX_SECS: u64 = 90;
const DOCKER_COMMAND_TIMEOUT_SECS: u64 = 120;
const DEFAULT_STEEL_VOLUME: &str = "finally-a-value-bot-steel-cache";
const DEFAULT_STEEL_PROFILE_VOLUME: &str = "finally-a-value-bot-steel-profile";

pub struct SteelBrowserHandle {
    pub managed_locally: bool,
    pub api_url: String,
}

impl SteelBrowserHandle {
    pub fn inactive() -> Arc<Self> {
        Arc::new(Self {
            managed_locally: false,
            api_url: default_local_steel_api_url(crate::config::default_steel_api_port()),
        })
    }
}

pub fn default_local_steel_api_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn is_local_steel_url(url: &str, port: u16) -> bool {
    let trimmed = url.trim().trim_end_matches('/');
    trimmed.is_empty()
        || trimmed == default_local_steel_api_url(port).trim_end_matches('/')
        || trimmed.starts_with("http://127.0.0.1:")
        || trimmed.starts_with("http://localhost:")
}

fn in_docker() -> bool {
    std::env::var("FINALLY_A_VALUE_BOT_IN_DOCKER").as_deref() == Ok("1")
        || Path::new("/.dockerenv").exists()
}

fn configured_api_url(config: &Config) -> String {
    std::env::var("STEEL_API_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| config.steel_api_url())
}

fn ensure_process_steel_api_url(api_url: &str) {
    if std::env::var("STEEL_API_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_none()
    {
        std::env::set_var("STEEL_API_URL", api_url.trim());
    }
}

async fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<std::process::Output, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        Command::new(program).args(args).output(),
    )
    .await
    .map_err(|_| {
        format!(
            "timed out after {timeout_secs}s: {program} {}",
            args.join(" ")
        )
    })?
    .map_err(|e| format!("failed to run {program}: {e}"))?;
    Ok(output)
}

async fn docker_available() -> bool {
    run_command_with_timeout("docker", &["info"], 10)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Steel Browser health paths (current images use `/v1/health`; older docs said `/api/health`).
fn steel_health_urls(api_url: &str) -> [String; 2] {
    let base = api_url.trim_end_matches('/');
    [format!("{base}/v1/health"), format!("{base}/api/health")]
}

pub async fn probe_steel_health(api_url: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };
    for health_url in steel_health_urls(api_url) {
        match client.get(&health_url).send().await {
            Ok(resp) if resp.status().is_success() => return true,
            _ => {}
        }
    }
    false
}

async fn wait_for_steel_health(api_url: &str) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(HEALTH_WAIT_MAX_SECS);
    loop {
        if probe_steel_health(api_url).await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(HEALTH_POLL_INTERVAL_MS)).await;
    }
}

async fn docker_container_state(container_name: &str) -> Option<String> {
    let output = run_command_with_timeout(
        "docker",
        &[
            "ps",
            "-a",
            "--filter",
            &format!("name=^{container_name}$"),
            "--format",
            "{{.State}}",
        ],
        15,
    )
    .await
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if state.is_empty() {
        None
    } else {
        Some(state)
    }
}

async fn docker_start_container(container_name: &str) -> Result<(), String> {
    let output = run_command_with_timeout("docker", &["start", container_name], 60).await?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "docker start {container_name} failed: {}",
            stderr.trim()
        ))
    }
}

async fn docker_run_container(config: &Config) -> Result<(), String> {
    let api_port = config.steel_api_port;
    let cdp_port = config.steel_cdp_port;
    let container_name = config.steel_docker_container_name.trim();
    let image = config.steel_docker_image.trim();
    let domain = format!("127.0.0.1:{api_port}");
    let cdp_domain = format!("127.0.0.1:{cdp_port}");
    let port_api = format!("{api_port}:3000");
    let port_cdp = format!("{cdp_port}:9223");
    let cache_volume_mount = format!("{DEFAULT_STEEL_VOLUME}:/app/.cache");
    let profile_volume_mount = format!("{DEFAULT_STEEL_PROFILE_VOLUME}:/app/api/user-data-dir");

    let output = run_command_with_timeout(
        "docker",
        &[
            "run",
            "-d",
            "--name",
            container_name,
            "-p",
            &port_api,
            "-p",
            &port_cdp,
            "-v",
            &cache_volume_mount,
            "-v",
            &profile_volume_mount,
            "-e",
            &format!("DOMAIN={domain}"),
            "-e",
            &format!("CDP_DOMAIN={cdp_domain}"),
            image,
        ],
        DOCKER_COMMAND_TIMEOUT_SECS,
    )
    .await?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("docker run failed: {}", stderr.trim()))
    }
}

async fn ensure_steel_container(config: &Config) -> Result<bool, String> {
    if !docker_available().await {
        return Err("docker is not available on PATH".into());
    }

    let container_name = config.steel_docker_container_name.trim();
    match docker_container_state(container_name).await {
        Some(state) if state.starts_with("running") => Ok(false),
        Some(_) => {
            docker_start_container(container_name).await?;
            Ok(true)
        }
        None => {
            docker_run_container(config).await?;
            Ok(true)
        }
    }
}

/// Start (or attach to) the local Steel browser container when managed mode is enabled.
pub async fn bootstrap(config: &Config) -> Arc<SteelBrowserHandle> {
    let api_url = configured_api_url(config);
    let mut managed_locally = false;

    if config.browser_managed && !in_docker() && is_local_steel_url(&api_url, config.steel_api_port)
    {
        if !probe_steel_health(&api_url).await {
            match ensure_steel_container(config).await {
                Ok(started) => {
                    managed_locally = started;
                    if started {
                        info!(
                            "Started Steel browser container {} on ports {} (API) / {} (CDP)",
                            config.steel_docker_container_name,
                            config.steel_api_port,
                            config.steel_cdp_port
                        );
                    } else {
                        info!(
                            "Reusing running Steel browser container {}",
                            config.steel_docker_container_name
                        );
                    }
                }
                Err(e) => {
                    warn!("Steel browser auto-start failed: {e}");
                }
            }
        } else {
            info!("Steel browser API already reachable at {api_url}");
        }
    }

    ensure_process_steel_api_url(&api_url);

    // Only block startup when managed browser mode is on (otherwise /v1 health polls
    // add a useless multi-minute wait before Web UI binds).
    if config.browser_managed {
        let healthy = wait_for_steel_health(&api_url).await;
        if healthy {
            info!("Steel browser ready at {api_url}");
        } else {
            warn!(
                "Steel browser not ready at {api_url}. \
                 Ensure Docker is running and BROWSER_MANAGED ports {} / {} are free.",
                config.steel_api_port, config.steel_cdp_port
            );
        }
    }

    Arc::new(SteelBrowserHandle {
        managed_locally,
        api_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_steel_url_detection() {
        assert!(is_local_steel_url("http://127.0.0.1:13920", 13_920));
        assert!(is_local_steel_url("http://localhost:13920", 13_920));
        assert!(is_local_steel_url("", 13_920));
        assert!(!is_local_steel_url("http://192.168.1.5:13920", 13_920));
    }

    #[test]
    fn default_url_uses_port() {
        assert_eq!(
            default_local_steel_api_url(13_920),
            "http://127.0.0.1:13920"
        );
    }

    #[test]
    fn steel_health_urls_prefer_v1() {
        let urls = steel_health_urls("http://127.0.0.1:13920/");
        assert_eq!(urls[0], "http://127.0.0.1:13920/v1/health");
        assert_eq!(urls[1], "http://127.0.0.1:13920/api/health");
    }
}
