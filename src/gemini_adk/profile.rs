//! Hot-reloadable Gemini ADK multi-agent topology (Web UI + per-persona override).

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::db::Database;
use crate::error::FinallyAValueBotError;

pub const APP_SETTING_GEMINI_ADK_TOPOLOGY_CONFIG: &str = "GEMINI_ADK_TOPOLOGY_CONFIG";
pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_AGENTS: usize = 16;
pub const MAX_INSTRUCTION_CHARS: usize = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdkAgentKind {
    Llm,
    Sequential,
    Parallel,
    Loop,
}

impl AdkAgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Llm => "llm",
            Self::Sequential => "sequential",
            Self::Parallel => "parallel",
            Self::Loop => "loop",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdkAgent {
    pub id: String,
    pub label: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub kind: AdkAgentKind,
    /// Gemini model id; empty inherits topology `default_model`.
    #[serde(default)]
    pub model: String,
    /// System-prompt overlay for Llm agents (appended to run prep system prompt).
    #[serde(default)]
    pub instruction: String,
    /// When set, only these tool names from ToolRegistry are exposed (plus synthetic ADK tools).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    /// Child agent ids (for sequential/parallel/loop, or Llm transfer_to_agent targets).
    #[serde(default)]
    pub sub_agents: Vec<String>,
    /// When set, the agent's final text is written into session state under this key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_key: Option<String>,
    /// Max iterations for Loop agents (and Llm tool loops when set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_iterations: Option<usize>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdkTopologyProfile {
    pub version: u32,
    pub root_agent_id: String,
    pub agents: Vec<AdkAgent>,
    /// Default Gemini model when an Llm agent leaves `model` empty.
    #[serde(default = "default_model")]
    pub default_model: String,
}

fn default_model() -> String {
    crate::gemini_adk::config::DEFAULT_GEMINI_ADK_MODEL.to_string()
}

impl AdkTopologyProfile {
    pub fn default_profile() -> Self {
        Self {
            version: SCHEMA_VERSION,
            root_agent_id: "coordinator".into(),
            default_model: default_model(),
            agents: vec![
                AdkAgent {
                    id: "coordinator".into(),
                    label: "Coordinator".into(),
                    enabled: true,
                    kind: AdkAgentKind::Llm,
                    model: String::new(),
                    instruction: "You are the root coordinator. Use tools to accomplish the user request. Transfer to a specialist sub-agent when their expertise fits better than continuing yourself.".into(),
                    allowed_tools: None,
                    sub_agents: vec!["researcher".into(), "executor".into()],
                    output_key: Some("final".into()),
                    max_iterations: Some(40),
                },
                AdkAgent {
                    id: "researcher".into(),
                    label: "Researcher".into(),
                    enabled: true,
                    kind: AdkAgentKind::Llm,
                    model: String::new(),
                    instruction: "You gather facts, read files, and search. Prefer read-only tools. Return a concise research summary.".into(),
                    allowed_tools: None,
                    sub_agents: vec![],
                    output_key: Some("research".into()),
                    max_iterations: Some(25),
                },
                AdkAgent {
                    id: "executor".into(),
                    label: "Executor".into(),
                    enabled: true,
                    kind: AdkAgentKind::Llm,
                    model: String::new(),
                    instruction: "You implement changes using the bot tools. Be precise and verify outcomes.".into(),
                    allowed_tools: None,
                    sub_agents: vec![],
                    output_key: Some("execution".into()),
                    max_iterations: Some(40),
                },
            ],
        }
    }

    pub fn migrate(mut self) -> Self {
        if self.version < SCHEMA_VERSION {
            self.version = SCHEMA_VERSION;
        }
        if self.default_model.trim().is_empty() {
            self.default_model = default_model();
        }
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version == 0 || self.version > SCHEMA_VERSION {
            return Err(format!(
                "unsupported topology schema version {}",
                self.version
            ));
        }
        if self.agents.is_empty() {
            return Err("topology must have at least one agent".into());
        }
        if self.agents.len() > MAX_AGENTS {
            return Err(format!("topology may have at most {MAX_AGENTS} agents"));
        }
        let mut ids = HashSet::new();
        for agent in &self.agents {
            let id = agent.id.trim();
            if id.is_empty() {
                return Err("agent id must not be empty".into());
            }
            if !ids.insert(id.to_string()) {
                return Err(format!("duplicate agent id '{id}'"));
            }
            if agent.instruction.len() > MAX_INSTRUCTION_CHARS {
                return Err(format!(
                    "agent '{id}' instruction exceeds {MAX_INSTRUCTION_CHARS} chars"
                ));
            }
            for child in &agent.sub_agents {
                let c = child.trim();
                if c.is_empty() {
                    return Err(format!("agent '{id}' has empty sub_agent id"));
                }
                if c == id
                    && matches!(
                        agent.kind,
                        AdkAgentKind::Sequential | AdkAgentKind::Parallel | AdkAgentKind::Loop
                    )
                {
                    return Err(format!(
                        "agent '{id}' cannot list itself as a workflow child"
                    ));
                }
            }
        }
        if !ids.contains(self.root_agent_id.trim()) {
            return Err(format!(
                "root_agent_id '{}' not found in agents",
                self.root_agent_id
            ));
        }
        for agent in &self.agents {
            for child in &agent.sub_agents {
                if !ids.contains(child.trim()) {
                    return Err(format!(
                        "agent '{}' references unknown sub_agent '{}'",
                        agent.id, child
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn agent_map(&self) -> HashMap<String, &AdkAgent> {
        self.agents.iter().map(|a| (a.id.clone(), a)).collect()
    }
}

pub fn load_from_db(db: &Database) -> Result<AdkTopologyProfile, FinallyAValueBotError> {
    let settings = db.list_app_settings()?;
    let raw = settings
        .into_iter()
        .find(|s| {
            s.key
                .eq_ignore_ascii_case(APP_SETTING_GEMINI_ADK_TOPOLOGY_CONFIG)
        })
        .map(|s| s.value);
    let Some(raw) = raw.filter(|s| !s.trim().is_empty()) else {
        return Ok(AdkTopologyProfile::default_profile());
    };
    let profile: AdkTopologyProfile = serde_json::from_str(&raw).map_err(|e| {
        FinallyAValueBotError::Config(format!("invalid GEMINI_ADK_TOPOLOGY_CONFIG JSON: {e}"))
    })?;
    let profile = profile.migrate();
    profile
        .validate()
        .map_err(|e| FinallyAValueBotError::Config(format!("invalid ADK topology: {e}")))?;
    Ok(profile)
}

pub fn persist_to_db(
    db: &Database,
    profile: &AdkTopologyProfile,
) -> Result<(), FinallyAValueBotError> {
    profile
        .validate()
        .map_err(|e| FinallyAValueBotError::Config(format!("invalid ADK topology: {e}")))?;
    let json = serde_json::to_string_pretty(profile)
        .map_err(|e| FinallyAValueBotError::Config(format!("serialize ADK topology: {e}")))?;
    db.set_app_setting(APP_SETTING_GEMINI_ADK_TOPOLOGY_CONFIG, &json)
}

pub fn parse_topology_json(raw: &str) -> Result<AdkTopologyProfile, FinallyAValueBotError> {
    let profile: AdkTopologyProfile = serde_json::from_str(raw)
        .map_err(|e| FinallyAValueBotError::Config(format!("invalid ADK topology JSON: {e}")))?;
    let profile = profile.migrate();
    profile
        .validate()
        .map_err(|e| FinallyAValueBotError::Config(format!("invalid ADK topology: {e}")))?;
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_profile_validates() {
        let p = AdkTopologyProfile::default_profile();
        assert!(p.validate().is_ok());
    }

    #[test]
    fn rejects_missing_root() {
        let mut p = AdkTopologyProfile::default_profile();
        p.root_agent_id = "nope".into();
        assert!(p.validate().is_err());
    }
}
