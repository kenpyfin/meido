use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;

use super::{schema_object, Tool, ToolResult};
use crate::claude::ToolDefinition;

pub struct AddVaultItemTool {
    embedding_url: String,
    vector_db_url: String,
    collection: String,
    http_client: reqwest::Client,
}

impl AddVaultItemTool {
    pub fn new(embedding_url: &str, vector_db_url: &str, collection: &str) -> Self {
        Self {
            embedding_url: embedding_url.trim_end_matches('/').to_string(),
            vector_db_url: vector_db_url.trim_end_matches('/').to_string(),
            collection: collection.to_string(),
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl Tool for AddVaultItemTool {
    fn name(&self) -> &str {
        "add_vault_item"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "add_vault_item",
            "Add a new item to the vault (long-term memory). Use this to store successful solutions, learned patterns, or important facts for future retrieval. The item will be embedded and stored in the vector database.",
            schema_object(
                json!({
                    "content": {
                        "type": "string",
                        "description": "The text content to store (the lesson, solution, or fact)"
                    },
                    "source": {
                        "type": "string",
                        "description": "Origin of this information (e.g. 'self-correction', 'user-instruction', 'task-123')"
                    }
                }),
                &["content", "source"],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let content = match input.get("content").and_then(|v| v.as_str()) {
            Some(c) if !c.trim().is_empty() => c.to_string(),
            _ => return ToolResult::error("Missing or empty 'content' parameter".into()),
        };
        let source = input
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        // Step 1: Get embedding
        let embed_resp = match self
            .http_client
            .post(format!("{}/embedding", self.embedding_url))
            .json(&json!({"content": content}))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("Embedding server unreachable: {e}")),
        };

        if !embed_resp.status().is_success() {
            let status = embed_resp.status();
            let body = embed_resp.text().await.unwrap_or_default();
            return ToolResult::error(format!("Embedding server returned {status}: {body}"));
        }

        let embed_json: serde_json::Value = match embed_resp.json().await {
            Ok(j) => j,
            Err(e) => return ToolResult::error(format!("Failed to parse embedding response: {e}")),
        };

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
                return ToolResult::error("Unexpected embedding response format".into());
            };

        if embedding.is_empty() {
            return ToolResult::error("Embedding server returned empty embedding vector".into());
        }

        // Step 2: Get Collection ID
        let col_resp = match self
            .http_client
            .get(format!(
                "{}/api/v1/collections/{}",
                self.vector_db_url, self.collection
            ))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("ChromaDB unreachable: {e}")),
        };

        if !col_resp.status().is_success() {
            // If not found, try to create it? For now, just error.
            let status = col_resp.status();
            let body = col_resp.text().await.unwrap_or_default();
            return ToolResult::error(format!(
                "ChromaDB collection '{}' not found ({status}): {body}",
                self.collection
            ));
        }

        let col_json: serde_json::Value = match col_resp.json().await {
            Ok(j) => j,
            Err(e) => {
                return ToolResult::error(format!("Failed to parse collection response: {e}"))
            }
        };

        let collection_id = match col_json.get("id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => return ToolResult::error("Could not find collection ID".into()),
        };

        // Step 3: Add to ChromaDB
        let item_id = Uuid::new_v4().to_string();
        let add_resp = match self
            .http_client
            .post(format!(
                "{}/api/v1/collections/{}/add",
                self.vector_db_url, collection_id
            ))
            .json(&json!({
                "ids": [item_id],
                "embeddings": [embedding],
                "metadatas": [{"source": source, "timestamp": chrono::Utc::now().to_rfc3339()}],
                "documents": [content]
            }))
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(format!("ChromaDB add error: {e}")),
        };

        if !add_resp.status().is_success() {
            let status = add_resp.status();
            let body = add_resp.text().await.unwrap_or_default();
            return ToolResult::error(format!("ChromaDB add failed ({status}): {body}"));
        }

        ToolResult::success(format!(
            "Successfully added item to vault (id: {})",
            item_id
        ))
    }
}
