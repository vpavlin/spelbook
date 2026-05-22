//! Local JSON program cache for spelbook.
//!
//! Stores program entries at `~/.local/share/spelbook/programs.json`
//! (or the path given by `SPELBOOK_CACHE_PATH`).
//!
//! The cache file is a JSON array of program-entry objects:
//! ```json
//! [{"program_id":"<64hex>","name":"...","version":"...","author":"...",
//!   "idl_cid":"...","description":"...","tags":[...],"registered_at":0}]
//! ```
//!
//! All functions are infallible from the caller's perspective — errors are
//! logged to stderr and treated as "empty cache" / no-op.

use serde_json::Value;
use std::path::PathBuf;

// ── Path ──────────────────────────────────────────────────────────────────────

/// Return the cache file path.
///
/// Prefers `$SPELBOOK_CACHE_PATH`; falls back to
/// `$HOME/.local/share/spelbook/programs.json`.
pub fn cache_path() -> PathBuf {
    if let Ok(p) = std::env::var("SPELBOOK_CACHE_PATH") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("spelbook")
        .join("programs.json")
}

// ── Load / Save ───────────────────────────────────────────────────────────────

/// Read the cache file and return all entries.
///
/// Returns an empty `Vec` on any I/O or parse error (missing file is normal).
pub fn load_cache() -> Vec<Value> {
    let path = cache_path();
    let data = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("[spelbook cache] cannot read {:?}: {}", path, e);
            }
            return Vec::new();
        }
    };
    match serde_json::from_str::<Value>(&data) {
        Ok(Value::Array(arr)) => arr,
        Ok(_) => {
            eprintln!("[spelbook cache] {:?} is not a JSON array — ignoring", path);
            Vec::new()
        }
        Err(e) => {
            eprintln!("[spelbook cache] JSON parse error in {:?}: {}", path, e);
            Vec::new()
        }
    }
}

/// Serialize `entries` and write them to the cache file.
///
/// Creates parent directories if they do not exist.
/// Returns `Err(String)` with a human-readable message on failure.
pub fn save_cache(entries: &[Value]) -> Result<(), String> {
    let path = cache_path();

    // Ensure the parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create cache directory {:?}: {}", parent, e))?;
    }

    let json = serde_json::to_string_pretty(entries)
        .map_err(|e| format!("JSON serialization error: {}", e))?;

    std::fs::write(&path, json).map_err(|e| format!("cannot write cache {:?}: {}", path, e))?;

    Ok(())
}

// ── Upsert ────────────────────────────────────────────────────────────────────

/// Add or replace an entry in the cache, keyed by `program_id`.
///
/// If an entry with the same `program_id` already exists it is replaced;
/// otherwise the new entry is appended.  Errors during save are logged to
/// stderr and silently ignored so that the main operation is not disrupted.
pub fn upsert_entry(entry: Value) {
    let program_id = match entry["program_id"].as_str() {
        Some(id) => id.to_string(),
        None => {
            eprintln!("[spelbook cache] upsert_entry: entry has no program_id field");
            return;
        }
    };

    let mut entries = load_cache();
    let pos = entries
        .iter()
        .position(|e| e["program_id"].as_str() == Some(&program_id));

    match pos {
        Some(i) => entries[i] = entry,
        None => entries.push(entry),
    }

    if let Err(e) = save_cache(&entries) {
        eprintln!("[spelbook cache] failed to save cache: {}", e);
    }
}

// ── Search ────────────────────────────────────────────────────────────────────

/// Search cached entries by case-insensitive substring match.
///
/// Matches against `name`, `description`, and each element of `tags`.
/// An empty `query` returns **all** entries.
pub fn search_cache(query: &str) -> Vec<Value> {
    let entries = load_cache();
    if query.is_empty() {
        return entries;
    }
    let q = query.to_lowercase();
    entries
        .into_iter()
        .filter(|e| {
            // Match against name
            let name_match = e["name"]
                .as_str()
                .map(|s| s.to_lowercase().contains(&q))
                .unwrap_or(false);

            // Match against description
            let desc_match = e["description"]
                .as_str()
                .map(|s| s.to_lowercase().contains(&q))
                .unwrap_or(false);

            // Match against any tag
            let tag_match = e["tags"]
                .as_array()
                .map(|tags| {
                    tags.iter()
                        .any(|t| t.as_str().map(|s| s.to_lowercase().contains(&q)).unwrap_or(false))
                })
                .unwrap_or(false);

            name_match || desc_match || tag_match
        })
        .collect()
}
