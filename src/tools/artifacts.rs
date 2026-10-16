use super::{auth_context_from_input, schema_object, Tool, ToolResult};
use crate::artifacts::{Catalog, Filter};
use crate::claude::ToolDefinition;
use async_trait::async_trait;
use serde_json::json;
use std::path::PathBuf;

pub struct ArtifactTool {
    pub root: PathBuf,
    pub origin_vault: PathBuf,
    pub operation: &'static str,
    pub db: std::sync::Arc<crate::db::Database>,
}
#[async_trait]
impl Tool for ArtifactTool {
    fn name(&self) -> &str {
        self.operation
    }
    fn definition(&self) -> ToolDefinition {
        let (description, properties, required) = match self.operation {
            "publish_artifact" => (
                "Publish a finished output as an immutable downloadable artifact. Returns a verified server-generated link; use it verbatim. The delivery layer automatically attaches publications from this run. Existing identical publication is reused. Use supersedes only for an explicitly replaced earlier output.",
                json!({"path":{"type":"string","description":"Exact completed source file path"},"job_id":{"type":"string","description":"Optional gateway background job ID"},"supersedes":{"type":"string","description":"Optional artifact ID explicitly replaced by this output"}}),
                vec!["path"],
            ),
            "find_artifacts" => (
                "Find this persona's published files by filename or producing job. Returns recent job status as well. Use when asked for an earlier output/link or job status. Resolve the requested job/take from conversation; clarify ambiguity rather than choosing by recency.",
                json!({"q":{"type":"string"},"job_id":{"type":"string"},"cursor":{"type":"string"},"kind":{"type":"string"},"before":{"type":"string"},"after":{"type":"string"}}),
                vec![],
            ),
            _ => (
                "Get a published artifact by ID, including its verified delivery link and availability.",
                json!({"id":{"type":"string"}}),
                vec!["id"],
            ),
        };
        ToolDefinition::new(
            self.operation,
            description,
            schema_object(properties, &required),
        )
    }
    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let Some(auth) = auth_context_from_input(&input) else {
            return ToolResult::error("Missing trusted persona context".into());
        };
        let root = self.root.clone();
        let origin_vault = self.origin_vault.clone();
        let op = self.operation;
        let db = self.db.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<serde_json::Value> {
            let mut catalog = Catalog::open(&root)?;
            let cid = auth.caller_chat_id;
            let pid = auth.caller_persona_id;
            match op {
                "publish_artifact" => {
                    let raw = input["path"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("path is required"))?;
                    let cwd = super::resolve_tool_working_dir_for_auth(&root, Some(&auth));
                    let path = super::resolve_tool_path_with_origin(
                        &root,
                        &cwd,
                        raw,
                        &origin_vault,
                        auth.vector_db_root.as_deref(),
                    );
                    super::assert_persona_tool_path_allowed_with_origin(
                        &root,
                        &path,
                        Some(&auth),
                        false,
                        &origin_vault,
                    )
                    .map_err(anyhow::Error::msg)?;
                    let job=input["job_id"].as_str();
                    if let Some(id)=job {
                        let job=db.get_background_job(id)?.ok_or_else(||anyhow::anyhow!("Unknown job"))?;
                        if job.chat_id!=cid||job.persona_id!=pid {anyhow::bail!("Job does not belong to this persona")}
                    }
                    if let Some(previous)=input["supersedes"].as_str() {
                        let previous=catalog.get(previous)?;
                        if previous.chat_id!=cid||previous.persona_id!=pid {anyhow::bail!("Replacement must belong to the same persona")}
                    }
                    let a=catalog.publish(cid,pid,&path,"generated")?;
                    catalog.associate(&a.id, job.or(auth.run_id.as_deref()), auth.session_id.as_deref(), None)?;
                    if let Some(run_id)=auth.run_id.as_deref(){catalog.associate_run(&a.id,run_id)?;}
                    if let Some(previous)=input["supersedes"].as_str(){
                        if previous!=a.id{catalog.update(previous,None,None,Some(&a.id))?;}
                    }
                    Ok(serde_json::to_value(catalog.get(&a.id)?)?)
                }
                "find_artifacts" => {
                    let filter: Filter = serde_json::from_value(input)?;
                    let page=catalog.list(cid,pid,&filter)?;
                    let jobs=db.list_artifact_jobs(cid,pid,auth.session_id.as_deref())?.into_iter()
                        .map(|j|json!({"id":j.id,"label":j.label,"status":j.status,"created_at":j.created_at})).collect::<Vec<_>>();
                    Ok(json!({"artifacts":page.artifacts,"next_cursor":page.next_cursor,"recent_jobs":jobs,"indexing":!catalog.migration_complete(&format!("{cid}/{pid}"))?}))
                }
                _ => {
                    let a = catalog.get(
                        input["id"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("id is required"))?,
                    )?;
                    if a.chat_id != cid || a.persona_id != pid {
                        anyhow::bail!("Artifact does not belong to this persona")
                    }
                    Ok(serde_json::to_value(a)?)
                }
            }
        })
        .await;
        match result {
            Ok(Ok(value)) => ToolResult::success(value.to_string()),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}
