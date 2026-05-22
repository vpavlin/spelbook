//! Logos Storage (Codex) operation implementations.
//! Uses the Codex HTTP REST API: POST /api/storage/v1/data, GET /api/storage/v1/data/{cid}/...

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};

fn parse_args(args: &str) -> Result<Value, String> {
    serde_json::from_str(args).map_err(|e| format!("invalid JSON: {}", e))
}

fn get_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key].as_str().ok_or_else(|| format!("missing field '{}'", key))
}

/// Upload a file to Logos Storage (Codex).
///
/// Args JSON:
/// ```json
/// {
///   "logos_storage_url": "http://localhost:8080",
///   "file_path": "/path/to/file.json"
/// }
/// ```
///
/// Returns:
/// ```json
/// {"success": true, "cid": "bafy..."}
/// ```
pub fn upload(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let logos_storage_url = match get_str(&v, "logos_storage_url") {
        Ok(u) => u.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let file_path = match get_str(&v, "file_path") {
        Ok(p) => p.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return json!({"success": false, "error": format!("runtime error: {}", e)}).to_string(),
    };

    rt.block_on(async move { upload_async(&logos_storage_url, &file_path).await })
}

pub async fn upload_async(logos_storage_url: &str, file_path: &str) -> String {
    let file_bytes = match tokio::fs::read(file_path).await {
        Ok(b) => b,
        Err(e) => {
            return json!({"success": false, "error": format!("cannot read file '{}': {}", file_path, e)}).to_string()
        }
    };

    let filename = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload")
        .to_string();

    let url = format!("{}/api/storage/v1/data", logos_storage_url.trim_end_matches('/'));

    // New Logos Storage API: raw streaming upload, no multipart
    // Content-Type: application/octet-stream (or specific mime type)
    // Content-Disposition: attachment; filename="<filename>"
    let client = reqwest::Client::new();
    let resp = match client
        .post(&url)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Disposition", format!("attachment; filename=\"{}\"", filename))
        .body(file_bytes)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return json!({"success": false, "error": format!("HTTP request failed: {}", e)}).to_string(),
    };

    let status = resp.status();
    let body = match resp.text().await {
        Ok(b) => b,
        Err(e) => return json!({"success": false, "error": format!("failed to read response: {}", e)}).to_string(),
    };

    if !status.is_success() {
        return json!({"success": false, "error": format!("upload failed (HTTP {}): {}", status, body)}).to_string();
    }

    // Parse CID from response — Codex returns {"cid": "bafy..."}
    match serde_json::from_str::<Value>(&body) {
        Ok(resp_json) => {
            if let Some(cid) = resp_json["cid"].as_str() {
                json!({"success": true, "cid": cid}).to_string()
            } else {
                // Some Codex versions return just the CID as a string
                let cid = body.trim().trim_matches('"');
                json!({"success": true, "cid": cid}).to_string()
            }
        }
        Err(_) => {
            // Plain text CID response
            let cid = body.trim().trim_matches('"');
            json!({"success": true, "cid": cid}).to_string()
        }
    }
}

/// Download content from Logos Storage by CID.
///
/// Args JSON:
/// ```json
/// {
///   "logos_storage_url": "http://localhost:8080",
///   "cid": "bafy..."
/// }
/// ```
///
/// Returns:
/// ```json
/// {"success": true, "content": "<base64-encoded bytes>"}
/// ```
pub fn download(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let logos_storage_url = match get_str(&v, "logos_storage_url") {
        Ok(u) => u.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let cid = match get_str(&v, "cid") {
        Ok(c) => c.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return json!({"success": false, "error": format!("runtime error: {}", e)}).to_string(),
    };

    rt.block_on(async move { download_async(&logos_storage_url, &cid).await })
}

pub async fn download_async(logos_storage_url: &str, cid: &str) -> String {
    let url = format!(
        "{}/api/storage/v1/data/{}/network/stream",
        logos_storage_url.trim_end_matches('/'),
        cid
    );

    let client = reqwest::Client::new();
    let resp = match client.get(&url).send().await {
        Ok(r) => r,
        Err(e) => return json!({"success": false, "error": format!("HTTP request failed: {}", e)}).to_string(),
    };

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return json!({"success": false, "error": format!("download failed (HTTP {}): {}", status, body)}).to_string();
    }

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return json!({"success": false, "error": format!("failed to read response body: {}", e)}).to_string()
        }
    };

    let encoded = BASE64.encode(&bytes);
    json!({"success": true, "content": encoded}).to_string()
}

/// Fetch and parse an IDL JSON from Logos Storage by CID.
///
/// Args JSON:
/// ```json
/// {
///   "logos_storage_url": "http://localhost:8080",
///   "cid": "bafy..."
/// }
/// ```
///
/// Returns:
/// ```json
/// {"success": true, "idl": { ...parsed IDL object... }}
/// ```
pub fn fetch_idl(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let logos_storage_url = match get_str(&v, "logos_storage_url") {
        Ok(u) => u.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let cid = match get_str(&v, "cid") {
        Ok(c) => c.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return json!({"success": false, "error": format!("runtime error: {}", e)}).to_string(),
    };

    rt.block_on(async move { fetch_idl_async(&logos_storage_url, &cid).await })
}

pub async fn fetch_idl_async(logos_storage_url: &str, cid: &str) -> String {
    // Download the raw bytes
    let download_result = download_async(logos_storage_url, cid).await;
    let dl: Value = match serde_json::from_str(&download_result) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": format!("internal error: {}", e)}).to_string(),
    };

    if dl["success"] != true {
        return download_result;
    }

    // Decode base64 content
    let encoded = match dl["content"].as_str() {
        Some(s) => s,
        None => return json!({"success": false, "error": "no content in download response"}).to_string(),
    };

    let bytes = match BASE64.decode(encoded) {
        Ok(b) => b,
        Err(e) => return json!({"success": false, "error": format!("base64 decode failed: {}", e)}).to_string(),
    };

    // Parse as JSON IDL
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(idl) => json!({"success": true, "idl": idl}).to_string(),
        Err(e) => json!({"success": false, "error": format!("IDL is not valid JSON: {}", e)}).to_string(),
    }
}
