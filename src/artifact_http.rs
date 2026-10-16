//! Bounded file streaming shared by artifact IDs and historical aliases.
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use std::path::Path as FsPath;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
type ApiError = (StatusCode, String);
fn error(e: impl std::fmt::Display) -> ApiError {
    (StatusCode::BAD_REQUEST, e.to_string())
}
// Query strings need direct numeric fields: serde(flatten) loses the URL
// deserializer's string-to-number conversion for nested fields.
#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub chat_id: Option<i64>,
    pub persona_id: Option<i64>,
    pub q: Option<String>,
    pub kind: Option<String>,
    pub status: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub job_id: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}
impl ListQuery {
    pub fn into_filter(self) -> crate::artifacts::Filter {
        crate::artifacts::Filter {
            q: self.q,
            kind: self.kind,
            status: self.status,
            before: self.before,
            after: self.after,
            job_id: self.job_id,
            cursor: self.cursor,
            limit: self.limit,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct ContentOptions {
    #[serde(default, deserialize_with = "upload_bool")]
    pub preview: Option<bool>,
    #[serde(default, deserialize_with = "upload_bool")]
    pub download: Option<bool>,
}
fn upload_bool<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|v| match v.as_str() {
            "1" | "true" => Ok(true),
            "0" | "false" => Ok(false),
            _ => Err(serde::de::Error::custom("Expected true, false, 1 or 0")),
        })
        .transpose()
}

pub async fn serve_file(
    path: &FsPath,
    name: &str,
    headers: &HeaderMap,
    method: &Method,
    query: &ContentOptions,
) -> Result<Response, ApiError> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Artifact bytes are missing".into()))?;
    let size = file.metadata().await.map_err(error)?.len();
    let mime = guess_upload_content_type(FsPath::new(name));
    let preview = query.preview.unwrap_or(false);
    let text_preview = preview && matches!(crate::artifacts::kind(name), "text" | "markdown");
    let mut start = 0;
    let mut length = if text_preview { size.min(262144) } else { size };
    let mut status = StatusCode::OK;
    if let Some(range) = headers
        .get("range")
        .filter(|_| !text_preview)
        .and_then(|r| r.to_str().ok())
    {
        let range_result = parse_range(range, size);
        match range_result {
            Some((s, e)) => {
                start = s;
                length = e - s + 1;
                status = StatusCode::PARTIAL_CONTENT;
            }
            None => {
                return Ok((
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [("content-range", format!("bytes */{size}"))],
                )
                    .into_response());
            }
        }
    }
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(error)?;
    let stream = async_stream::try_stream! {
        let mut remaining=length;let mut buf=vec![0u8;65536];
        while remaining>0 {
            let cap=remaining.min(buf.len() as u64) as usize;
            let n=file.read(&mut buf[..cap]).await?;
            if n==0 {Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,"Artifact truncated"))?;}
            remaining-=n as u64;
            yield buf[..n].to_vec();
        }
    };
    let stream: std::pin::Pin<
        Box<dyn futures_util::Stream<Item = Result<Vec<u8>, std::io::Error>> + Send>,
    > = Box::pin(stream);
    let mut response = Response::new(if *method == Method::HEAD {
        Body::empty()
    } else {
        Body::from_stream(stream)
    });
    *response.status_mut() = status;
    let out = response.headers_mut();
    out.insert("content-type", HeaderValue::from_static(mime));
    out.insert(
        "content-length",
        HeaderValue::from_str(&length.to_string()).map_err(error)?,
    );
    out.insert("accept-ranges", HeaderValue::from_static("bytes"));
    out.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    out.insert(
        "cache-control",
        HeaderValue::from_static("private, no-store"),
    );
    if let Some(policy) = document_security_policy(name) {
        out.insert("content-security-policy", HeaderValue::from_static(policy));
    }
    let download =
        query.download.unwrap_or(false) || (!preview && crate::artifacts::kind(name) == "html");
    let disposition = format!(
        "{}; filename*=UTF-8''{}",
        if download { "attachment" } else { "inline" },
        urlencoding::encode(name)
    );
    out.insert(
        "content-disposition",
        HeaderValue::from_str(&disposition).map_err(error)?,
    );
    if status == StatusCode::PARTIAL_CONTENT {
        out.insert(
            "content-range",
            HeaderValue::from_str(&format!("bytes {}-{}/{size}", start, start + length - 1))
                .map_err(error)?,
        );
    }
    Ok(response)
}
/// Sandbox only documents that can run script when opened directly.
/// Chrome applies this header to its own video, audio, image, and PDF viewers,
/// and `sandbox` plus `default-src 'none'` then blocks playback.
fn document_security_policy(name: &str) -> Option<&'static str> {
    matches!(
        FsPath::new(name)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("html" | "htm" | "svg")
    )
    .then_some("default-src 'none'; img-src data: blob: https:; style-src 'unsafe-inline'; sandbox")
}

fn parse_range(value: &str, size: u64) -> Option<(u64, u64)> {
    if size == 0 {
        return None;
    }
    let (a, b) = value.strip_prefix("bytes=")?.split_once('-')?;
    if a.is_empty() {
        let n = b.parse::<u64>().ok()?;
        if n == 0 {
            return None;
        }
        return Some((size.saturating_sub(n), size - 1));
    }
    let start = a.parse::<u64>().ok()?;
    let end = if b.is_empty() {
        size - 1
    } else {
        b.parse::<u64>().ok()?.min(size - 1)
    };
    (start < size && end >= start).then_some((start, end))
}

fn guess_upload_content_type(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("bmp") => "image/bmp",
        Some("md") | Some("markdown") => "text/markdown; charset=utf-8",
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("m4a") => "audio/mp4",
        Some("flac") => "audio/flac",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=2-5", 10), Some((2, 5)));
        assert_eq!(parse_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_range("bytes=10-", 10), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 10), None);
    }
}
