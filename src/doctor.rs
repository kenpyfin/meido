use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::Config;
use crate::mcp::McpConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

impl CheckStatus {
    fn as_label(self) -> &'static str {
        match self {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
        }
    }

    fn as_emoji(self) -> &'static str {
        match self {
            CheckStatus::Pass => "✅",
            CheckStatus::Warn => "⚠️",
            CheckStatus::Fail => "❌",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub id: String,
    pub title: String,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub platform: String,
    pub arch: String,
    pub in_wsl: bool,
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    fn new() -> Self {
        Self {
            platform: current_platform().to_string(),
            arch: std::env::consts::ARCH.to_string(),
            in_wsl: is_wsl(),
            checks: Vec::new(),
        }
    }

    fn push(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        status: CheckStatus,
        detail: impl Into<String>,
        fix: Option<String>,
    ) {
        self.checks.push(DoctorCheck {
            id: id.into(),
            title: title.into(),
            status,
            detail: detail.into(),
            fix,
        });
    }

    fn summary(&self) -> (usize, usize, usize) {
        let mut pass = 0usize;
        let mut warn = 0usize;
        let mut fail = 0usize;
        for check in &self.checks {
            match check.status {
                CheckStatus::Pass => pass += 1,
                CheckStatus::Warn => warn += 1,
                CheckStatus::Fail => fail += 1,
            }
        }
        (pass, warn, fail)
    }
}

pub fn run_cli(args: &[String]) -> anyhow::Result<()> {
    let json_output = args.iter().any(|a| a == "--json");
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "Usage: meido doctor [--json]\n\nChecks PATH, shell/runtime dependencies, browser automation prerequisites, and MCP command dependencies."
        );
        return Ok(());
    }

    let report = build_report();

    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }

    let (_, _, fail) = report.summary();
    if fail > 0 {
        std::process::exit(2);
    }

    Ok(())
}

fn build_report() -> DoctorReport {
    let mut report = DoctorReport::new();

    report.push(
        "env.platform",
        "Platform",
        CheckStatus::Pass,
        format!(
            "os={} arch={} wsl={}",
            report.platform, report.arch, report.in_wsl
        ),
        None,
    );

    check_config(&mut report);
    check_wecom_credentials(&mut report);
    check_llm_provider_base_url(&mut report);
    check_evaluator_api_key(&mut report);
    check_cost_routing_local(&mut report);
    check_shadow_workspace(&mut report);
    check_legacy_flat_shared(&mut report);
    check_path(&mut report);
    check_shell(&mut report);
    check_node_and_browser(&mut report);
    check_mcp_dependencies(&mut report);
    check_cursor_mcp_bridge(&mut report);
    check_gemini_adk(&mut report);
    check_mempalace(&mut report);

    report
}

fn check_mempalace(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let mode = crate::recall::RecallMode::parse(&config.mempalace.recall_mode);
    if mode == crate::recall::RecallMode::Disabled {
        report.push(
            "recall.mempalace",
            "MemPalace recall",
            CheckStatus::Pass,
            "Recall mode is disabled. MemPalace is not used, and this is not a successful index.",
            None,
        );
        return;
    }
    let managed = crate::mempalace_setup::resolved_stdio(&config);
    if config.mempalace.endpoint.trim().is_empty() && managed.is_none() {
        report.push(
            "recall.mempalace",
            "MemPalace recall",
            CheckStatus::Fail,
            format!(
                "Recall mode is {} but the MemPalace MCP is not installed and neither MEMPALACE_LIGHT_MCP_COMMAND nor MEMPALACE_LIGHT_MCP_ENDPOINT is set. Recall falls back explicitly; it does not claim success.",
                mode.as_str()
            ),
            Some("Start the bot or run gateway install to install the MemPalace MCP, or set MEMPALACE_RECALL_MODE=disabled.".into()),
        );
        return;
    }
    if let Some((command, _)) = &managed {
        if !crate::recall::command_available(command) {
            report.push(
                "recall.mempalace",
                "MemPalace recall",
                CheckStatus::Fail,
                format!(
                    "Recall mode is {mode} but the MCP command is not executable. Doctor did not start the server.",
                    mode = mode.as_str()
                ),
                Some("Re-run gateway install, or fix MEMPALACE_LIGHT_MCP_COMMAND.".into()),
            );
            return;
        }
    }
    let (pending, rejected, freshness) = crate::db::Database::new(&config.runtime_data_dir())
        .ok()
        .and_then(|db| {
            let counts = db.recall_ingest_counts().ok()?;
            let freshness = db
                .recall_checkpoint("last_success")
                .ok()
                .flatten()
                .unwrap_or_else(|| "none".into());
            Some((counts.0, counts.1, freshness))
        })
        .unwrap_or((0, 0, "none".into()));
    report.push(
        "recall.mempalace",
        "MemPalace recall",
        CheckStatus::Warn,
        format!(
            "Recall mode is {}. Command or endpoint is configured. Doctor does not open a connection, so this is not a live health check. Ingest pending={pending} rejected={rejected} last_success={freshness}.",
            mode.as_str()
        ),
        None,
    );
}

fn check_gemini_adk(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let runtime_dir_str = config.runtime_data_dir();
    let engine = crate::db::Database::new(&runtime_dir_str)
        .ok()
        .and_then(|db| {
            crate::runtime_toggles::RuntimeToggles::agent_engine_from_app_settings(&db).ok()
        })
        .flatten()
        .unwrap_or_else(|| {
            crate::runtime_toggles::AgentEngine::parse(
                &std::env::var("AGENT_ENGINE").unwrap_or_default(),
            )
        });
    if engine != crate::runtime_toggles::AgentEngine::GeminiAdk {
        return;
    }
    let api_key_ok = crate::gemini_adk::GeminiAdkSettings::api_key_configured(&config);
    if !api_key_ok {
        report.push(
            "gemini_adk.api_key",
            "Gemini ADK API key",
            CheckStatus::Warn,
            "AGENT_ENGINE=gemini_adk but GEMINI_API_KEY/GOOGLE_API_KEY is missing".to_string(),
            Some("Add GEMINI_API_KEY (or GOOGLE_API_KEY) to .env.".to_string()),
        );
    } else {
        report.push(
            "gemini_adk.api_key",
            "Gemini ADK API key",
            CheckStatus::Pass,
            "Gemini API key configured for native ADK engine".to_string(),
            None,
        );
    }
    if let Ok(db) = crate::db::Database::new(&runtime_dir_str) {
        match crate::gemini_adk::profile::load_from_db(&db) {
            Ok(profile) => {
                if let Err(e) = profile.validate() {
                    report.push(
                        "gemini_adk.topology",
                        "Gemini ADK topology",
                        CheckStatus::Warn,
                        format!("Invalid topology: {e}"),
                        Some(
                            "Fix or reset topology in Settings → Agent engine → Gemini ADK."
                                .to_string(),
                        ),
                    );
                } else {
                    report.push(
                        "gemini_adk.topology",
                        "Gemini ADK topology",
                        CheckStatus::Pass,
                        format!(
                            "Topology ok ({} agents, root={})",
                            profile.agents.len(),
                            profile.root_agent_id
                        ),
                        None,
                    );
                }
            }
            Err(e) => {
                report.push(
                    "gemini_adk.topology",
                    "Gemini ADK topology",
                    CheckStatus::Warn,
                    format!("Failed to load topology: {e}"),
                    None,
                );
            }
        }
    }
}

fn check_cursor_mcp_bridge(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let runtime_dir_str = config.runtime_data_dir();
    let engine = crate::db::Database::new(&runtime_dir_str)
        .ok()
        .and_then(|db| {
            crate::runtime_toggles::RuntimeToggles::agent_engine_from_app_settings(&db).ok()
        })
        .flatten()
        .unwrap_or_else(|| {
            crate::runtime_toggles::AgentEngine::parse(
                &std::env::var("AGENT_ENGINE").unwrap_or_default(),
            )
        });
    if engine != crate::runtime_toggles::AgentEngine::Cursor {
        return;
    }
    let endpoint = crate::cursor_mcp_bridge::mcp_endpoint_url(config.web_port);
    if !config.web_enabled {
        report.push(
            "cursor_engine.mcp_bridge",
            "Cursor engine MCP tool bridge",
            CheckStatus::Warn,
            "AGENT_ENGINE=cursor but WEB_ENABLED=false — bot tools are unavailable to Cursor"
                .to_string(),
            Some("Set WEB_ENABLED=true so the loopback MCP endpoint can start.".to_string()),
        );
        return;
    }
    report.push(
        "cursor_engine.mcp_bridge",
        "Cursor engine MCP tool bridge",
        CheckStatus::Pass,
        format!("Loopback MCP expected at {endpoint} (enable in Settings → Cursor)"),
        None,
    );
}

fn check_wecom_credentials(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let env_partial = config.wecom_corp_id.is_some()
        || config.wecom_corp_secret.is_some()
        || config.wecom_callback_token.is_some()
        || config.wecom_encoding_aes_key.is_some()
        || config.wecom_agent_id > 0
        || config.wecom_aibot_id.is_some();
    let runtime_dir = config.runtime_data_dir();
    let inst = crate::db::Database::new(&runtime_dir).ok().and_then(|db| {
        db.get_channel_bot_instance(crate::db::BOT_INSTANCE_WECOM_PRIMARY)
            .ok()
            .flatten()
    });
    if inst.is_none() && !env_partial {
        return;
    }
    let uses_aibot = if let Some(row) = inst.as_ref() {
        let mode = row.wecom_mode.trim().to_ascii_lowercase();
        mode == "aibot"
            || mode == "websocket"
            || (!row.wecom_aibot_id.trim().is_empty() && mode != "callback")
    } else {
        config.wecom_uses_aibot()
    };
    if uses_aibot {
        let (bot_id, secret_set) = if let Some(row) = inst {
            (
                row.wecom_aibot_id.trim().to_string(),
                !row.token.trim().is_empty(),
            )
        } else {
            (
                config.wecom_aibot_id.clone().unwrap_or_default(),
                config
                    .wecom_corp_secret
                    .as_deref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false),
            )
        };
        let mut missing = Vec::new();
        if bot_id.is_empty() {
            missing.push("bot_id");
        }
        if !secret_set {
            missing.push("bot_secret");
        }
        if missing.is_empty() {
            report.push(
                "channel.wecom",
                "WeCom credentials",
                CheckStatus::Pass,
                "WeCom AI Bot long connection configured (no public callback)",
                None,
            );
        } else {
            report.push(
                "channel.wecom",
                "WeCom credentials",
                CheckStatus::Fail,
                format!(
                    "WeCom AI Bot is partially configured; missing: {}",
                    missing.join(", ")
                ),
                Some(
                    "Set Bot ID and long-connection Secret in Settings → Integrations, then restart."
                        .to_string(),
                ),
            );
        }
        return;
    }
    let (corp_id, secret_set, agent_id, callback_set, aes_set, port) = if let Some(row) = inst {
        (
            row.wecom_corp_id.trim().to_string(),
            !row.token.trim().is_empty(),
            row.wecom_agent_id,
            !row.wecom_callback_token.trim().is_empty(),
            !row.wecom_encoding_aes_key.trim().is_empty(),
            row.wecom_webhook_port,
        )
    } else {
        (
            config.wecom_corp_id.clone().unwrap_or_default(),
            config
                .wecom_corp_secret
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false),
            config.wecom_agent_id,
            config
                .wecom_callback_token
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false),
            config
                .wecom_encoding_aes_key
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false),
            config.wecom_webhook_port,
        )
    };
    let missing: Vec<&str> = [
        (corp_id.is_empty(), "corp_id"),
        (!secret_set, "corp_secret"),
        (agent_id <= 0, "agent_id"),
        (!callback_set, "callback_token"),
        (!aes_set, "encoding_aes_key"),
        (port == 0, "webhook_port"),
    ]
    .into_iter()
    .filter_map(|(missing, name)| if missing { Some(name) } else { None })
    .collect();
    if missing.is_empty() {
        report.push(
            "channel.wecom",
            "WeCom credentials",
            CheckStatus::Pass,
            format!("WeCom app configured (callback port {port})"),
            None,
        );
    } else {
        report.push(
            "channel.wecom",
            "WeCom credentials",
            CheckStatus::Fail,
            format!("WeCom is partially configured; missing: {}", missing.join(", ")),
            Some(
                "Complete Corp ID, Agent ID, corp secret, callback token, and EncodingAESKey in Settings → Integrations, then restart.".to_string(),
            ),
        );
    }
}

fn check_config(report: &mut DoctorReport) {
    match Config::resolve_config_path() {
        Ok(Some(path)) => report.push(
            "config.file",
            "Config file",
            CheckStatus::Pass,
            format!("found {}", path.display()),
            None,
        ),
        Ok(None) => report.push(
            "config.file",
            "Config file",
            CheckStatus::Warn,
            ".env not found".to_string(),
            Some("Copy .env.example to .env.".to_string()),
        ),
        Err(err) => report.push(
            "config.file",
            "Config file",
            CheckStatus::Fail,
            err.to_string(),
            Some("Fix MEIDO_CONFIG or create a valid config file.".to_string()),
        ),
    }
}

fn check_llm_provider_base_url(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let Ok(db) = crate::db::Database::new(&config.runtime_data_dir()) else {
        return;
    };
    let Ok(personas) = db.list_personas_for_doctor() else {
        return;
    };
    let mut warned = false;
    for persona in personas {
        let provider = persona
            .llm_provider_override
            .as_deref()
            .map(crate::llm_catalog::resolve_catalog_provider_id)
            .unwrap_or_default();
        if provider != "xai" {
            continue;
        }
        let base = persona
            .llm_base_url_override
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("https://api.x.ai/v1");
        if !base.contains("api.x.ai") {
            warned = true;
            report.push(
                "llm.xai_base_url",
                "xAI Grok base URL",
                CheckStatus::Warn,
                format!("Persona {} uses xAI with base URL {base}", persona.name),
                Some("Expected https://api.x.ai/v1 unless you use a deliberate proxy.".to_string()),
            );
        }
    }
    if !warned {
        report.push(
            "llm.xai_base_url",
            "xAI Grok base URL",
            CheckStatus::Pass,
            "No persona stores an unexpected xAI base URL.".to_string(),
            None,
        );
    }
}

fn check_evaluator_api_key(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let needs_key = config.post_tool_evaluator_enabled || config.response_quality_evaluator_enabled;
    if !needs_key {
        return;
    }
    let Ok(db) = crate::db::Database::new(&config.runtime_data_dir()) else {
        return;
    };
    let pair = crate::topics::read_system_llm_settings(&db).ok();
    let ready = pair.is_some_and(|(provider, model)| {
        provider
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            && model
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    });
    if ready {
        report.push(
            "evaluator.system_llm",
            "System LLM (PTE/PDQE)",
            CheckStatus::Pass,
            "System LLM provider and model are saved.".to_string(),
            None,
        );
    } else {
        report.push(
            "evaluator.system_llm",
            "System LLM (PTE/PDQE)",
            CheckStatus::Warn,
            "PTE and PDQE do not run until Settings → System LLM has a provider and model."
                .to_string(),
            Some("Open Settings → System LLM and save a provider and model.".to_string()),
        );
    }
}

fn check_cost_routing_local(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let runtime_dir_str = config.runtime_data_dir();
    let Ok(db) = crate::db::Database::new(&runtime_dir_str) else {
        return;
    };
    let Ok(personas) = db.list_personas_for_doctor() else {
        return;
    };
    for persona in personas {
        if persona.agent_engine_override.as_deref() != Some("classic_cost_routing") {
            continue;
        }
        let local = crate::local_delegate::from_persona(&persona);
        if local.local_routable() {
            report.push(
                "local_delegate.cost_routing",
                "Classic · Cost routing local delegate",
                CheckStatus::Pass,
                format!(
                    "Persona {} local delegate verified ({} @ {})",
                    persona.name, local.local_model, local.local_base_url
                ),
                None,
            );
        } else if local.local_configured() && !local.local_tools_ok {
            report.push(
                "local_delegate.cost_routing",
                "Classic · Cost routing local delegate",
                CheckStatus::Warn,
                format!(
                    "Persona {} has a local URL and model, but tool calling is not verified. The turn stops until Test passes.",
                    persona.name
                ),
                Some("Open the Persona page and run the tool-calling test.".to_string()),
            );
        } else {
            report.push(
                "local_delegate.cost_routing",
                "Classic · Cost routing local delegate",
                CheckStatus::Warn,
                format!(
                    "Persona {} uses cost routing without a local URL and model. The turn stops.",
                    persona.name
                ),
                Some("Set the local URL and model on the Persona page.".to_string()),
            );
        }
    }
}

fn check_shadow_workspace(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let root = config.workspace_root_absolute();
    let shadow = crate::tools::shadow_workspace_path(&root);
    if shadow.is_dir() {
        report.push(
            "workspace.shadow",
            "Shadow workspace",
            CheckStatus::Warn,
            format!(
                "nested workspace at {} — not WORKSPACE_DIR; agents may have doubled path prefixes",
                shadow.display()
            ),
            Some(
                "Migrate data from shared/workspace/ to the canonical tree, remove the shadow dir, \
                 and use tool paths relative to shared/ without a workspace/ prefix."
                    .to_string(),
            ),
        );
    } else {
        report.push(
            "workspace.shadow",
            "Shadow workspace",
            CheckStatus::Pass,
            "no shared/workspace/ nested copy detected".to_string(),
            None,
        );
    }
}

fn check_legacy_flat_shared(report: &mut DoctorReport) {
    let Ok(config) = Config::load() else {
        return;
    };
    let shared = config.workspace_root_absolute().join("shared");
    let legacy_candidates = ["scripts", "parking"];
    let found: Vec<String> = legacy_candidates
        .iter()
        .map(|n| shared.join(n))
        .filter(|p| p.exists())
        .map(|p| p.display().to_string())
        .collect();
    if found.is_empty() {
        report.push(
            "workspace.persona_shared",
            "Persona shared layout",
            CheckStatus::Pass,
            "no legacy flat shared/scripts or shared/parking directories detected".to_string(),
            None,
        );
    } else {
        report.push(
            "workspace.persona_shared",
            "Persona shared layout",
            CheckStatus::Warn,
            format!(
                "legacy flat shared directories still present: {}",
                found.join(", ")
            ),
            Some(
                "Move reusable scripts into skills/<name>/ or shared/skills/<name>/, and move scratch/output files under shared/personas/{chat_id}/{persona_id}/."
                    .to_string(),
            ),
        );
    }
}

fn check_path(report: &mut DoctorReport) {
    let target = if cfg!(target_os = "windows") {
        user_home_dir().map(|h| h.join(".local").join("bin"))
    } else {
        None
    };

    if let Some(dir) = target {
        if path_contains(&dir) {
            report.push(
                "path.install_dir",
                "Install dir in PATH",
                CheckStatus::Pass,
                format!("{} is in PATH", dir.display()),
                None,
            );
        } else {
            report.push(
                "path.install_dir",
                "Install dir in PATH",
                CheckStatus::Warn,
                format!("{} is not in PATH", dir.display()),
                Some(
                    "Re-run install.ps1 or add `%USERPROFILE%\\.local\\bin` to user PATH and restart terminal."
                        .to_string(),
                ),
            );
        }
    } else if command_exists("meido") {
        report.push(
            "path.meido",
            "meido in PATH",
            CheckStatus::Pass,
            "meido is discoverable in PATH".to_string(),
            None,
        );
    } else {
        report.push(
            "path.meido",
            "meido in PATH",
            CheckStatus::Warn,
            "meido is not discoverable in PATH".to_string(),
            Some("Add the meido binary directory to PATH.".to_string()),
        );
    }
}

fn check_shell(report: &mut DoctorReport) {
    if cfg!(target_os = "windows") {
        let status = if command_exists("pwsh") || command_exists("powershell") {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };

        let detail = if status == CheckStatus::Pass {
            "PowerShell runtime detected".to_string()
        } else {
            "PowerShell runtime not found".to_string()
        };

        let fix = if status == CheckStatus::Pass {
            None
        } else {
            Some("Install PowerShell 7+ or ensure Windows PowerShell is available.".to_string())
        };

        report.push("shell.runtime", "Shell runtime", status, detail, fix);

        match detect_execution_policy() {
            Ok(policy) => {
                let (status, detail, fix) = classify_execution_policy(&policy);
                report.push(
                    "powershell.policy",
                    "PowerShell execution policy",
                    status,
                    detail,
                    fix,
                );
            }
            Err(err) => {
                report.push(
                    "powershell.policy",
                    "PowerShell execution policy",
                    CheckStatus::Warn,
                    format!("failed to read policy: {err}"),
                    Some(
                        "Run `Get-ExecutionPolicy -Scope CurrentUser` manually in PowerShell."
                            .to_string(),
                    ),
                );
            }
        }
    } else {
        let shell_ok = command_exists("bash") || command_exists("sh");
        report.push(
            "shell.runtime",
            "Shell runtime",
            if shell_ok {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            if shell_ok {
                "bash/sh runtime detected".to_string()
            } else {
                "bash/sh runtime not found".to_string()
            },
            if shell_ok {
                None
            } else {
                Some("Install a POSIX shell (bash or sh).".to_string())
            },
        );
    }
}

fn check_node_and_browser(report: &mut DoctorReport) {
    let node_ok = command_exists("node");
    report.push(
        "deps.node",
        "Node.js",
        if node_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        if node_ok {
            "node found".to_string()
        } else {
            "node not found".to_string()
        },
        if node_ok {
            None
        } else {
            Some("Install Node.js LTS from https://nodejs.org/".to_string())
        },
    );

    let npm_ok = command_exists("npm");
    report.push(
        "deps.npm",
        "npm",
        if npm_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        if npm_ok {
            "npm found".to_string()
        } else {
            "npm not found".to_string()
        },
        if npm_ok {
            None
        } else {
            Some("Install npm (bundled with Node.js).".to_string())
        },
    );

    let steel_url = Config::load()
        .map(|c| c.steel_api_url())
        .unwrap_or_else(|_| {
            std::env::var("STEEL_API_URL")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "http://127.0.0.1:13920".to_string())
        });
    let steel_api_port = Config::load()
        .map(|c| c.steel_api_port)
        .unwrap_or(crate::config::default_steel_api_port());
    let steel_cdp_port = Config::load()
        .map(|c| c.steel_cdp_port)
        .unwrap_or(crate::config::default_steel_cdp_port());
    let steel_base = steel_url.trim_end_matches('/');
    let steel_ok = ["v1/health", "api/health"].iter().any(|path| {
        let steel_health_url = format!("{steel_base}/{path}");
        std::process::Command::new("curl")
            .args(["-sf", "--max-time", "3", &steel_health_url])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    });
    let steel_venv = Config::load()
        .ok()
        .map(|c| {
            let workspace = c.working_dir();
            std::path::Path::new(&workspace)
                .join("shared")
                .join(".venv-steel")
                .join("bin")
                .join("python3")
                .is_file()
        })
        .unwrap_or(false);

    report.push(
        "deps.steel_browser",
        "steel-browser",
        if steel_ok || steel_venv {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        if steel_ok {
            format!("Steel API reachable at {steel_url}")
        } else if steel_venv {
            "Steel Python venv found; API not reachable (start Steel Docker)".to_string()
        } else {
            "Steel browser not configured".to_string()
        },
        if steel_ok {
            None
        } else {
            Some(
                format!(
                    "Run `bash builtin_skills/steel-browser/setup_steel_env.sh`, set `BROWSER_MANAGED=true` to auto-start Steel on bot boot, then start manually: `docker run -d --name meido-steel -p {steel_api_port}:3000 -p {steel_cdp_port}:9223 -v meido-steel-cache:/app/.cache -v meido-steel-profile:/app/api/user-data-dir -e DOMAIN=127.0.0.1:{steel_api_port} -e CDP_DOMAIN=127.0.0.1:{steel_cdp_port} ghcr.io/steel-dev/steel-browser:latest`"
                ),
            )
        },
    );

    let cursor_agent_path = Config::load()
        .map(|c| c.cursor_agent_cli_path.trim().to_string())
        .unwrap_or_else(|_| {
            if cfg!(target_os = "windows") {
                "cursor-agent.cmd".into()
            } else {
                "cursor-agent".into()
            }
        });
    let cursor_agent_ok = !cursor_agent_path.is_empty() && command_exists(&cursor_agent_path);
    report.push(
        "deps.cursor_agent",
        "cursor-agent",
        if cursor_agent_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Warn
        },
        if cursor_agent_ok {
            "cursor-agent command found".to_string()
        } else {
            "cursor-agent command not found".to_string()
        },
        if cursor_agent_ok {
            None
        } else {
            Some("Run `curl https://cursor.com/install -fsS | bash`.".to_string())
        },
    );
}

fn check_mcp_dependencies(report: &mut DoctorReport) {
    let data_root = match Config::load() {
        Ok(cfg) => cfg.data_root_dir(),
        Err(_) => PathBuf::from("./meido.data"),
    };

    let mcp_path = data_root.join("mcp.json");
    if !mcp_path.exists() {
        report.push(
            "mcp.config",
            "MCP config",
            CheckStatus::Warn,
            format!("{} not found", mcp_path.display()),
            Some("Create mcp.json if you need MCP servers.".to_string()),
        );
        return;
    }

    let content = match std::fs::read_to_string(&mcp_path) {
        Ok(s) => s,
        Err(err) => {
            report.push(
                "mcp.config",
                "MCP config",
                CheckStatus::Fail,
                format!("failed reading {}: {err}", mcp_path.display()),
                None,
            );
            return;
        }
    };

    let parsed: McpConfig = match serde_json::from_str(&content) {
        Ok(cfg) => cfg,
        Err(err) => {
            report.push(
                "mcp.config",
                "MCP config",
                CheckStatus::Fail,
                format!("invalid JSON in {}: {err}", mcp_path.display()),
                Some("Validate mcp.json format and key names.".to_string()),
            );
            return;
        }
    };

    report.push(
        "mcp.config",
        "MCP config",
        CheckStatus::Pass,
        format!(
            "loaded {} with {} server(s)",
            mcp_path.display(),
            parsed.mcp_servers.len()
        ),
        None,
    );

    for (name, server) in &parsed.mcp_servers {
        let transport = server.transport.trim().to_ascii_lowercase();
        if transport == "streamable_http" || transport == "http" {
            if server.endpoint.trim().is_empty() {
                report.push(
                    format!("mcp.{name}.endpoint"),
                    format!("MCP server '{name}' endpoint"),
                    CheckStatus::Fail,
                    "endpoint is empty".to_string(),
                    Some("Set endpoint/url for streamable_http transport.".to_string()),
                );
            } else {
                report.push(
                    format!("mcp.{name}.endpoint"),
                    format!("MCP server '{name}' endpoint"),
                    CheckStatus::Pass,
                    server.endpoint.to_string(),
                    None,
                );
            }
            continue;
        }

        let command = server.command.trim();
        if command.is_empty() {
            report.push(
                format!("mcp.{name}.command"),
                format!("MCP server '{name}' command"),
                CheckStatus::Fail,
                "command is empty for stdio transport".to_string(),
                Some("Set command/args for stdio MCP server.".to_string()),
            );
            continue;
        }

        if command_exists(command) {
            report.push(
                format!("mcp.{name}.command"),
                format!("MCP server '{name}' command"),
                CheckStatus::Pass,
                format!("{command} found"),
                None,
            );
        } else {
            report.push(
                format!("mcp.{name}.command"),
                format!("MCP server '{name}' command"),
                CheckStatus::Fail,
                format!("{command} not found in PATH"),
                Some("Install the MCP command dependency or use absolute path.".to_string()),
            );
        }
    }
}

fn print_report(report: &DoctorReport) {
    println!("Meido Doctor");
    println!(
        "Environment: os={} arch={} wsl={}",
        report.platform, report.arch, report.in_wsl
    );
    println!();

    for check in &report.checks {
        println!(
            "[{} {:<4}] {:<28} ({}) {}",
            check.status.as_emoji(),
            check.status.as_label(),
            check.title,
            check.id,
            check.detail
        );
        if let Some(fix) = &check.fix {
            println!("        fix: {}", fix);
        }
    }

    let (pass, warn, fail) = report.summary();
    println!();
    println!("Summary: pass={} warn={} fail={}", pass, warn, fail);
    if fail > 0 {
        println!("Doctor exit code: 2 (hard failures present)");
    } else {
        println!("Doctor exit code: 0");
    }
}

fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

fn is_wsl() -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }

    if std::env::var_os("WSL_INTEROP").is_some() || std::env::var_os("WSL_DISTRO_NAME").is_some() {
        return true;
    }

    if let Ok(content) = std::fs::read_to_string("/proc/version") {
        let lower = content.to_ascii_lowercase();
        return lower.contains("microsoft") || lower.contains("wsl");
    }

    false
}

fn user_home_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn path_contains(dir: &Path) -> bool {
    let Ok(path_var) = std::env::var("PATH") else {
        return false;
    };

    let want = normalize_path_for_compare(dir);
    for entry in std::env::split_paths(&path_var) {
        if normalize_path_for_compare(&entry) == want {
            return true;
        }
    }
    false
}

fn normalize_path_for_compare(path: &Path) -> String {
    let s = path
        .to_string_lossy()
        .trim()
        .trim_end_matches(['/', '\\'])
        .to_string();
    if cfg!(target_os = "windows") {
        s.to_ascii_lowercase()
    } else {
        s
    }
}

fn command_exists(command: &str) -> bool {
    if command.trim().is_empty() {
        return false;
    }

    let path_var = std::env::var_os("PATH").unwrap_or_default();

    #[cfg(target_os = "windows")]
    let candidates: Vec<String> = {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let ext_list: Vec<String> = exts
            .split(';')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let mut out = vec![command.to_string()];
        let lower = command.to_ascii_lowercase();
        if !ext_list.iter().any(|ext| lower.ends_with(ext)) {
            for ext in ext_list {
                out.push(format!("{command}{ext}"));
            }
        }
        out
    };

    #[cfg(not(target_os = "windows"))]
    let candidates: Vec<String> = vec![command.to_string()];

    for base in std::env::split_paths(&path_var) {
        for candidate in &candidates {
            let full = base.join(candidate);
            if full.is_file() {
                return true;
            }
        }
    }
    false
}

fn detect_execution_policy() -> Result<String, String> {
    if !cfg!(target_os = "windows") {
        return Ok("NotApplicable".to_string());
    }

    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-ExecutionPolicy -Scope CurrentUser",
        ])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn classify_execution_policy(policy: &str) -> (CheckStatus, String, Option<String>) {
    let p = policy.trim().to_ascii_lowercase();
    match p.as_str() {
        "remotesigned" | "allsigned" | "bypass" | "unrestricted" => {
            (CheckStatus::Pass, format!("CurrentUser={policy}"), None)
        }
        "undefined" => (
            CheckStatus::Warn,
            format!("CurrentUser={policy}"),
            Some(
                "Set execution policy: `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned`"
                    .into(),
            ),
        ),
        "restricted" => (
            CheckStatus::Fail,
            format!("CurrentUser={policy}"),
            Some("Run `Set-ExecutionPolicy -Scope CurrentUser RemoteSigned` in PowerShell.".into()),
        ),
        _ => (
            CheckStatus::Warn,
            format!("CurrentUser={policy}"),
            Some("Verify policy allows running local install scripts.".into()),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_execution_policy_restricted() {
        let (status, detail, fix) = classify_execution_policy("Restricted");
        assert_eq!(status, CheckStatus::Fail);
        assert!(detail.contains("Restricted"));
        assert!(fix.unwrap().contains("RemoteSigned"));
    }

    #[test]
    fn test_classify_execution_policy_remotesigned() {
        let (status, _detail, fix) = classify_execution_policy("RemoteSigned");
        assert_eq!(status, CheckStatus::Pass);
        assert!(fix.is_none());
    }

    #[test]
    fn test_normalize_path_compare() {
        let p = PathBuf::from("/tmp/abc/");
        let normalized = normalize_path_for_compare(&p);
        assert!(!normalized.ends_with('/'));
    }
}
