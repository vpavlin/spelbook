//! spelbook-ffi — C FFI wrapper for the SPELbook Program Registry
//!
//! Enables Logos Core Qt plugins (C++) to interact with the LEZ registry
//! and Logos Storage (Codex) without depending on Rust directly.
//!
//! Pattern: JSON string in → JSON string out (matches logos-blockchain-c style)
//! All returned strings must be freed with `lez_registry_free_string()`.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

mod cache;
mod registry;
mod storage;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Convert a C string pointer to a Rust &str, returning an error JSON on failure.
fn cstr_to_str<'a>(ptr: *const c_char) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err("null pointer".to_string());
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|e| format!("invalid UTF-8: {}", e))
}

/// Convert a Rust String to a C string (heap-allocated, caller must free).
fn to_cstring(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => CString::new(r#"{"success":false,"error":"internal: string contains null byte"}"#)
            .unwrap()
            .into_raw(),
    }
}

/// Return a JSON error string.
fn error_json(msg: &str) -> *mut c_char {
    to_cstring(format!(r#"{{"success":false,"error":{}}}"#, serde_json::json!(msg)))
}

// ── Registry Operations ───────────────────────────────────────────────────────

/// Register a program in the LEZ registry.
/// See spelbook.h for args_json schema.
#[no_mangle]
pub extern "C" fn lez_registry_register(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };
    let result = registry::register(args);
    to_cstring(result)
}

/// Update metadata for an existing registry entry.
#[no_mangle]
pub extern "C" fn lez_registry_update(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };
    let result = registry::update(args);
    to_cstring(result)
}

/// List all registered programs.
#[no_mangle]
pub extern "C" fn lez_registry_list(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };
    let result = registry::list(args);
    to_cstring(result)
}

/// Get a single program entry by name.
#[no_mangle]
pub extern "C" fn lez_registry_get_by_name(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };
    let result = registry::get_by_name(args);
    to_cstring(result)
}

/// Get a single program entry by program_id.
#[no_mangle]
pub extern "C" fn lez_registry_get_by_id(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };
    let result = registry::get_by_id(args);
    to_cstring(result)
}

// ── Cache / Search Operations ─────────────────────────────────────────────────

/// Search cached program entries by name, description, or tags.
///
/// args_json: `{"query": "token"}` — `query` is required; pass `""` to return all.
///
/// Returns:
/// ```json
/// {"success": true, "programs": [...], "count": N}
/// ```
/// or
/// ```json
/// {"success": false, "error": "..."}
/// ```
#[no_mangle]
pub extern "C" fn lez_registry_search(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };

    let v: serde_json::Value = match serde_json::from_str(args) {
        Ok(v) => v,
        Err(e) => return error_json(&format!("invalid JSON: {}", e)),
    };

    let query = v["query"].as_str().unwrap_or("");
    let programs = cache::search_cache(query);
    let count = programs.len();

    to_cstring(
        serde_json::json!({
            "success": true,
            "programs": programs,
            "count": count,
        })
        .to_string(),
    )
}

// ── Logos Storage Operations ──────────────────────────────────────────────────

/// Upload a file to Logos Storage, returns CID.
#[no_mangle]
pub extern "C" fn lez_storage_upload(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };
    let result = storage::upload(args);
    to_cstring(result)
}

/// Download content from Logos Storage by CID.
#[no_mangle]
pub extern "C" fn lez_storage_download(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };
    let result = storage::download(args);
    to_cstring(result)
}

/// Fetch and parse IDL JSON from Logos Storage.
#[no_mangle]
pub extern "C" fn lez_storage_fetch_idl(args_json: *const c_char) -> *mut c_char {
    let args = match cstr_to_str(args_json) {
        Ok(s) => s,
        Err(e) => return error_json(&e),
    };
    let result = storage::fetch_idl(args);
    to_cstring(result)
}

// ── Memory Management ─────────────────────────────────────────────────────────

/// Free a string returned by any lez_registry_* or lez_storage_* function.
#[no_mangle]
pub extern "C" fn lez_registry_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)) };
    }
}

// ── Version ───────────────────────────────────────────────────────────────────

/// Returns the version string of this FFI library.
#[no_mangle]
pub extern "C" fn lez_registry_version() -> *mut c_char {
    to_cstring(env!("CARGO_PKG_VERSION").to_string())
}
