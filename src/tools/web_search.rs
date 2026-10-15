use std::error::Error;

use async_trait::async_trait;
use serde_json::json;

use super::web_html::extract_ddg_results;
use super::{schema_object, Tool, ToolResult};
use crate::claude::ToolDefinition;

const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";

pub struct WebSearchTool {
    /// When set, use Tavily Search API (recommended for agents).
    pub tavily_api_key: Option<String>,
    /// When set (and Tavily not configured), use this SearXNG instance instead of DuckDuckGo.
    pub searxng_url: Option<String>,
}

impl WebSearchTool {
    pub fn new(tavily_api_key: Option<String>, searxng_url: Option<String>) -> Self {
        Self {
            tavily_api_key,
            searxng_url,
        }
    }

    fn use_tavily(&self) -> bool {
        self.tavily_api_key
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn definition(&self) -> ToolDefinition {
        let backend = if self.use_tavily() {
            "Tavily"
        } else if self.searxng_url.is_some() {
            "SearXNG"
        } else {
            "DuckDuckGo"
        };
        let query_desc = if self.use_tavily() {
            "The search query. Use plain keywords; optional topic/time_range/limit refine results."
        } else {
            "The search query. With SearXNG you can use !category or !engine (e.g. !science, !files, !wp) and syntax like filetype:pdf, site:example.com."
        };
        let mut schema = json!({
            "query": {
                "type": "string",
                "description": query_desc
            }
        });
        if self.use_tavily() {
            schema["limit"] = json!({
                "type": "integer",
                "description": "Max results to return (1-20). Default 8."
            });
            schema["search_depth"] = json!({
                "type": "string",
                "description": "Tavily search depth: basic, advanced, fast, or ultra-fast (latency vs. quality). Default basic."
            });
            schema["topic"] = json!({
                "type": "string",
                "description": "Tavily topic: general, news, or finance. Default general. You can also set categories to include \"news\" to imply news."
            });
            schema["time_range"] = json!({
                "type": "string",
                "description": "Tavily time filter: day, week, month, or year (optional)."
            });
            schema["include_answer"] = json!({
                "type": "boolean",
                "description": "If true, include Tavily's LLM summary answer when available (uses extra credits when applicable)."
            });
            schema["categories"] = json!({
                "type": "string",
                "description": "Optional hint: if this contains \"news\", topic defaults to news (same as SearXNG-style category hints)."
            });
        } else if self.searxng_url.is_some() {
            schema["categories"] = json!({
                "type": "string",
                "description": "SearXNG categories to search: general, science, files, images, it, news, map, music, social, videos, etc. Comma-separated. Overrides default."
            });
            schema["engines"] = json!({
                "type": "string",
                "description": "SearXNG engines to use (e.g. google, semantic_scholar, libgen). Comma-separated. Optional."
            });
            schema["time_range"] = json!({
                "type": "string",
                "description": "Limit results to time range: day, week, month, year. Optional."
            });
            schema["language"] = json!({
                "type": "string",
                "description": "Preferred search language/locale for SearXNG (examples: zh, zh-CN, en, all). Optional."
            });
            schema["limit"] = json!({
                "type": "integer",
                "description": "Max number of returned results (1-20). Optional; default is 8."
            });
            schema["safesearch"] = json!({
                "type": "integer",
                "description": "SearXNG safesearch level: 0 (off), 1 (moderate), 2 (strict). Optional."
            });
        }
        let desc_suffix = if self.use_tavily() {
            " Set TAVILY_API_KEY in .env. Optional: search_depth, topic, time_range, limit, include_answer."
        } else if self.searxng_url.is_some() {
            " Set SEARXNG_URL in .env to use SearXNG (supports categories, engines, time_range, language, limit, safesearch)."
        } else {
            " For reliable search, set TAVILY_API_KEY (Tavily) or SEARXNG_URL (SearXNG)."
        };
        ToolDefinition::new(
            "web_search",
            format!(
                "Search the web using {}. Returns titles, URLs, and snippets.{desc_suffix}",
                backend
            ),
            schema_object(schema, &["query"]),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return ToolResult::error("Missing required parameter: query".into()),
        };

        let result = if self.use_tavily() {
            let key = self.tavily_api_key.as_deref().unwrap_or("").trim();
            search_tavily(key, query, &input).await
        } else if let Some(ref base) = self.searxng_url {
            let categories = input
                .get("categories")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty());
            let engines = input
                .get("engines")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty());
            let time_range = input
                .get("time_range")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty());
            let language = input
                .get("language")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty());
            let limit = input
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|v| v.clamp(1, 20) as usize);
            let safesearch = input
                .get("safesearch")
                .and_then(|v| v.as_u64())
                .map(|v| v.clamp(0, 2));
            search_searxng(
                base, query, categories, engines, time_range, language, limit, safesearch,
            )
            .await
        } else {
            search_ddg(query).await
        };

        match result {
            Ok(results) => {
                if results.is_empty() {
                    let msg = if self.use_tavily() {
                        "No results found.".into()
                    } else if self.searxng_url.is_some() {
                        "No results found.".into()
                    } else {
                        "No results found. DuckDuckGo may be blocking or returning empty results. For reliable search, set TAVILY_API_KEY (Tavily) or SEARXNG_URL in .env.".into()
                    };
                    ToolResult::success(msg)
                } else {
                    ToolResult::success(results)
                }
            }
            Err(e) => ToolResult::error(format!("Search failed: {e}")),
        }
    }
}

fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .timeout(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| {
            let msg = e.to_string();
            let detail = e
                .source()
                .map(|s: &dyn Error| s.to_string())
                .unwrap_or_else(String::new);
            if detail.is_empty() {
                msg
            } else {
                format!("{msg}: {detail}")
            }
        })
}

async fn search_tavily(
    api_key: &str,
    query: &str,
    input: &serde_json::Value,
) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("TAVILY_API_KEY is empty".into());
    }

    let max_results = input
        .get("limit")
        .and_then(|v| v.as_u64())
        .map(|v| v.clamp(1, 20))
        .unwrap_or(8);

    let search_depth = input
        .get("search_depth")
        .and_then(|v| v.as_str())
        .filter(|s| {
            matches!(
                s.to_ascii_lowercase().as_str(),
                "advanced" | "basic" | "fast" | "ultra-fast"
            )
        })
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| "basic".to_string());

    let topic = if let Some(t) = input
        .get("topic")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match t.to_ascii_lowercase().as_str() {
            "general" | "news" | "finance" => t.to_ascii_lowercase(),
            _ => "general".to_string(),
        }
    } else if let Some(cats) = input.get("categories").and_then(|v| v.as_str()) {
        let lower = cats.to_ascii_lowercase();
        if lower.contains("news") {
            "news".to_string()
        } else if lower.contains("finance") {
            "finance".to_string()
        } else {
            "general".to_string()
        }
    } else {
        "general".to_string()
    };

    let mut body = json!({
        "api_key": api_key,
        "query": query,
        "max_results": max_results,
        "search_depth": search_depth,
        "topic": topic,
    });

    if let Some(tr) = input
        .get("time_range")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let tr_norm = match tr.to_ascii_lowercase().as_str() {
            "day" | "week" | "month" | "year" => tr.to_ascii_lowercase(),
            "d" => "day".to_string(),
            "w" => "week".to_string(),
            "m" => "month".to_string(),
            "y" => "year".to_string(),
            _ => String::new(),
        };
        if !tr_norm.is_empty() {
            body["time_range"] = json!(tr_norm);
        }
    }

    if let Some(b) = input.get("include_answer").and_then(|v| v.as_bool()) {
        body["include_answer"] = json!(b);
    }

    let client = build_http_client()?;

    let resp = client
        .post(TAVILY_SEARCH_URL)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        let preview: String = text.chars().take(500).collect();
        return Err(format!("Tavily returned HTTP {status}: {preview}"));
    }

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let mut output = String::new();

    if let Some(ans) = data.get("answer").and_then(|v| v.as_str()) {
        if !ans.trim().is_empty() {
            output.push_str("Answer: ");
            output.push_str(ans.trim());
            output.push_str("\n\n");
        }
    }

    let results: &[serde_json::Value] = data
        .get("results")
        .and_then(|v| v.as_array())
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    for (i, item) in results.iter().enumerate() {
        let title = item
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let url = item
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let content = item
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if url.is_empty() && title.is_empty() {
            continue;
        }
        output.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            title,
            url,
            content
        ));
    }

    if output.trim().is_empty() {
        return Ok("No results found.\nHint: try a shorter query, different topic (e.g. news), or search_depth basic.".to_string());
    }

    Ok(output)
}

async fn search_ddg(query: &str) -> Result<String, String> {
    // DuckDuckGo blocks GET/bot requests (403). Use POST with form data and browser-like
    // headers as used by no-JS clients (see SearXNG DDG engine docs).
    let url = "https://html.duckduckgo.com/html/";

    let client = build_http_client()?;

    let form = [("q", query), ("b", ""), ("kl", "wt-wt")];

    let resp = client
        .post(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header("Referer", "https://html.duckduckgo.com/")
        .header("Sec-Fetch-Mode", "navigate")
        .header("Sec-Fetch-Dest", "document")
        .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&form)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!(
            "DuckDuckGo returned HTTP {} (bot detection may be blocking; try again later)",
            resp.status()
        ));
    }

    let body = resp.text().await.map_err(|e| e.to_string())?;
    let items = extract_ddg_results(&body, 8);

    let mut output = String::new();
    for (i, item) in items.iter().enumerate() {
        output.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            item.title,
            item.url,
            item.snippet
        ));
    }

    Ok(output)
}

async fn search_searxng(
    base_url: &str,
    query: &str,
    categories: Option<&str>,
    engines: Option<&str>,
    time_range: Option<&str>,
    language: Option<&str>,
    limit: Option<usize>,
    safesearch: Option<u64>,
) -> Result<String, String> {
    let base = base_url.trim_end_matches('/');
    let encoded = urlencoding::encode(query);
    let mut url = format!("{base}/search?q={encoded}&format=json");
    if let Some(c) = categories {
        url.push_str("&categories=");
        url.push_str(&urlencoding::encode(c));
    }
    if let Some(e) = engines {
        url.push_str("&engines=");
        url.push_str(&urlencoding::encode(e));
    }
    if let Some(tr) = time_range {
        let tr_lower = tr.trim().to_ascii_lowercase();
        let tr_val = match tr_lower.as_str() {
            "day" | "week" | "month" | "year" => tr_lower,
            _ => "month".to_string(),
        };
        url.push_str("&time_range=");
        url.push_str(&urlencoding::encode(&tr_val));
    }
    if let Some(lang) = language {
        url.push_str("&language=");
        url.push_str(&urlencoding::encode(lang));
    }
    if let Some(level) = safesearch {
        url.push_str("&safesearch=");
        url.push_str(&level.to_string());
    }

    let client = build_http_client()?;

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let msg = if status.as_u16() == 429 {
            "SearXNG instance rate limited (429 Too Many Requests). Use another instance (SEARXNG_URL), self-host, or try again later."
        } else {
            "SearXNG returned HTTP "
        };
        return Err(format!("{}{}.", msg, status));
    }

    let body = resp.text().await.map_err(|e| e.to_string())?;
    let data: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    let results: &[serde_json::Value] = data
        .get("results")
        .and_then(|v| v.as_array())
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    let mut output = String::new();
    let result_limit = limit.unwrap_or(8);
    for (i, item) in results.iter().take(result_limit).enumerate() {
        let title = item
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let url = item
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let content = item
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if url.is_empty() && title.is_empty() {
            continue;
        }
        output.push_str(&format!(
            "{}. {}\n   {}\n   {}\n\n",
            i + 1,
            title,
            url,
            content
        ));
    }

    if output.trim().is_empty() {
        return Ok(build_searxng_no_results_message(&data));
    }

    Ok(output)
}

fn build_searxng_no_results_message(data: &serde_json::Value) -> String {
    let mut lines = vec!["No results found.".to_string()];

    let unresponsive = data
        .get("unresponsive_engines")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .take(6)
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|s| !s.is_empty());

    if let Some(engines) = unresponsive {
        lines.push(format!("Unresponsive engines: {engines}."));
    }

    let answers = data
        .get("answers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .take(2)
                .filter_map(|item| item.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .filter(|s| !s.is_empty());

    if let Some(answer_text) = answers {
        lines.push(format!("Direct answers: {answer_text}"));
    } else {
        lines.push(
            "Hint: simplify the query (drop quotes/site filters), then retry with category=general or set language explicitly (e.g., zh-CN)."
                .to_string(),
        );
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_web_search_definition_ddg() {
        let tool = WebSearchTool::new(None, None);
        assert_eq!(tool.name(), "web_search");
        let def = tool.definition();
        assert_eq!(def.name, "web_search");
        assert!(def.description.contains("DuckDuckGo"));
        assert!(def.input_schema["properties"]["query"].is_object());
        let required = def.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "query"));
    }

    #[test]
    fn test_web_search_definition_tavily_fields() {
        let tool = WebSearchTool::new(Some("tvly-test".into()), None);
        let def = tool.definition();
        assert!(def.description.contains("Tavily"));
        assert!(def.input_schema["properties"]["search_depth"].is_object());
        assert!(def.input_schema["properties"]["topic"].is_object());
        assert!(def.input_schema["properties"]["limit"].is_object());
    }

    #[test]
    fn test_web_search_definition_searxng_fields() {
        let tool = WebSearchTool::new(None, Some("https://search.example.org".into()));
        let def = tool.definition();
        assert!(def.description.contains("SearXNG"));
        assert!(def.input_schema["properties"]["categories"].is_object());
        assert!(def.input_schema["properties"]["engines"].is_object());
        assert!(def.input_schema["properties"]["time_range"].is_object());
        assert!(def.input_schema["properties"]["language"].is_object());
        assert!(def.input_schema["properties"]["limit"].is_object());
        assert!(def.input_schema["properties"]["safesearch"].is_object());
    }

    #[test]
    fn test_searxng_no_results_message_has_hint() {
        let msg = build_searxng_no_results_message(&json!({}));
        assert!(msg.contains("No results found."));
        assert!(msg.contains("Hint: simplify the query"));
    }

    #[test]
    fn test_searxng_no_results_message_includes_unresponsive() {
        let msg = build_searxng_no_results_message(&json!({
            "unresponsive_engines": ["google", "brave"]
        }));
        assert!(msg.contains("Unresponsive engines: google, brave."));
    }

    #[tokio::test]
    async fn test_web_search_missing_query() {
        let tool = WebSearchTool::new(None, None);
        let result = tool.execute(json!({})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter: query"));
    }

    #[tokio::test]
    async fn test_web_search_null_query() {
        let tool = WebSearchTool::new(None, None);
        let result = tool.execute(json!({"query": null})).await;
        assert!(result.is_error);
        assert!(result.content.contains("Missing required parameter: query"));
    }
}
