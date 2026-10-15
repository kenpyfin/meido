//! OAuth 2.0 helpers for social media platforms (TikTok, Instagram, LinkedIn).

use crate::config::Config;
use crate::error::FinallyAValueBotError;

/// Build the OAuth redirect base URL from config. Uses social.base_url if set,
/// otherwise derives from web_host:web_port (for local dev).
pub fn oauth_base_url(config: &Config) -> Option<String> {
    let base = config
        .social
        .as_ref()
        .and_then(|s| s.base_url.clone().filter(|u| !u.trim().is_empty()));
    if let Some(b) = base {
        return Some(b);
    }
    if config.social.is_some() && config.web_enabled {
        let host = &config.web_host;
        let port = config.web_port;
        let scheme = if host == "127.0.0.1" || host == "localhost" || host == "::1" {
            "http"
        } else {
            "https"
        };
        return Some(format!("{scheme}://{host}:{port}"));
    }
    None
}

/// Generate authorize URL for a platform. Returns None if platform is not configured.
pub fn authorize_url(
    config: &Config,
    platform: &str,
    state: &str,
) -> Result<Option<String>, FinallyAValueBotError> {
    let Some(base) = oauth_base_url(config) else {
        return Ok(None);
    };
    let social = config.social.as_ref();
    let redirect_uri = format!(
        "{}/api/oauth/callback/{}",
        base.trim_end_matches('/'),
        platform
    );

    let url = match platform {
        "tiktok" => {
            let cfg = social
                .and_then(|s| {
                    if s.tiktok.client_id.is_some() && s.tiktok.client_secret.is_some() {
                        Some(&s.tiktok)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    FinallyAValueBotError::Config("TikTok OAuth not configured".into())
                })?;
            let client_id = cfg.client_id.as_deref().unwrap_or("");
            if client_id.is_empty() {
                return Ok(None);
            }
            let scopes = "user.info.basic,video.list";
            format!(
                "https://www.tiktok.com/v2/auth/authorize/?client_key={}&scope={}&response_type=code&redirect_uri={}&state={}",
                urlencoding::encode(client_id),
                urlencoding::encode(scopes),
                urlencoding::encode(&redirect_uri),
                urlencoding::encode(state),
            )
        }
        "instagram" => {
            let cfg = social
                .and_then(|s| {
                    if s.instagram.client_id.is_some() && s.instagram.client_secret.is_some() {
                        Some(&s.instagram)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    FinallyAValueBotError::Config("Instagram OAuth not configured".into())
                })?;
            let client_id = cfg.client_id.as_deref().unwrap_or("");
            if client_id.is_empty() {
                return Ok(None);
            }
            let scope = "instagram_basic,user_media";
            format!(
                "https://api.instagram.com/oauth/authorize?client_id={}&redirect_uri={}&scope={}&response_type=code&state={}",
                urlencoding::encode(client_id),
                urlencoding::encode(&redirect_uri),
                urlencoding::encode(scope),
                urlencoding::encode(state),
            )
        }
        "linkedin" => {
            let cfg = social
                .and_then(|s| {
                    if s.linkedin.client_id.is_some() && s.linkedin.client_secret.is_some() {
                        Some(&s.linkedin)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    FinallyAValueBotError::Config("LinkedIn OAuth not configured".into())
                })?;
            let client_id = cfg.client_id.as_deref().unwrap_or("");
            if client_id.is_empty() {
                return Ok(None);
            }
            let scope = "openid profile email w_member_social r_organization_social";
            format!(
                "https://www.linkedin.com/oauth/v2/authorization?response_type=code&client_id={}&redirect_uri={}&state={}&scope={}",
                urlencoding::encode(client_id),
                urlencoding::encode(&redirect_uri),
                urlencoding::encode(state),
                urlencoding::encode(scope),
            )
        }
        _ => return Ok(None),
    };

    Ok(Some(url))
}

/// Token exchange result.
#[derive(Debug)]
pub struct TokenResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<String>,
}

/// Exchange authorization code for access token.
pub async fn exchange_code(
    config: &Config,
    platform: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenResult, FinallyAValueBotError> {
    let social = config
        .social
        .as_ref()
        .ok_or_else(|| FinallyAValueBotError::Config("Social OAuth not configured".into()))?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| FinallyAValueBotError::ToolExecution(e.to_string()))?;

    match platform {
        "tiktok" => {
            let client_key =
                social.tiktok.client_id.as_deref().ok_or_else(|| {
                    FinallyAValueBotError::Config("TikTok client_id not set".into())
                })?;
            let client_secret = social.tiktok.client_secret.as_deref().ok_or_else(|| {
                FinallyAValueBotError::Config("TikTok client_secret not set".into())
            })?;

            let params = [
                ("client_key", client_key),
                ("client_secret", client_secret),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ];
            let resp = client
                .post("https://open.tiktokapis.com/v2/oauth/token/")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(&params)
                .send()
                .await
                .map_err(|e| FinallyAValueBotError::ToolExecution(e.to_string()))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| FinallyAValueBotError::ToolExecution(e.to_string()))?;

            if !status.is_success() {
                let err_msg = body
                    .get("error_description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Token exchange failed");
                return Err(FinallyAValueBotError::ToolExecution(err_msg.to_string()));
            }

            let data = body
                .get("data")
                .and_then(|d| d.as_object())
                .ok_or_else(|| {
                    FinallyAValueBotError::ToolExecution("Invalid TikTok token response".into())
                })?;

            let access_token = data
                .get("access_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    FinallyAValueBotError::ToolExecution("No access_token in response".into())
                })?
                .to_string();

            let refresh_token = data
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(String::from);

            let expires_at = data
                .get("expires_in")
                .and_then(|v| v.as_i64())
                .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs))
                .map(|dt| dt.to_rfc3339());

            Ok(TokenResult {
                access_token,
                refresh_token,
                expires_at,
            })
        }
        "instagram" => {
            let client_id = social.instagram.client_id.as_deref().ok_or_else(|| {
                FinallyAValueBotError::Config("Instagram client_id not set".into())
            })?;
            let client_secret = social.instagram.client_secret.as_deref().ok_or_else(|| {
                FinallyAValueBotError::Config("Instagram client_secret not set".into())
            })?;

            let params = [
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("code", code),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ];
            let resp = client
                .post("https://api.instagram.com/oauth/access_token")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(&params)
                .send()
                .await
                .map_err(|e| FinallyAValueBotError::ToolExecution(e.to_string()))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| FinallyAValueBotError::ToolExecution(e.to_string()))?;

            if !status.is_success() {
                let err_msg = body
                    .get("error_message")
                    .and_then(|v| v.as_str())
                    .or_else(|| body.get("error").and_then(|v| v.as_str()))
                    .unwrap_or("Token exchange failed");
                return Err(FinallyAValueBotError::ToolExecution(err_msg.to_string()));
            }

            let access_token = body
                .get("access_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    FinallyAValueBotError::ToolExecution("No access_token in response".into())
                })?
                .to_string();

            Ok(TokenResult {
                access_token,
                refresh_token: None,
                expires_at: None,
            })
        }
        "linkedin" => {
            let client_id = social.linkedin.client_id.as_deref().ok_or_else(|| {
                FinallyAValueBotError::Config("LinkedIn client_id not set".into())
            })?;
            let client_secret = social.linkedin.client_secret.as_deref().ok_or_else(|| {
                FinallyAValueBotError::Config("LinkedIn client_secret not set".into())
            })?;

            let params = [
                ("grant_type", "authorization_code"),
                ("code", code),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("redirect_uri", redirect_uri),
            ];
            let resp = client
                .post("https://www.linkedin.com/oauth/v2/accessToken")
                .header("Content-Type", "application/x-www-form-urlencoded")
                .form(&params)
                .send()
                .await
                .map_err(|e| FinallyAValueBotError::ToolExecution(e.to_string()))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| FinallyAValueBotError::ToolExecution(e.to_string()))?;

            if !status.is_success() {
                let err_msg = body
                    .get("error_description")
                    .and_then(|v| v.as_str())
                    .or_else(|| body.get("error").and_then(|v| v.as_str()))
                    .unwrap_or("Token exchange failed");
                return Err(FinallyAValueBotError::ToolExecution(err_msg.to_string()));
            }

            let access_token = body
                .get("access_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    FinallyAValueBotError::ToolExecution("No access_token in response".into())
                })?
                .to_string();

            let refresh_token = body
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(String::from);

            let expires_at = body
                .get("expires_in")
                .and_then(|v| v.as_i64())
                .map(|secs| chrono::Utc::now() + chrono::Duration::seconds(secs))
                .map(|dt| dt.to_rfc3339());

            Ok(TokenResult {
                access_token,
                refresh_token,
                expires_at,
            })
        }
        _ => Err(FinallyAValueBotError::Config(format!(
            "Unknown platform: {platform}"
        ))),
    }
}
