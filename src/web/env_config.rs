//! `GET`/`PATCH /api/env-config` and setup actions that used to live in the installer.

use super::*;
use crate::dotenv_sync::{self, EnvUpdate, ReadinessExtras};
use std::collections::HashMap;

type ApiError = (StatusCode, String);

#[derive(Debug, Deserialize)]
pub(super) struct EnvConfigPatch {
    #[serde(default)]
    values: HashMap<String, String>,
    #[serde(default)]
    clear: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SetupActionBody {
    action: String,
}

pub(super) struct LoadedEnv {
    restart: bool,
    changed: bool,
    values: HashMap<String, String>,
}

pub(super) async fn load_env(state: &WebState) -> Result<LoadedEnv, ApiError> {
    let path = dotenv_sync::config_path().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let sync = dotenv_sync::refresh_process_env(&path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if sync.changed {
        for (key, value) in &sync.values {
            if dotenv_sync::is_secret_key(key) {
                state.app_state.env_redactor.register_trusted(value);
            }
        }
        state
            .app_state
            .persona_external_paths
            .reload_from_process_env();
        apply_runtime_keys(state, &sync.values);
        if sync.values.contains_key("SYSTEM_LLM_PROVIDER")
            || sync.values.contains_key("SYSTEM_LLM_MODEL")
        {
            let provider = sync
                .values
                .get("SYSTEM_LLM_PROVIDER")
                .cloned()
                .unwrap_or_default();
            let model = sync
                .values
                .get("SYSTEM_LLM_MODEL")
                .cloned()
                .unwrap_or_default();
            let chat_id = state.app_state.config.operator_inbox_chat_id();
            let _ = call_blocking(state.app_state.db.clone(), move |db| {
                crate::topics::save_system_llm_settings(db, &provider, &model)?;
                if !provider.trim().is_empty() && !model.trim().is_empty() {
                    let personas = db.list_personas(chat_id)?;
                    if let Some(persona) = personas.iter().find(|item| item.name == "default") {
                        let missing = persona
                            .model_override
                            .as_deref()
                            .map(str::trim)
                            .unwrap_or("")
                            .is_empty();
                        if missing {
                            db.set_persona_engine_override(chat_id, persona.id, Some("classic"))?;
                            db.set_persona_llm_selection(
                                chat_id,
                                persona.id,
                                Some(provider.as_str()),
                                Some(model.as_str()),
                                None,
                            )?;
                        }
                    }
                }
                Ok(())
            })
            .await;
        }
    }
    Ok(LoadedEnv {
        restart: dotenv_sync::restart_required(&state.app_state.config, &sync.values),
        changed: sync.changed,
        values: sync.values,
    })
}

pub(super) async fn note_disk_changes(state: &WebState) -> bool {
    matches!(load_env(state).await, Ok(loaded) if loaded.restart)
}

fn apply_runtime_keys(state: &WebState, values: &HashMap<String, String>) {
    let toggles = &state.app_state.runtime_toggles;
    if let Some(value) = values.get("TOOL_OUTPUT_DEBUG") {
        toggles.set_tool_output_debug(crate::config::parse_bool_setting(value));
    }
    if let Some(value) = values.get("POST_TOOL_EVALUATOR_ENABLED") {
        toggles.set_post_tool_evaluator_enabled(crate::config::parse_bool_setting(value));
    }
    if let Some(value) = values.get("RESPONSE_QUALITY_EVALUATOR_ENABLED") {
        toggles.set_response_quality_evaluator_enabled(crate::config::parse_bool_setting(value));
    }
    if let Some(value) = values.get("AGENT_ENGINE") {
        toggles.set_agent_engine(crate::runtime_toggles::AgentEngine::parse(value));
    }
}

fn persist_env(updates: Vec<EnvUpdate>) -> Result<(), String> {
    if updates.is_empty() {
        return Ok(());
    }
    let path = dotenv_sync::config_path()?;
    let workspace = std::env::var("WORKSPACE_DIR").unwrap_or_else(|_| "./workspace".into());
    let token_set = std::env::var("WEB_AUTH_TOKEN")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());
    let mut updates = updates;
    dotenv_sync::normalize_updates(&workspace, &mut updates, token_set)?;
    dotenv_sync::apply_updates(&path, &updates)?;
    for update in &updates {
        match &update.value {
            Some(value) => std::env::set_var(&update.key, value),
            None => std::env::remove_var(&update.key),
        }
    }
    if updates.iter().any(|update| {
        matches!(
            update.key.as_str(),
            "WORKSPACE_DIR"
                | "VAULT_ORIGIN_VAULT_PATH"
                | "VAULT_VECTOR_DB_PATH"
                | "VAULT_PRINCIPLES_PATH"
        )
    }) {
        let workspace = std::env::var("WORKSPACE_DIR").unwrap_or(workspace);
        let origin = std::env::var("VAULT_ORIGIN_VAULT_PATH").ok();
        let db = std::env::var("VAULT_VECTOR_DB_PATH").ok();
        let principles = std::env::var("VAULT_PRINCIPLES_PATH").ok();
        dotenv_sync::ignore_workspace_paths(&workspace, origin.as_deref(), db.as_deref());
        dotenv_sync::seed_principles(&workspace, principles.as_deref());
    }
    Ok(())
}

pub(super) fn persist_pairs(pairs: &[(&str, Option<&str>)]) -> Result<(), String> {
    let updates = pairs
        .iter()
        .map(|(key, value)| EnvUpdate {
            key: (*key).to_string(),
            value: value.map(str::to_string),
        })
        .collect();
    persist_env(updates)
}

async fn readiness(state: &WebState, values: &HashMap<String, String>) -> ReadinessExtras {
    let system_llm_saved = call_blocking(state.app_state.db.clone(), |db| {
        let (provider, model) = crate::topics::read_system_llm_settings(db)?;
        Ok(provider.is_some() && model.is_some())
    })
    .await
    .unwrap_or(false);
    let data_dir = state.app_state.config.runtime_data_dir();
    let provider_key_saved = values.iter().any(|(key, value)| {
        dotenv_sync::is_secret_key(key)
            && key != "WEB_AUTH_TOKEN"
            && key != "TAVILY_API_KEY"
            && !value.trim().is_empty()
    }) || crate::engine_connections::CONNECTIONS.iter().any(|def| {
        def.secret_name.is_some_and(|name| {
            name != "TAVILY_API_KEY"
                && (crate::engine_connections::secret_is_saved(&data_dir, name)
                    || std::env::var(name)
                        .ok()
                        .is_some_and(|v| !v.trim().is_empty()))
        })
    });
    let local = state.app_state.llm.local_delegate_config();
    let mempalace = crate::mempalace_setup::is_installed(&state.app_state.config);
    let workspace = steel_workspace(state);
    let image = steel_image_name(state);
    let managed = browser_managed_now(state);
    let steel_installed = tokio::task::spawn_blocking(move || {
        crate::steel_browser_sidecar::runtime_installed(&workspace, &image, managed)
    })
    .await
    .unwrap_or(false);
    ReadinessExtras {
        system_llm_saved,
        provider_key_saved,
        local_model_ready: !local.local_base_url.trim().is_empty()
            && !local.local_model.trim().is_empty(),
        mempalace_ready: mempalace,
        steel_installed,
    }
}

fn steel_workspace(state: &WebState) -> String {
    std::env::var("WORKSPACE_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| state.app_state.config.workspace_dir.clone())
}

fn steel_image_name(state: &WebState) -> String {
    std::env::var("STEEL_DOCKER_IMAGE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| state.app_state.config.steel_docker_image.clone())
}

fn browser_managed_now(state: &WebState) -> bool {
    std::env::var("BROWSER_MANAGED")
        .ok()
        .map(|value| crate::config::parse_bool_setting(&value))
        .unwrap_or(state.app_state.config.browser_managed)
}

fn persist_steel_defaults() -> Result<(), String> {
    let mut pairs = vec![("BROWSER_MANAGED", Some("true"))];
    let fill = |key: &'static str| {
        std::env::var(key)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
    };
    if !fill("STEEL_API_PORT") {
        pairs.push(("STEEL_API_PORT", Some("13920")));
    }
    if !fill("STEEL_CDP_PORT") {
        pairs.push(("STEEL_CDP_PORT", Some("13923")));
    }
    if !fill("STEEL_API_URL") {
        pairs.push(("STEEL_API_URL", Some("http://127.0.0.1:13920")));
    }
    if !fill("STEEL_DOCKER_IMAGE") {
        pairs.push((
            "STEEL_DOCKER_IMAGE",
            Some("ghcr.io/steel-dev/steel-browser:latest"),
        ));
    }
    if !fill("STEEL_DOCKER_CONTAINER_NAME") {
        pairs.push(("STEEL_DOCKER_CONTAINER_NAME", Some("meido-steel")));
    }
    persist_pairs(&pairs)
}

async fn install_mempalace_if_missing(
    config: crate::config::Config,
) -> Result<&'static str, ApiError> {
    let check = config.clone();
    let already = tokio::task::spawn_blocking(move || crate::mempalace_setup::is_installed(&check))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if already {
        return Ok("MemPalace is already installed.");
    }
    tokio::task::spawn_blocking(move || crate::mempalace_setup::ensure_managed_mempalace(&config))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok("MemPalace was installed. Restart Meido so recall uses it.")
}

async fn install_steel_if_missing(state: &WebState) -> Result<String, ApiError> {
    let managed_before = browser_managed_now(state);
    persist_steel_defaults().map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let workspace = steel_workspace(state);
    let image = steel_image_name(state);
    let workspace_check = workspace.clone();
    let image_check = image.clone();
    let (image_ready, venv_ready) = tokio::task::spawn_blocking(move || {
        (
            crate::steel_browser_sidecar::image_present(&image_check),
            crate::steel_browser_sidecar::skill_venv_installed(&workspace_check),
        )
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut notes = Vec::new();
    if image_ready {
        notes.push("Steel image is already present.".to_string());
    } else {
        let image_pull = image;
        tokio::task::spawn_blocking(move || {
            std::process::Command::new("docker")
                .args(["pull", image_pull.as_str()])
                .status()
                .map_err(|e| format!("docker pull failed: {e}"))
                .and_then(|status| {
                    if status.success() {
                        Ok(())
                    } else {
                        Err("docker pull failed. Check that the Docker daemon is running.".into())
                    }
                })
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        notes.push("Steel image was pulled.".to_string());
    }
    if venv_ready {
        notes.push("Steel skill environment is already installed.".to_string());
    } else {
        let workspace_install = workspace;
        tokio::task::spawn_blocking(move || {
            crate::steel_browser_sidecar::ensure_skill_venv(&workspace_install)
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        notes.push("Steel skill environment was installed.".to_string());
    }
    if !image_ready || !venv_ready || !managed_before {
        notes.push("Restart Meido to start the browser.".to_string());
    }
    Ok(notes.join(" "))
}

fn field_json(values: &HashMap<String, String>, key: &str) -> serde_json::Value {
    let secret = dotenv_sync::is_secret_key(key);
    let set = values
        .get(key)
        .is_some_and(|value| !value.trim().is_empty());
    json!({
        "key": key,
        "set": set,
        "secret": secret,
        "value": if secret { serde_json::Value::Null } else { json!(values.get(key).map(|s| s.as_str()).unwrap_or("")) },
    })
}

fn env_view(
    state: &WebState,
    values: &HashMap<String, String>,
    reloaded: bool,
    extras: &ReadinessExtras,
) -> serde_json::Value {
    let keys = [
        "WORKSPACE_DIR",
        "TIMEZONE",
        "AGENT_DISPLAY_NAME",
        "WEB_HOST",
        "WEB_PORT",
        "WEB_AUTH_TOKEN",
        "VAULT_ORIGIN_VAULT_PATH",
        "VAULT_VECTOR_DB_PATH",
        "VAULT_VECTOR_DB_COLLECTION",
        "VAULT_PRINCIPLES_PATH",
        "VAULT_EMBEDDING_SERVER_URL",
        "VAULT_ORIGIN_VAULT_REPO",
        "SYSTEM_LLM_PROVIDER",
        "SYSTEM_LLM_MODEL",
        "TAVILY_API_KEY",
        "MEMPALACE_RECALL_MODE",
        "BROWSER_MANAGED",
        "STEEL_API_PORT",
        "STEEL_CDP_PORT",
        "STEEL_API_URL",
        "STEEL_DOCKER_IMAGE",
        "STEEL_DOCKER_CONTAINER_NAME",
        "SEARXNG_URL",
    ];
    let fields: Vec<_> = keys.iter().map(|key| field_json(values, key)).collect();
    let python = if cfg!(windows) { "python" } else { "python3" };
    json!({
        "ok": true,
        "reloaded_from_disk": reloaded,
        "requires_restart_for_env_changes": dotenv_sync::restart_required(&state.app_state.config, values),
        "initializer": dotenv_sync::initializer_status(values, extras),
        "fields": fields,
        "probes": {
            "node_available": dotenv_sync::command_available("node"),
            "npm_available": dotenv_sync::command_available("npm"),
            "python_available": dotenv_sync::command_available(python),
            "docker_available": dotenv_sync::command_available("docker"),
            "tmux_available": dotenv_sync::command_available("tmux"),
            "lan_ipv4": dotenv_sync::lan_ipv4(),
            "gateway_installed": crate::gateway::user_gateway_service_installed(),
            "mempalace_installed": extras.mempalace_ready,
            "steel_installed": extras.steel_installed,
        },
    })
}

pub(super) async fn api_env_config_get(
    headers: HeaderMap,
    State(state): State<WebState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let loaded = load_env(&state).await?;
    let extras = readiness(&state, &loaded.values).await;
    Ok(Json(env_view(
        &state,
        &loaded.values,
        loaded.changed,
        &extras,
    )))
}

pub(super) async fn api_env_config_patch(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<EnvConfigPatch>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let mut updates = Vec::new();
    for key in &body.clear {
        updates.push(EnvUpdate {
            key: key.clone(),
            value: None,
        });
    }
    for (key, value) in body.values {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            updates.push(EnvUpdate { key, value: None });
        } else {
            updates.push(EnvUpdate {
                key,
                value: Some(trimmed),
            });
        }
    }
    if let Some(key) = updates.iter().find_map(|update| {
        crate::engine_connections::CONNECTIONS
            .iter()
            .find_map(|def| {
                def.secret_name
                    .filter(|name| name.eq_ignore_ascii_case(&update.key))
            })
    }) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Save {key} under Settings → Connections."),
        ));
    }
    persist_env(updates).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    if let (Ok(provider), Ok(model)) = (
        std::env::var("SYSTEM_LLM_PROVIDER"),
        std::env::var("SYSTEM_LLM_MODEL"),
    ) {
        if !provider.trim().is_empty() && !model.trim().is_empty() {
            let _ = call_blocking(state.app_state.db.clone(), move |db| {
                crate::topics::save_system_llm_settings(db, &provider, &model)
            })
            .await;
        }
    }
    let loaded = load_env(&state).await?;
    let values = loaded.values;
    let extras = readiness(&state, &values).await;
    let mut view = env_view(&state, &values, false, &extras);
    view["message"] = json!("Saved to .env.");
    Ok(Json(view))
}

pub(super) async fn api_setup_action_post(
    headers: HeaderMap,
    State(state): State<WebState>,
    Json(body): Json<SetupActionBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_auth(&headers, state.auth_token.as_deref())?;
    let action = body.action.trim().to_string();
    let message = match action.as_str() {
        "install_mempalace" => install_mempalace_if_missing(state.app_state.config.clone()).await?,
        "setup_steel" | "install_mempalace_and_steel" => {
            let config = state.app_state.config.clone();
            let mempalace = if action == "setup_steel" {
                ""
            } else {
                install_mempalace_if_missing(config).await?
            };
            let steel = install_steel_if_missing(&state).await?;
            let message = if mempalace.is_empty() {
                steel
            } else {
                format!("{mempalace} {steel}")
            };
            return Ok(Json(json!({ "ok": true, "message": message })));
        }
        "install_gateway" => {
            tokio::task::spawn_blocking(crate::gateway::install_login_service)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            "Login service installed. It starts Meido at login and restarts it if the process exits."
        }
        "uninstall_gateway" => {
            tokio::task::spawn_blocking(crate::gateway::uninstall_login_service)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            "Login service removed."
        }
        _ => {
            return Err((StatusCode::BAD_REQUEST, "Unknown setup action.".into()));
        }
    };
    Ok(Json(json!({ "ok": true, "message": message })))
}

pub(super) async fn installation_needs_initialization(state: &WebState) -> bool {
    let Ok(loaded) = load_env(state).await else {
        return true;
    };
    let sync_values = loaded.values;
    let extras = readiness(state, &sync_values).await;
    dotenv_sync::initializer_status(&sync_values, &extras).needs_initialization
}
