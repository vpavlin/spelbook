//! Registry operation implementations for the FFI layer.
//!
//! Each function takes a JSON args string and returns a JSON result string.
//! Transaction building follows the same pattern as lez-multisig/cli.
//!
//! Common JSON input fields:
//! - `sequencer_url`: e.g. "http://127.0.0.1:3040"
//! - `wallet_path`:   path to the LEZ wallet directory (sets NSSA_WALLET_HOME_DIR)
//! - `program_id_hex`: 64-char hex string identifying the registry program binary

use nssa::{
    public_transaction::{Message, WitnessSet},
    AccountId, PublicTransaction,
};
use registry_core::{
    compute_program_entry_pda, compute_registry_state_pda, Instruction, ProgramEntry, RegistryState,
};
use serde_json::{json, Value};
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

/// Parse a 64-hex-char program_id string into [u32; 8] (big-endian words).
fn parse_program_id_hex(s: &str) -> Result<nssa::ProgramId, String> {
    let s = s.trim_start_matches("0x");
    if s.len() != 64 {
        return Err(format!("program_id_hex must be 64 hex chars (got {})", s.len()));
    }
    let bytes = hex::decode(s).map_err(|e| format!("invalid hex in program_id: {}", e))?;
    let mut pid = [0u32; 8];
    for (i, chunk) in bytes.chunks(4).enumerate() {
        pid[i] = u32::from_be_bytes(chunk.try_into().unwrap());
    }
    Ok(pid)
}

/// Submit a transaction and wait for confirmation.
async fn submit_and_wait(
    client: &common::sequencer_client::SequencerClient,
    tx: PublicTransaction,
) -> Result<String, String> {
    let response = client
        .send_tx_public(tx)
        .await
        .map_err(|e| format!("failed to submit transaction: {}", e))?;

    Ok(response.tx_hash.to_string())
}

/// Build + submit a signed transaction for a registry instruction.
async fn submit_signed_registry_tx(
    wallet_core: &WalletCore,
    registry_program_id: nssa::ProgramId,
    account_ids: Vec<AccountId>,
    signer_id: AccountId,
    instruction: Instruction,
) -> Result<String, String> {
    let nonces = wallet_core
        .get_accounts_nonces(vec![signer_id])
        .await
        .map_err(|e| format!("failed to get nonces: {}", e))?;

    let signing_key = wallet_core
        .storage()
        .user_data
        .get_pub_account_signing_key(signer_id)
        .ok_or_else(|| {
            format!(
                "signing key not found for account {} — is it in your wallet?",
                signer_id
            )
        })?;

    let message = Message::try_new(registry_program_id, account_ids, nonces, instruction)
        .map_err(|e| format!("failed to build message: {:?}", e))?;

    let witness_set = WitnessSet::for_message(&message, &[signing_key]);
    let tx = PublicTransaction::new(message, witness_set);

    submit_and_wait(&wallet_core.sequencer_client, tx).await
}

/// Fetch and deserialize a Borsh-encoded account.
async fn fetch_borsh_account<T: borsh::BorshDeserialize>(
    wallet_core: &WalletCore,
    account_id: AccountId,
) -> Result<Option<T>, String> {
    let account = wallet_core
        .get_account_public(account_id)
        .await
        .map_err(|e| format!("failed to fetch account {}: {}", account_id, e))?;
    let data: Vec<u8> = account.data.into();
    if data.is_empty() {
        return Ok(None);
    }
    let decoded = borsh::from_slice::<T>(&data).map_err(|e| format!("failed to deserialize account data: {}", e))?;
    Ok(Some(decoded))
}

/// Load WalletCore with optional wallet_path override.
fn load_wallet(wallet_path: Option<&str>) -> Result<WalletCore, String> {
    if let Some(path) = wallet_path {
        std::env::set_var("NSSA_WALLET_HOME_DIR", path);
    }
    WalletCore::from_env().map_err(|e| format!("failed to load wallet: {}", e))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new program in the LEZ registry.
///
/// Args JSON:
/// ```json
/// {
///   "sequencer_url":    "http://127.0.0.1:3040",
///   "wallet_path":      "/path/to/wallet",
///   "registry_program_id": "...(64 hex chars)...",
///   "account":          "<author AccountId base58>",
///   "program_id":       "...(64 hex chars)...",
///   "name":             "lez-multisig",
///   "version":          "0.1.0",
///   "idl_cid":          "bafy...",
///   "description":      "M-of-N multisig governance",
///   "tags":             ["governance", "multisig"]
/// }
/// ```
pub fn register(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return json!({"success": false, "error": format!("runtime error: {}", e)}).to_string(),
    };

    rt.block_on(async { register_async(&v).await })
}

async fn register_async(v: &Value) -> String {
    let sequencer_url = match get_str(v, "sequencer_url") {
        Ok(s) => s.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let wallet_path = v["wallet_path"].as_str();
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
    let author_id: AccountId = match account_str.parse() {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": format!("invalid account id: {:?}", e)}).to_string(),
    };

    // Override sequencer URL in env for wallet
    std::env::set_var("NSSA_SEQUENCER_URL", &sequencer_url);

    let wallet_core = match load_wallet(wallet_path) {
        Ok(w) => w,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

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
        &wallet_core,
        registry_program_id,
        vec![registry_state_id, author_id, entry_pda_id],
        author_id,
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
///   "account":             "<author AccountId base58>",
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

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return json!({"success": false, "error": format!("runtime error: {}", e)}).to_string(),
    };

    rt.block_on(async { update_async(&v).await })
}

async fn update_async(v: &Value) -> String {
    let sequencer_url = match get_str(v, "sequencer_url") {
        Ok(s) => s.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let wallet_path = v["wallet_path"].as_str();
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
    let author_id: AccountId = match account_str.parse() {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": format!("invalid account id: {:?}", e)}).to_string(),
    };

    std::env::set_var("NSSA_SEQUENCER_URL", &sequencer_url);

    let wallet_core = match load_wallet(wallet_path) {
        Ok(w) => w,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

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
        &wallet_core,
        registry_program_id,
        vec![registry_state_id, author_id, entry_pda_id],
        author_id,
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
///
/// Note: Full enumeration requires an off-chain indexer in v1.
/// The state PDA only stores the count.
pub fn list(args: &str) -> String {
    let v = match parse_args(args) {
        Ok(v) => v,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return json!({"success": false, "error": format!("runtime error: {}", e)}).to_string(),
    };

    rt.block_on(async { list_async(&v).await })
}

async fn list_async(v: &Value) -> String {
    let sequencer_url = match get_str(v, "sequencer_url") {
        Ok(s) => s.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let wallet_path = v["wallet_path"].as_str();
    let registry_prog_id_hex = match get_str(v, "registry_program_id") {
        Ok(s) => s,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let registry_program_id = match parse_program_id_hex(registry_prog_id_hex) {
        Ok(id) => id,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    std::env::set_var("NSSA_SEQUENCER_URL", &sequencer_url);

    let wallet_core = match load_wallet(wallet_path) {
        Ok(w) => w,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let registry_state_id = compute_registry_state_pda(&registry_program_id);

    match fetch_borsh_account::<RegistryState>(&wallet_core, registry_state_id).await {
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
/// This function searches known PDAs — for a full name-based scan, an
/// off-chain indexer is needed. Currently returns an informative message.
///
/// Args JSON:
/// ```json
/// {
///   "sequencer_url":       "http://127.0.0.1:3040",
///   "wallet_path":         "/path/to/wallet",
///   "registry_program_id": "...(64 hex chars)...",
///   "name":                "lez-multisig"
/// }
/// ```
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
    // Return a clear message explaining this limitation.
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

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => return json!({"success": false, "error": format!("runtime error: {}", e)}).to_string(),
    };

    rt.block_on(async { get_by_id_async(&v).await })
}

async fn get_by_id_async(v: &Value) -> String {
    let sequencer_url = match get_str(v, "sequencer_url") {
        Ok(s) => s.to_string(),
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };
    let wallet_path = v["wallet_path"].as_str();
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

    std::env::set_var("NSSA_SEQUENCER_URL", &sequencer_url);

    let wallet_core = match load_wallet(wallet_path) {
        Ok(w) => w,
        Err(e) => return json!({"success": false, "error": e}).to_string(),
    };

    let entry_pda_id = compute_program_entry_pda(&registry_program_id, &program_id);

    match fetch_borsh_account::<ProgramEntry>(&wallet_core, entry_pda_id).await {
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
