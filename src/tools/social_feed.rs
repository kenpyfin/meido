//! Social media feed tools: TikTok, Instagram, LinkedIn.
//! Fetches user feeds via official APIs. Own-feed requires one-time OAuth per user.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use super::{auth_context_from_input, schema_object, Tool, ToolResult};
use crate::claude::ToolDefinition;
use crate::config::Config;
use crate::db::{call_blocking, Database};
use crate::social_oauth;

fn authorize_msg(platform: &str, url: &str) -> String {
    format!(
        "To fetch your {} feed, you must authorize first. Click this link to connect: {}",
        platform, url
    )
}

/// Shared logic: resolve chat_id, check token, return authorize message if needed.
async fn get_token_or_authorize(
    config: &Config,
    db: Arc<Database>,
    platform: &str,
    input: &serde_json::Value,
) -> Result<String, ToolResult> {
    let auth = match auth_context_from_input(input) {
        Some(a) => a,
        None => return Err(ToolResult::error("Missing auth context".into())),
    };
    let chat_id = auth.caller_chat_id;
    let platform_owned = platform.to_string();

    let token_opt =
        match call_blocking(db, move |db| db.get_social_token(&platform_owned, chat_id)).await {
            Ok(opt) => opt.map(|t| t.access_token),
            Err(e) => return Err(ToolResult::error(e.to_string())),
        };

    if let Some(t) = token_opt {
        return Ok(t);
    }

    let base =
        social_oauth::oauth_base_url(config).unwrap_or_else(|| "http://127.0.0.1:10961".into());
    let auth_path = format!(
        "{}/api/oauth/authorize/{}",
        base.trim_end_matches('/'),
        platform
    );
    let url = format!(
        "{}?chat_id={}",
        auth_path,
        urlencoding::encode(&chat_id.to_string())
    );
    Err(ToolResult::error(authorize_msg(platform, &url)))
}

// --- TikTok ---

pub struct FetchTiktokFeedTool {
    config: Config,
    db: Arc<Database>,
}

impl FetchTiktokFeedTool {
    pub fn new(config: &Config, db: Arc<Database>) -> Self {
        FetchTiktokFeedTool {
            config: config.clone(),
            db,
        }
    }
}

#[async_trait]
impl Tool for FetchTiktokFeedTool {
    fn name(&self) -> &str {
        "fetch_tiktok_feed"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "fetch_tiktok_feed",
            "Fetch TikTok videos from the user's feed. Requires one-time OAuth authorization per user. Omit username to fetch the caller's own videos. Public profile by username is not supported by the TikTok API.",
            schema_object(
                json!({
                    "username": {
                        "type": "string",
                        "description": "Optional. Username for public profile; omit to fetch your own feed (requires prior OAuth). TikTok API only supports own feed."
                    },
                    "max_items": {
                        "type": "integer",
                        "description": "Max videos to return (default 10, max 20)"
                    },
                    "cursor": {
                        "type": "string",
                        "description": "Pagination cursor from previous response"
                    }
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        if input
            .get("username")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            return ToolResult::error(
                "Public profile fetch by username is not supported by the TikTok API. Omit username to fetch your own feed.".into(),
            );
        }

        let token =
            match get_token_or_authorize(&self.config, self.db.clone(), "tiktok", &input).await {
                Ok(t) => t,
                Err(e) => return e,
            };

        let max_count = input
            .get("max_items")
            .and_then(|v| v.as_i64())
            .unwrap_or(10)
            .clamp(1, 20) as i64;
        let cursor = input.get("cursor").and_then(|v| v.as_str());

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let mut body = json!({
            "max_count": max_count,
            "fields": "id,title,create_time,cover_image_url,video_description,view_count,like_count,comment_count,share_count,duration,embed_link"
        });
        if let Some(c) = cursor {
            body["cursor"] = json!(c);
        }

        let resp = match client
            .post("https://open.tiktokapis.com/v2/video/list/")
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let status = resp.status();
        let body: serde_json::Value = match resp.json().await {
            Ok(b) => b,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        if !status.is_success() {
            let err = body
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("API request failed");
            return ToolResult::error(format!("TikTok API error: {}", err));
        }

        let videos = body
            .get("data")
            .and_then(|d| d.get("videos"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let next_cursor = body
            .get("data")
            .and_then(|d| d.get("cursor"))
            .and_then(|c| c.as_str())
            .map(String::from);

        let mut out = serde_json::Map::new();
        out.insert("videos".into(), json!(videos));
        if let Some(c) = next_cursor {
            out.insert("next_cursor".into(), json!(c));
        }
        out.insert("count".into(), json!(videos.len()));

        ToolResult::success(serde_json::to_string_pretty(&serde_json::Value::Object(out)).unwrap())
    }
}

// --- Instagram ---

pub struct FetchInstagramFeedTool {
    config: Config,
    db: Arc<Database>,
}

impl FetchInstagramFeedTool {
    pub fn new(config: &Config, db: Arc<Database>) -> Self {
        FetchInstagramFeedTool {
            config: config.clone(),
            db,
        }
    }
}

#[async_trait]
impl Tool for FetchInstagramFeedTool {
    fn name(&self) -> &str {
        "fetch_instagram_feed"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "fetch_instagram_feed",
            "Fetch Instagram media from the user's feed. Requires one-time OAuth authorization per user. Omit username to fetch your own feed. Public profile by username has limited API support.",
            schema_object(
                json!({
                    "username": {
                        "type": "string",
                        "description": "Optional. Username for public profile; omit to fetch your own feed (requires prior OAuth)"
                    },
                    "max_items": {
                        "type": "integer",
                        "description": "Max posts to return (default 10)"
                    },
                    "cursor": {
                        "type": "string",
                        "description": "Pagination cursor from previous response"
                    }
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        if input
            .get("username")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            return ToolResult::error(
                "Public profile fetch by username is not supported. Omit username to fetch your own feed.".into(),
            );
        }

        let token = match get_token_or_authorize(&self.config, self.db.clone(), "instagram", &input)
            .await
        {
            Ok(t) => t,
            Err(e) => return e,
        };

        // Instagram Graph API: need ig-user-id first. Get /me?fields=id to get user id, then /{id}/media
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let resp = match client
            .get("https://graph.instagram.com/me")
            .query(&[("fields", "id"), ("access_token", &token)])
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        let me: serde_json::Value = match resp.json().await {
            Ok(j) => j,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        if let Some(err) = me.get("error") {
            let msg = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Instagram API error");
            return ToolResult::error(format!("Instagram API: {}", msg));
        }

        let user_id = match me.get("id").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Could not get Instagram user ID".into()),
        };

        let limit = input
            .get("max_items")
            .and_then(|v| v.as_i64())
            .unwrap_or(10)
            .clamp(1, 50);
        let limit_str = limit.to_string();
        let mut params = vec![
            (
                "fields",
                "id,caption,media_type,media_url,permalink,timestamp",
            ),
            ("limit", limit_str.as_str()),
        ];
        if let Some(c) = input.get("cursor").and_then(|v| v.as_str()) {
            params.push(("after", c));
        }

        let media_resp = match client
            .get(format!("https://graph.instagram.com/{}/media", user_id))
            .query(&params)
            .query(&[("access_token", token)])
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        let media: serde_json::Value = match media_resp.json().await {
            Ok(j) => j,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let data = media
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();
        let next = media
            .get("paging")
            .and_then(|p| p.get("cursors"))
            .and_then(|c| c.get("after"))
            .and_then(|a| a.as_str())
            .map(String::from);

        let mut out = serde_json::Map::new();
        out.insert("media".into(), json!(data));
        if let Some(n) = next {
            out.insert("next_cursor".into(), json!(n));
        }
        out.insert("count".into(), json!(data.len()));

        ToolResult::success(serde_json::to_string_pretty(&serde_json::Value::Object(out)).unwrap())
    }
}

// --- LinkedIn ---

pub struct FetchLinkedinFeedTool {
    config: Config,
    db: Arc<Database>,
}

impl FetchLinkedinFeedTool {
    pub fn new(config: &Config, db: Arc<Database>) -> Self {
        FetchLinkedinFeedTool {
            config: config.clone(),
            db,
        }
    }
}

#[async_trait]
impl Tool for FetchLinkedinFeedTool {
    fn name(&self) -> &str {
        "fetch_linkedin_feed"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "fetch_linkedin_feed",
            "Fetch LinkedIn posts from the user's feed. Requires one-time OAuth authorization per user. Omit username to fetch your own posts. Public profile by username is not supported.",
            schema_object(
                json!({
                    "username": {
                        "type": "string",
                        "description": "Optional. Username for public profile; omit to fetch your own feed (requires prior OAuth)"
                    },
                    "max_items": {
                        "type": "integer",
                        "description": "Max posts to return (default 10)"
                    }
                }),
                &[],
            ),
        )
    }

    async fn execute(&self, input: serde_json::Value) -> ToolResult {
        if input
            .get("username")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
        {
            return ToolResult::error(
                "Public profile fetch by username is not supported. Omit username to fetch your own feed.".into(),
            );
        }

        let token =
            match get_token_or_authorize(&self.config, self.db.clone(), "linkedin", &input).await {
                Ok(t) => t,
                Err(e) => return e,
            };

        // LinkedIn Posts API: GET /posts with author URN
        // First get current user URN via /me
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let me_resp = match client
            .get("https://api.linkedin.com/v2/me")
            .header("Authorization", format!("Bearer {}", token))
            .header("Linkedin-Version", "202401")
            .header("X-Restli-Protocol-Version", "2.0.0")
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        let me: serde_json::Value = match me_resp.json().await {
            Ok(j) => j,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let id = match me.get("id").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => return ToolResult::error("Could not get LinkedIn user ID".into()),
        };
        let author_urn = format!("urn:li:person:{}", id);

        let count = input
            .get("max_items")
            .and_then(|v| v.as_i64())
            .unwrap_or(10)
            .clamp(1, 100);

        let count_str = count.to_string();
        let params = [
            ("author", author_urn.as_str()),
            ("count", count_str.as_str()),
        ];

        let posts_resp = match client
            .get("https://api.linkedin.com/rest/posts")
            .header("Authorization", format!("Bearer {}", token))
            .header("Linkedin-Version", "202401")
            .header("X-Restli-Protocol-Version", "2.0.0")
            .query(&params)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return ToolResult::error(e.to_string()),
        };
        let posts: serde_json::Value = match posts_resp.json().await {
            Ok(j) => j,
            Err(e) => return ToolResult::error(e.to_string()),
        };

        let elements = posts
            .get("elements")
            .and_then(|e| e.as_array())
            .cloned()
            .unwrap_or_default();

        let mut out = serde_json::Map::new();
        out.insert("posts".into(), json!(elements));
        out.insert("count".into(), json!(elements.len()));

        ToolResult::success(serde_json::to_string_pretty(&serde_json::Value::Object(out)).unwrap())
    }
}
