//! Registry operation implementations for the FFI layer.
//!
//! Each function takes a JSON args string and returns a JSON result string.
//! Transaction building follows the same pattern as the spel-client-gen generated code
//! and lez-multisig-ffi, using logos-execution-zone at v0.2.0-rc3.
//!
//! Common JSON input fields:
//! - `sequencer_url`: e.g. "http://127.0.0.1:3040"
//! - `wallet_path`:   path to the LEZ wallet directory (sets NSSA_WALLET_HOME_DIR)
//! - `registry_program_id`: 64-char hex string identifying the registry program binary

use nssa::{
    public_transaction::{Message, WitnessSet},
    AccountId, ProgramId, PublicTransaction,
};
use registry_core::{
    compute_program_entry_pda, compute_registry_state_pda, Instruction, ProgramEntry, RegistryState,
};
use serde_json::{json, Value};
use sequencer_service_rpc::RpcClient as _;
use wallet::WalletCore;

use crate::cache;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_args(args: &str) -> Result<Value, String> {
    serde_json::from_str(args).map_err(|e| format!("invalid JSON: {}", e))
}

fn get_str<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v[key].as_str().ok_or_else(|| format!("missing field '{}'", key))
}

/// Parse a 64-hex-char program_id string into [u32; 8] (little-endian words).
/// Matches the spel-client-gen convention used by lez-multisig and all spel-generated FFI clients.
fn parse_program_id_hex(s: &str) -> Result<ProgramId, String> {
    let s = s.trim_start_matches("0x");
    if s.len() != 64 {
        return Err(format!("program_id_hex must be 64 hex chars (got {})", s.len()));
    }
    let bytes = hex::decode(s).map_err(|e| format!("invalid hex in program_id: {}", e))?;
    let mut pid = [0u32; 8];
    for (i, chunk) in bytes.chunks(4).enumerate() {
        pid[i] = u32::from_le_bytes(chunk.try_into().unwrap());
    }
    Ok(pid)
}

/// Parse an AccountId from a string (base58, hex, or "Public/<id>" form).
fn parse_account_id(s: &str) -> Result<AccountId, String> {
    let raw = s;
    let s = s.strip_prefix("Public/").or_else(|| s.strip_prefix("Private/")).unwrap_or(s);
    if let Ok(id) = s.parse() {
        return Ok(id);
    }
    let s = s.trim_start_matches("0x");
    if s.len() == 64 {
        let bytes = hex::decode(s).map_err(|e| format!("invalid hex: {}", e))?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        return Ok(AccountId::new(arr));
    }
    Err(format!("invalid AccountId: {}", raw))
}

static ASYNC_RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
fn get_runtime() -> &'static tokio::runtime::Runtime {
    ASYNC_RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("failed to create Tokio runtime"))
}

static WALLET_INIT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Initialize WalletCore from JSON args (reads wallet_path and sequencer_url).
fn init_wallet(v: &Value) -> Result<WalletCore, String> {
    let _guard = WALLET_INIT_LOCK.lock().map_err(|_| "wallet lock poisoned".to_string())?;
    let wallet_path = v["wallet_path"].as_str().ok_or("missing required field: wallet_path")?;
    if wallet_path.is_empty() || wallet_path.contains('\0') {
        return Err("wallet_path must be a non-empty path without null bytes".into());
    }
    let sequencer_url = v["sequencer_url"].as_str().ok_or("missing required field: sequencer_url")?;
    std::env::set_var("NSSA_WALLET_HOME_DIR", wallet_path);
    std::env::set_var("NSSA_SEQUENCER_URL", sequencer_url);
    WalletCore::from_env().map_err(|e| format!("wallet init: {}", e))
}

/// Build + submit a signed transaction for a registry instruction using the new logos-execution-zone API.
async fn submit_signed_registry_tx(
    wallet: &WalletCore,
    registry_program_id: ProgramId,
    account_ids: Vec<AccountId>,
    signer_ids: Vec<AccountId>,
    instruction: Instruction,
) -> Result<String, String> {
    let nonces = wallet
        .get_accounts_nonces(signer_ids.clone())
        .await
        .map_err(|e| format!("nonces: {}", e))?;

    let mut signing_keys = Vec::new();
    for sid in &signer_ids {
        let key = wallet
            .storage()
            .user_data
            .get_pub_account_signing_key(*sid)
            .ok_or_else(|| format!("signing key not found for {} — is it in your wallet?", sid))?;
        signing_keys.push(key);
    }

    let message = Message::try_new(registry_program_id, account_ids, nonces, instruction)
        .map_err(|e| format!("message: {:?}", e))?;
    let witness_set = WitnessSet::for_message(&message, &signing_keys);
    let tx = PublicTransaction::new(message, witness_set);

    let raw = wallet
        .sequencer_client
        .send_transaction(common::transaction::NSSATransaction::Public(tx))
        .await
        .map_err(|e| format!("submit: {}", e))?;
    let tx_hash_hex = hex::encode(raw.0);
    let poller = wallet::poller::TxPoller::new(wallet.config(), wallet.sequencer_client.clone());
    poller.poll_tx(raw).await.map_err(|e| format!("confirm: {}", e))?;

    Ok(tx_hash_hex)
}

/// Fetch and deserialize a Borsh-encoded account.
async fn fetch_borsh_account<T: borsh::BorshDeserialize>(
    wallet: &WalletCore,
    account_id: AccountId,
) -> Result<Option<T>, String> {
    let account = wallet
        .sequencer_client
        .get_account(account_id)
        .await
        .map_err(|e| format!("failed to fetch account {}: {}", account_id, e))?;
    if account.data.is_empty() {
        return Ok(None);
    }
    let decoded = borsh::from_slice::<T>(&account.data)
        .map_err(|e| format!("failed to deserialize account data: {}", e))?;
    Ok(Some(decoded))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new program in the LEZ registry.
///
/// Args JSON:
/// ```json
/// {
///   "sequencer_url":       "http://127.0.0.1:3040",
///   "wallet_path":         "/path/to/wallet",
///   "registry_program_id": "...(64 hex chars)...",
///   "account":             "<author AccountId>",
///   "program_id":          "...(64 hex chars)...",
///   "name":                "lez-multisig",
///   "version":             "0.1.0",
///   "idl_cid":             "bafy...",
///   "description":         "M-of-N multisig governance",
///   "tags":                ["governance", "multisig"]
/// }
/// ```
pub fn register(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = get_runtime();
    rt.block_on(async { register_async(&v).await })
}

async fn register_async(v: &Value) -> String {
    let registry_prog_id_hex = match get_str(v, "registry_program_id") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let account_str = match get_str(v, "account") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let program_id_hex = match get_str(v, "program_id") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let name = match get_str(v, "name") {
        Ok(s) => s.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let version = match get_str(v, "version") {
        Ok(s) => s.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let idl_cid = match get_str(v, "idl_cid") {
        Ok(s) => s.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let description = v["description"].as_str().unwrap_or("").to_string();
    let tags: Vec<String> = v["tags"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|t| t.as_str().map(|s| s.to_string()))
        .collect();

    // Parse IDs
    let registry_program_id = match parse_program_id_hex(registry_prog_id_hex) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let program_id = match parse_program_id_hex(program_id_hex) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let author_id = match parse_account_id(account_str) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": format!("invalid account id: {}", e)}).to_string(),
    };

    let wallet = match init_wallet(v) {
        Ok(w) => w,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    // Compute PDAs using registry_core helpers
    let registry_state_id = compute_registry_state_pda(&registry_program_id);
    let entry_pda_id = compute_program_entry_pda(&registry_program_id, &program_id);

    let instruction = Instruction::Register {
        program_id,
        name: name.clone(),
        version: version.clone(),
        idl_cid: idl_cid.clone(),
        description,
        tags,
    };

    match submit_signed_registry_tx(
        &wallet,
        registry_program_id,
        vec![registry_state_id, author_id, entry_pda_id],
        vec![author_id],
        instruction,
    )
    .await
    {
        Ok(tx_hash) => {
            // Populate the local cache so the entry is immediately searchable.
            let entry_json = json!({
                "program_id": program_id_hex,
                "name": name,
                "version": version,
                "author": author_id.to_string(),
                "idl_cid": idl_cid,
                "description": v["description"].as_str().unwrap_or(""),
                "tags": v["tags"].as_array().cloned().unwrap_or_default(),
                "registered_at": 0,
            });
            cache::upsert_entry(entry_json);

            json!({
                "success": true,
                "tx_hash": tx_hash,
                "entry_pda": entry_pda_id.to_string(),
                "name": name,
                "version": version,
                "idl_cid": idl_cid,
            })
            .to_string()
        }
        Err(e) => json!({"success": false, "error": e}).to_string(),
    }
}

/// Update an existing program entry.
///
/// Args JSON:
/// ```json
/// {
///   "sequencer_url":       "http://127.0.0.1:3040",
///   "wallet_path":         "/path/to/wallet",
///   "registry_program_id": "...(64 hex chars)...",
///   "account":             "<author AccountId>",
///   "program_id":          "...(64 hex chars)...",
///   "version":             "0.2.0",
///   "idl_cid":             "bafy...",
///   "description":         "updated description",
///   "tags":                ["governance"]
/// }
/// ```
pub fn update(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = get_runtime();
    rt.block_on(async { update_async(&v).await })
}

async fn update_async(v: &Value) -> String {
    let registry_prog_id_hex = match get_str(v, "registry_program_id") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let account_str = match get_str(v, "account") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let program_id_hex = match get_str(v, "program_id") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let version = v["version"].as_str().unwrap_or("").to_string();
    let idl_cid = v["idl_cid"].as_str().unwrap_or("").to_string();
    let description = v["description"].as_str().unwrap_or("").to_string();
    let tags: Vec<String> = v["tags"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|t| t.as_str().map(|s| s.to_string()))
        .collect();

    let registry_program_id = match parse_program_id_hex(registry_prog_id_hex) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let program_id = match parse_program_id_hex(program_id_hex) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let author_id = match parse_account_id(account_str) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": format!("invalid account id: {}", e)}).to_string(),
    };

    let wallet = match init_wallet(v) {
        Ok(w) => w,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    // Compute PDAs using registry_core helpers
    let registry_state_id = compute_registry_state_pda(&registry_program_id);
    let entry_pda_id = compute_program_entry_pda(&registry_program_id, &program_id);

    let instruction = Instruction::Update {
        program_id,
        version,
        idl_cid,
        description,
        tags,
    };

    match submit_signed_registry_tx(
        &wallet,
        registry_program_id,
        vec![registry_state_id, author_id, entry_pda_id],
        vec![author_id],
        instruction,
    )
    .await
    {
        Ok(tx_hash) => json!({
            "success": true,
            "tx_hash": tx_hash,
            "entry_pda": entry_pda_id.to_string(),
        })
        .to_string(),
        Err(e) => json!({"success": false, "error": e}).to_string(),
    }
}

/// List all registered programs by querying the registry state.
///
/// Args JSON:
/// ```json
/// {
///   "sequencer_url":       "http://127.0.0.1:3040",
///   "wallet_path":         "/path/to/wallet",
///   "registry_program_id": "...(64 hex chars)..."
/// }
/// ```
///
/// Returns:
/// ```json
/// {"success": true, "program_count": 3, "note": "..."}
/// ```
pub fn list(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = get_runtime();
    rt.block_on(async { list_async(&v).await })
}

async fn list_async(v: &Value) -> String {
    let registry_prog_id_hex = match get_str(v, "registry_program_id") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let registry_program_id = match parse_program_id_hex(registry_prog_id_hex) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let wallet = match init_wallet(v) {
        Ok(w) => w,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let registry_state_id = compute_registry_state_pda(&registry_program_id);

    match fetch_borsh_account::<RegistryState>(&wallet, registry_state_id).await {
        Ok(None) => json!({
            "success": true,
            "program_count": 0,
            "registry_state_pda": registry_state_id.to_string(),
            "note": "Registry not yet initialized (no programs registered)"
        })
        .to_string(),
        Ok(Some(state)) => json!({
            "success": true,
            "program_count": state.program_count,
            "authority": state.authority.to_string(),
            "registry_state_pda": registry_state_id.to_string(),
            "note": "Full program list requires off-chain indexer in v1; use get_by_name/get_by_id for individual lookups"
        })
        .to_string(),
        Err(e) => json!({"success": false, "error": e}).to_string(),
    }
}

/// Get a single program entry by name.
///
/// Note: In v1, PDA derivation is by program_id (hash), not by name.
/// Returns an informative error directing callers to use get_by_id.
pub fn get_by_name(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let name = match get_str(&v, "name") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    // In v1, name-based lookup requires the program_id to derive the PDA.
    json!({
        "success": false,
        "error": format!(
            "name-based lookup ('{}') requires an off-chain indexer (v1 limitation). \
             Use get_by_id with the program_id_hex to derive the PDA directly.",
            name
        )
    })
    .to_string()
}

/// Get a single program entry by program_id (hex).
///
/// Args JSON:
/// ```json
/// {
///   "sequencer_url":       "http://127.0.0.1:3040",
///   "wallet_path":         "/path/to/wallet",
///   "registry_program_id": "...(64 hex chars)...",
///   "program_id":          "...(64 hex chars)..."
/// }
/// ```
pub fn get_by_id(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = get_runtime();
    rt.block_on(async { get_by_id_async(&v).await })
}

async fn get_by_id_async(v: &Value) -> String {
    let registry_prog_id_hex = match get_str(v, "registry_program_id") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let program_id_hex = match get_str(v, "program_id") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let registry_program_id = match parse_program_id_hex(registry_prog_id_hex) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let program_id = match parse_program_id_hex(program_id_hex) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let wallet = match init_wallet(v) {
        Ok(w) => w,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let entry_pda_id = compute_program_entry_pda(&registry_program_id, &program_id);

    match fetch_borsh_account::<ProgramEntry>(&wallet, entry_pda_id).await {
        Ok(None) => json!({
            "success": false,
            "error": "program entry not found",
            "entry_pda": entry_pda_id.to_string()
        })
        .to_string(),
        Ok(Some(entry)) => {
            // Format program_id as hex string
            let pid_hex: String = entry
                .program_id
                .iter()
                .flat_map(|w| w.to_be_bytes())
                .map(|b| format!("{:02x}", b))
                .collect();

            let entry_json = json!({
                "program_id": pid_hex,
                "name": entry.name,
                "version": entry.version,
                "author": entry.author.to_string(),
                "idl_cid": entry.idl_cid,
                "description": entry.description,
                "registered_at": entry.registered_at,
                "tags": entry.tags,
            });

            // Populate the local cache so the entry is available for search.
            cache::upsert_entry(entry_json.clone());

            json!({
                "success": true,
                "entry": entry_json,
                "entry_pda": entry_pda_id.to_string(),
            })
            .to_string()
        }
        Err(e) => json!({"success": false, "error": e}).to_string(),
    }
}
