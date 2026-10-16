use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

use super::command_runner::{build_command, shell_command};
use super::{schema_object, Tool, ToolResult};
use crate::claude::ToolDefinition;

/// Search mode: native (embedding + ChromaDB HTTP) or command (run vault_search_command).
#[derive(Clone)]
pub enum SearchVaultMode {
    Native {
        embedding_url: String,
        vector_db_url: String,
        collection: String,
        http_client: reqwest::Client,
    },
    Command {
        vault_search_command: String,
        working_dir: PathBuf,
    },
}

pub struct SearchVaultTool {
    mode: SearchVaultMode,
    recall: Option<std::sync::Arc<crate::recall::RecallService>>,
}

impl SearchVaultTool {
    /// Native mode: call embedding server + ChromaDB HTTP API. Requires both to be running.
    pub fn new_native(embedding_url: &str, vector_db_url: &str, collection: &str) -> Self {
        Self {
            mode: SearchVaultMode::Native {
                embedding_url: embedding_url.trim_end_matches('/').to_string(),
                vector_db_url: vector_db_url.trim_end_matches('/').to_string(),
                collection: collection.to_string(),
                http_client: reqwest::Client::new(),
            },
            recall: None,
        }
    }

    /// Command mode: run vault_search_command with {query} substituted. No ChromaDB server needed.
    pub fn new_command(vault_search_command: &str, working_dir: &str) -> Self {
        Self {
            mode: SearchVaultMode::Command {
                vault_search_command: vault_search_command.to_string(),
                working_dir: PathBuf::from(working_dir),
            },
            recall: None,
        }
    }

    pub fn with_recall(mut self, recall: std::sync::Arc<crate::recall::RecallService>) -> Self {
        self.recall = Some(recall);
        self
    }

    pub fn mode(&self) -> SearchVaultMode {
        self.mode.clone()
    }

    /// Legacy constructor for native mode (backwards compatible).
    pub fn new(embedding_url: &str, vector_db_url: &str, collection: &str) -> Self {
        Self::new_native(embedding_url, vector_db_url, collection)
    }

    /// Command mode: run vault_search_command with {query} substituted.
    async fn execute_command_mode(
        &self,
        vault_search_command: &str,
        working_dir: &PathBuf,
        query: &str,
    ) -> ToolResult {
        // Substitute {query} in the command (support both {query} and {query:shell} if needed)
        let command = vault_search_command.replace("{query}", query);

        let working_dir_resolved = working_dir.clone();
        if let Err(e) = tokio::fs::create_dir_all(&working_dir_resolved).await {
            return ToolResult::error(format!(
                "Failed to create working directory {}: {e}",
                working_dir_resolved.display()
            ));
        }

        let spec = shell_command(&command);
        let timeout_secs = 60u64;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            build_command(&spec, Some(&working_dir_resolved)).output(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let exit_code = output.status.code().unwrap_or(-1);

                let mut result_text = String::new();
                if !stdout.is_empty() {
                    result_text.push_str(&stdout);
                }
                if !stderr.is_empty() {
                    if !result_text.is_empty() {
                        result_text.push('\n');
                    }
                    result_text.push_str("STDERR:\n");
                    result_text.push_str(&stderr);
                }
                if result_text.is_empty() {
                    result_text = format!("Command completed with exit code {exit_code}");
                }

                if result_text.len() > 30000 {
                    result_text.truncate(30000);
                    result_text.push_str("\n... (output truncated)");
                }

                if exit_code == 0 {
                    ToolResult::success(result_text).with_status_code(exit_code)
                } else {
                    ToolResult::error(format!("Exit code {exit_code}\n{result_text}"))
                        .with_status_code(exit_code)
                        .with_error_type("process_exit")
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Failed to execute vault search command: {e}"))
                .with_error_type("spawn_error"),
            Err(_) => ToolResult::error(format!(
                "Vault search command timed out after {timeout_secs} seconds"
            ))
            .with_error_type("timeout"),
        }
    }
}

pub struct VaultSnippet {
    pub source: String,
    pub distance: f64,
    pub excerpt: String,
}

fn metadata_source(meta: &serde_json::Value) -> &str {
    meta.as_object()
        .and_then(|obj| {
            obj.get("source")
                .or_else(|| obj.get("path"))
                .or_else(|| obj.get("file"))
                .or_else(|| obj.get("filename"))
        })
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
}

#[async_trait]
impl Tool for SearchVaultTool {
    fn name(&self) -> &str {
        "search_vault"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "search_vault",
            "Semantically search the ORIGIN vault (Obsidian notes, documents) using vector similarity. Use this to find relevant knowledge base entries. This searches the vault knowledge base, NOT conversation history — use search_chat_history for that.",
            schema_object(
                json!({
                    "query": {
                        "type": "string",
                        "description": "Natural language query to search for in the vault"
                    },
                    "n_results": {
                        "type": "integer",
                        "description": "Number of results to return (default: 5, max: 20)"
                    }
                }),
                &["query"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        if input.get("__recall_internal").is_none() {
            if let Some(recall) = &self.recall {
                if recall.mode() != crate::recall::RecallMode::Disabled {
                    let query = match input.get("query").and_then(|v| v.as_str()) {
                        Some(query) if !query.trim().is_empty() => query.trim().to_string(),
                        _ => return ToolResult::error("Missing or empty 'query' parameter".into()),
                    };
                    let Some(auth) = super::auth_context_from_input(&input) else {
                        return ToolResult::error(
                        "Missing auth context (__meido_auth) with caller_persona_id for this chat"
                            .into(),
                    )
                    .with_error_type("auth_required");
                    };
                    let limit = input
                        .get("n_results")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(5)
                        .min(20) as usize;
                    let response = recall
                        .query(crate::recall::RecallRequest {
                            query,
                            scope: crate::recall::RecallScope::Knowledge,
                            chat_id: auth.caller_chat_id,
                            persona_id: auth.caller_persona_id,
                            limit,
                            from_date: None,
                            to_date: None,
                        })
                        .await;
                    return ToolResult::success(response.to_json().to_string());
                }
            }
        }
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.to_string(),
            _ => return ToolResult::error("Missing or empty 'query' parameter".into()),
        };

        match &self.mode {
            SearchVaultMode::Command {
                vault_search_command,
                working_dir,
            } => {
                let auth = super::auth_context_from_input(&input);
                let working_dir_resolved =
                    super::resolve_tool_working_dir_for_auth(working_dir, auth.as_ref());
                return self
                    .execute_command_mode(vault_search_command, &working_dir_resolved, &query)
                    .await;
            }
            SearchVaultMode::Native { .. } => {
                // Fall through to native implementation below
            }
        }

        let n_results = input
            .get("n_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .min(20) as usize;

        let snippets = match query_snippets(&self.mode, &query, n_results).await {
            Ok(snippets) => snippets,
            Err(err) => return ToolResult::error(err),
        };
        if snippets.is_empty() {
            return ToolResult::success(format!("No vault results found for: {query}"));
        }
        let formatted: Vec<serde_json::Value> = snippets
            .iter()
            .enumerate()
            .map(|(index, snippet)| {
                json!({
                    "rank": index + 1,
                    "source": snippet.source,
                    "distance": snippet.distance,
                    "content": snippet.excerpt
                })
            })
            .collect();
        return ToolResult::success(serde_json::to_string_pretty(&formatted).unwrap_or_default());
    }
}

pub async fn query_snippets(
    mode: &SearchVaultMode,
    query: &str,
    n_results: usize,
) -> Result<Vec<VaultSnippet>, String> {
    match mode {
        SearchVaultMode::Command {
            vault_search_command,
            working_dir,
        } => {
            let tool = SearchVaultTool {
                mode: mode.clone(),
                recall: None,
            };
            let result = tool
                .execute_command_mode(vault_search_command, working_dir, query)
                .await;
            if result.is_error {
                return Err("vault_command".into());
            }
            let excerpt: String = result.content.chars().take(500).collect();
            Ok(vec![VaultSnippet {
                source: "vault-command".into(),
                distance: 0.0,
                excerpt,
            }])
        }
        SearchVaultMode::Native {
            embedding_url,
            vector_db_url,
            collection,
            http_client,
        } => {
            native_snippets(
                http_client,
                embedding_url,
                vector_db_url,
                collection,
                query,
                n_results,
            )
            .await
        }
    }
}

async fn native_snippets(
    http_client: &reqwest::Client,
    embedding_url: &str,
    vector_db_url: &str,
    collection: &str,
    query: &str,
    n_results: usize,
) -> Result<Vec<VaultSnippet>, String> {
    let embed_resp = http_client
        .post(format!("{embedding_url}/embedding"))
        .json(&json!({ "content": query }))
        .send()
        .await
        .map_err(|_| "embedding_unreachable".to_string())?;
    if !embed_resp.status().is_success() {
        return Err("embedding_failed".into());
    }
    let embed_json: serde_json::Value = embed_resp
        .json()
        .await
        .map_err(|_| "embedding_parse".to_string())?;
    let embedding: Vec<serde_json::Value> =
        if let Some(outer) = embed_json.get("embedding").and_then(|v| v.as_array()) {
            if outer.first().and_then(|v| v.as_array()).is_some() {
                outer
                    .first()
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default()
            } else {
                outer.clone()
            }
        } else {
            return Err("embedding_format".into());
        };
    if embedding.is_empty() {
        return Err("embedding_empty".into());
    }

    let col_resp = http_client
        .get(format!("{vector_db_url}/api/v1/collections/{collection}"))
        .send()
        .await
        .map_err(|_| "chroma_unreachable".to_string())?;
    if !col_resp.status().is_success() {
        return Err("chroma_collection".into());
    }
    let col_json: serde_json::Value = col_resp
        .json()
        .await
        .map_err(|_| "chroma_parse".to_string())?;
    let collection_id = col_json
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "chroma_collection".to_string())?;

    let query_resp = http_client
        .post(format!(
            "{vector_db_url}/api/v1/collections/{collection_id}/query"
        ))
        .json(&json!({
            "query_embeddings": [embedding],
            "n_results": n_results,
            "include": ["documents", "metadatas", "distances"]
        }))
        .send()
        .await
        .map_err(|_| "chroma_query".to_string())?;
    if !query_resp.status().is_success() {
        return Err("chroma_query".into());
    }
    let results_json: serde_json::Value = query_resp
        .json()
        .await
        .map_err(|_| "chroma_parse".to_string())?;
    let documents = results_json
        .get("documents")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let metadatas = results_json
        .get("metadatas")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let distances = results_json
        .get("distances")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    Ok(documents
        .iter()
        .enumerate()
        .map(|(index, doc)| {
            let text = doc.as_str().unwrap_or("");
            let mut excerpt: String = text.chars().take(500).collect();
            if text.chars().count() > 500 {
                excerpt.push_str("...");
            }
            VaultSnippet {
                source: metadatas
                    .get(index)
                    .map(metadata_source)
                    .unwrap_or("unknown")
                    .to_string(),
                distance: distances
                    .get(index)
                    .and_then(|value| value.as_f64())
                    .unwrap_or(0.0),
                excerpt,
            }
        })
        .collect())
}
