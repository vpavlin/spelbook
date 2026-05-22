use anyhow::{Context, Result};
/// LEZ Program Registry CLI
///
/// On-chain registry for LEZ programs + Logos Storage (Codex) IDL management.
///
/// Usage:
///   registry register --account <id> --registry-program <hex> --program-id <hex> \
///                     --name <n> --version <v> --idl-cid <cid> [--sequencer-url <url>]
///   registry update   --account <id> --registry-program <hex> --program-id <hex> [--version ..] [--idl-cid ..] ...
///   registry list     --registry-program <hex> [--sequencer-url <url>]
///   registry info     --registry-program <hex> --program-id <hex> [--sequencer-url <url>]
///   registry upload-idl --path <file> [--storage-url <url>]
///   registry fetch-idl  --cid <cid> [--storage-url <url>]
///   registry status
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use nssa::{
    AccountId, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use common::transaction::NSSATransaction;
use sequencer_service_rpc::RpcClient as _;
use registry_core::{Instruction, ProgramEntry, RegistryState, compute_program_entry_pda, compute_registry_state_pda};
use wallet::WalletCore;

const DEFAULT_SEQUENCER_URL: &str = "http://127.0.0.1:3040";
const DEFAULT_STORAGE_URL: &str = "http://127.0.0.1:8080";

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "registry", version, about = "LEZ Program Registry CLI", long_about = None)]
// propagate_version removed to avoid conflict with --version arg
struct Cli {
    /// Sequencer URL
    #[arg(long, env = "NSSA_SEQUENCER_URL", default_value = DEFAULT_SEQUENCER_URL, global = true)]
    sequencer_url: String,

    /// Logos Storage (Codex) node URL
    #[arg(long, env = "LOGOS_STORAGE_URL", default_value = DEFAULT_STORAGE_URL, global = true)]
    storage_url: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Register a new program in the on-chain registry
    Register {
        /// Your account ID (base58) — becomes the author
        #[arg(long, short = 'a')]
        account: String,

        /// Registry program binary ID (hex-encoded [u32; 8], 64 hex chars)
        #[arg(long, env = "REGISTRY_PROGRAM_ID")]
        registry_program: String,

        /// Program ID to register (hex-encoded [u32; 8], 64 hex chars)
        #[arg(long)]
        program_id: String,

        /// Human-readable name, e.g. "lez-multisig"
        #[arg(long, short = 'n')]
        name: String,

        /// Version string, e.g. "0.1.0"
        #[arg(long, short = 'v')]
        version: String,

        /// Path to IDL JSON file — will be uploaded to Logos Storage automatically
        #[arg(long, conflicts_with = "idl_cid")]
        idl_path: Option<String>,

        /// Logos Storage CID of the IDL JSON (if already uploaded)
        #[arg(long, short = 'c', conflicts_with = "idl_path")]
        idl_cid: Option<String>,

        /// Free-form description
        #[arg(long, short = 'd', default_value = "")]
        description: String,

        /// Tags (repeat for multiple: --tag governance --tag multisig)
        #[arg(long, short = 't', num_args = 0..)]
        tag: Vec<String>,
    },

    /// Update metadata for an existing registered program
    Update {
        /// Your account ID (base58) — must be the original author
        #[arg(long, short = 'a')]
        account: String,

        /// Registry program binary ID (hex-encoded [u32; 8], 64 hex chars)
        #[arg(long, env = "REGISTRY_PROGRAM_ID")]
        registry_program: String,

        /// Program ID (hex-encoded [u32; 8], 64 hex chars)
        #[arg(long)]
        program_id: String,

        /// New version string
        #[arg(long, short = 'v', default_value = "")]
        version: String,

        /// New Codex CID (if already uploaded)
        #[arg(long, short = 'c', default_value = "", conflicts_with = "idl_path")]
        idl_cid: String,

        /// Path to new IDL JSON — will be uploaded automatically
        #[arg(long, conflicts_with = "idl_cid")]
        idl_path: Option<String>,

        /// New description
        #[arg(long, short = 'd', default_value = "")]
        description: String,

        /// New tags list (replaces existing)
        #[arg(long, short = 't', num_args = 0..)]
        tag: Vec<String>,
    },

    /// List all registered programs (shows count from registry state)
    List {
        /// Registry program binary ID (hex-encoded [u32; 8], 64 hex chars)
        #[arg(long, env = "REGISTRY_PROGRAM_ID")]
        registry_program: String,
    },

    /// Show details for one registered program by program_id hex
    Info {
        /// Registry program binary ID (hex-encoded [u32; 8], 64 hex chars)
        #[arg(long, env = "REGISTRY_PROGRAM_ID")]
        registry_program: String,

        /// Program ID hex (64 chars) to look up
        #[arg(long)]
        program_id: String,
    },

    /// Upload an IDL JSON file to Logos Storage — returns CID
    UploadIdl {
        /// Path to the IDL JSON file
        #[arg(long, short = 'p')]
        path: String,
    },

    /// Fetch and display an IDL JSON from Logos Storage by CID
    FetchIdl {
        /// Logos Storage CID
        #[arg(long)]
        cid: String,
    },

    /// Show registry status
    Status {
        /// Registry program binary ID (hex-encoded [u32; 8], 64 hex chars)
        #[arg(long, env = "REGISTRY_PROGRAM_ID", default_value = "")]
        registry_program: String,
    },

    /// Generate shell completions
    Completions {
        /// Shell to generate for
        #[arg(value_enum)]
        shell: Shell,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a hex-encoded program ID string ("xxxxxxxx..." 64 hex chars → [u32; 8]).
fn parse_program_id(s: &str) -> Result<nssa::ProgramId> {
    let s = s.trim_start_matches("0x");
    if s.len() != 64 {
        anyhow::bail!(
            "program_id must be 64 hex characters (8 × u32 big-endian), got {} chars",
            s.len()
        );
    }
    let bytes = hex::decode(s).context("invalid hex in program_id")?;
    let mut pid = [0u32; 8];
    for (i, chunk) in bytes.chunks(4).enumerate() {
        pid[i] = u32::from_be_bytes(chunk.try_into().unwrap());
    }
    Ok(pid)
}

/// Format a program ID as 64-char lowercase hex.
fn format_program_id(pid: &nssa::ProgramId) -> String {
    pid.iter()
        .flat_map(|w| w.to_be_bytes())
        .map(|b| format!("{:02x}", b))
        .collect()
}

async fn submit_and_confirm(wallet_core: &WalletCore, tx: PublicTransaction, label: &str) -> Result<String> {
    let response = wallet_core
        .sequencer_client
        .send_transaction(NSSATransaction::Public(tx))
        .await
        .context("failed to submit transaction")?;

    let tx_hash = response;

    println!("📤 {} submitted", label);
    println!("   tx_hash: {}", tx_hash);
    println!("   Waiting for confirmation...");

    let poller = wallet::poller::TxPoller::new(&wallet_core.config().clone(), wallet_core.sequencer_client.clone());

    match poller.poll_tx(tx_hash.clone()).await {
        Ok(_) => {
            println!("✅ Confirmed!");
            Ok(tx_hash.to_string())
        }
        Err(e) => {
            eprintln!("❌ Not confirmed: {e:#}");
            std::process::exit(1);
        }
    }
}

async fn submit_signed_tx(
    wallet_core: &WalletCore,
    registry_program_id: nssa::ProgramId,
    account_ids: Vec<AccountId>,
    signer_id: AccountId,
    instruction: Instruction,
    label: &str,
) -> Result<String> {
    let nonces = wallet_core
        .get_accounts_nonces(vec![signer_id])
        .await
        .context("failed to get nonces")?;

    let signing_key = wallet_core
        .storage()
        .user_data
        .get_pub_account_signing_key(signer_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "signing key not found for account {} — is it in your wallet?",
                signer_id
            )
        })?;

    let message = Message::try_new(registry_program_id, account_ids, nonces, instruction)
        .map_err(|e| anyhow::anyhow!("failed to build message: {:?}", e))?;

    let witness_set = WitnessSet::for_message(&message, &[signing_key]);
    let tx = PublicTransaction::new(message, witness_set);
    submit_and_confirm(wallet_core, tx, label).await
}

async fn fetch_account_data<T: borsh::BorshDeserialize>(
    wallet_core: &WalletCore,
    account_id: AccountId,
    label: &str,
) -> Result<Option<T>> {
    let account = wallet_core
        .get_account_public(account_id)
        .await
        .with_context(|| format!("failed to fetch {}", label))?;
    let data: Vec<u8> = account.data.into();
    if data.is_empty() {
        return Ok(None);
    }
    let decoded = borsh::from_slice::<T>(&data).with_context(|| format!("failed to deserialize {}", label))?;
    Ok(Some(decoded))
}

fn print_entry(entry: &ProgramEntry) {
    println!("  Name:          {}", entry.name);
    println!("  Version:       {}", entry.version);
    println!("  Author:        {}", entry.author);
    println!("  Program ID:    {}", format_program_id(&entry.program_id));
    println!("  IDL CID:       {}", entry.idl_cid);
    println!("  Description:   {}", entry.description);
    println!("  Registered at: {}", entry.registered_at);
    if !entry.tags.is_empty() {
        println!("  Tags:          {}", entry.tags.join(", "));
    }
}

/// Upload a file to Logos Storage and return the CID.
async fn upload_to_storage(file_path: &str, storage_url: &str) -> Result<String> {
    let file_bytes = tokio::fs::read(file_path)
        .await
        .with_context(|| format!("cannot read file '{}'", file_path))?;

    let filename = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload")
        .to_string();

    let url = format!("{}/api/storage/v1/data", storage_url.trim_end_matches('/'));
    let client = reqwest::Client::new();

    let resp = client
        .post(&url)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Disposition", format!("attachment; filename=\"{}\"", filename))
        .body(file_bytes)
        .send()
        .await
        .context("HTTP request to Logos Storage failed")?;

    let status = resp.status();
    let body = resp.text().await.context("failed to read storage response")?;

    if !status.is_success() {
        anyhow::bail!("Logos Storage upload failed (HTTP {}): {}", status, body);
    }

    // Parse CID from response
    if let Ok(resp_json) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(cid) = resp_json["cid"].as_str() {
            return Ok(cid.to_string());
        }
    }
    // Plain text CID
    Ok(body.trim().trim_matches('"').to_string())
}

/// Download content from Logos Storage by CID.
async fn download_from_storage(cid: &str, storage_url: &str) -> Result<Vec<u8>> {
    let url = format!(
        "{}/api/storage/v1/data/{}/network/stream",
        storage_url.trim_end_matches('/'),
        cid
    );
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .send()
        .await
        .context("HTTP request to Logos Storage failed")?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Logos Storage download failed (HTTP {}): {}", status, body);
    }

    let bytes = resp.bytes().await.context("failed to read storage response body")?;
    Ok(bytes.to_vec())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Commands that don't need wallet
    match &cli.command {
        Commands::Completions { shell } => {
            generate(*shell, &mut Cli::command(), "registry", &mut std::io::stdout());
            return;
        }
        Commands::Status { registry_program } => {
            println!("📋 LEZ Program Registry");
            println!("   Sequencer URL:    {}", cli.sequencer_url);
            println!("   Storage URL:      {}", cli.storage_url);
            if !registry_program.is_empty() {
                match parse_program_id(registry_program) {
                    Ok(id) => {
                        let state_pda = compute_registry_state_pda(&id);
                        println!("   Registry Prog ID: {}", format_program_id(&id));
                        println!("   Registry State:   {}", state_pda);
                    }
                    Err(e) => eprintln!("   Invalid registry_program: {}", e),
                }
            }
            return;
        }
        Commands::UploadIdl { path } => {
            println!("📤 Uploading IDL to Logos Storage...");
            println!("   File:        {}", path);
            println!("   Storage URL: {}", cli.storage_url);
            match upload_to_storage(path, &cli.storage_url).await {
                Ok(cid) => {
                    println!("\n✅ IDL uploaded successfully!");
                    println!("   CID: {}", cid);
                }
                Err(e) => {
                    eprintln!("❌ Upload failed: {:#}", e);
                    std::process::exit(1);
                }
            }
            return;
        }
        Commands::FetchIdl { cid } => {
            println!("📥 Fetching IDL from Logos Storage...");
            println!("   CID:         {}", cid);
            println!("   Storage URL: {}", cli.storage_url);
            match download_from_storage(cid, &cli.storage_url).await {
                Ok(bytes) => match serde_json::from_slice::<serde_json::Value>(&bytes) {
                    Ok(idl) => {
                        println!("\n✅ IDL fetched successfully!");
                        println!("{}", serde_json::to_string_pretty(&idl).unwrap_or_default());
                    }
                    Err(e) => {
                        eprintln!("❌ IDL is not valid JSON: {}", e);
                        std::process::exit(1);
                    }
                },
                Err(e) => {
                    eprintln!("❌ Fetch failed: {:#}", e);
                    std::process::exit(1);
                }
            }
            return;
        }
        _ => {}
    }

    // Override sequencer URL before wallet init
    unsafe {
        std::env::set_var("NSSA_SEQUENCER_URL", &cli.sequencer_url);
    }
    unsafe {
        std::env::set_var("NSSA_STORAGE_URL", &cli.storage_url);
    }

    let wallet_core = WalletCore::from_env().unwrap_or_else(|e| {
        eprintln!("❌ Failed to load wallet: {}", e);
        std::process::exit(1);
    });

    match cli.command {
        // ── Register ────────────────────────────────────────────────────
        Commands::Register {
            account,
            registry_program,
            program_id,
            name,
            version,
            idl_path,
            idl_cid,
            description,
            tag,
        } => {
            let registry_program_id = parse_program_id(&registry_program).unwrap_or_else(|e| {
                eprintln!("❌ Invalid --registry-program: {}", e);
                std::process::exit(1);
            });
            let prog_id = parse_program_id(&program_id).unwrap_or_else(|e| {
                eprintln!("❌ Invalid --program-id: {}", e);
                std::process::exit(1);
            });
            let author_id: AccountId = account.parse().unwrap_or_else(|e| {
                eprintln!("❌ Invalid --account: {:?}", e);
                std::process::exit(1);
            });

            // Resolve IDL CID: upload from path if given, else use provided CID
            let resolved_cid = if let Some(path) = idl_path {
                println!("📤 Uploading IDL from '{}'...", path);
                upload_to_storage(&path, &cli.storage_url).await.unwrap_or_else(|e| {
                    eprintln!("❌ IDL upload failed: {:#}", e);
                    std::process::exit(1);
                })
            } else if let Some(cid) = idl_cid {
                cid
            } else {
                eprintln!("❌ Either --idl-path or --idl-cid must be provided");
                std::process::exit(1);
            };

            let registry_state_id = compute_registry_state_pda(&registry_program_id);
            let entry_pda_id = compute_program_entry_pda(&registry_program_id, &prog_id);

            println!("📝 Registering program '{}' v{}", name, version);
            println!("   Registry state: {}", registry_state_id);
            println!("   Entry PDA:      {}", entry_pda_id);
            println!("   IDL CID:        {}", resolved_cid);

            let instruction = Instruction::Register {
                program_id: prog_id,
                name: name.clone(),
                version: version.clone(),
                idl_cid: resolved_cid.clone(),
                description,
                tags: tag,
            };

            submit_signed_tx(
                &wallet_core,
                registry_program_id,
                vec![registry_state_id, author_id, entry_pda_id],
                author_id,
                instruction,
                &format!("Register '{}'", name),
            )
            .await
            .unwrap_or_else(|e| {
                eprintln!("❌ Register failed: {:#}", e);
                std::process::exit(1);
            });

            println!("\n✅ Program '{}' v{} registered successfully!", name, version);
            println!("   Entry PDA: {}", entry_pda_id);
            println!("   IDL CID:   {}", resolved_cid);
        }

        // ── Update ──────────────────────────────────────────────────────
        Commands::Update {
            account,
            registry_program,
            program_id,
            version,
            idl_cid,
            idl_path,
            description,
            tag,
        } => {
            let registry_program_id = parse_program_id(&registry_program).unwrap_or_else(|e| {
                eprintln!("❌ Invalid --registry-program: {}", e);
                std::process::exit(1);
            });
            let prog_id = parse_program_id(&program_id).unwrap_or_else(|e| {
                eprintln!("❌ Invalid --program-id: {}", e);
                std::process::exit(1);
            });
            let author_id: AccountId = account.parse().unwrap_or_else(|e| {
                eprintln!("❌ Invalid --account: {:?}", e);
                std::process::exit(1);
            });

            // Resolve new IDL CID if path provided
            let resolved_cid = if let Some(path) = idl_path {
                println!("📤 Uploading new IDL from '{}'...", path);
                upload_to_storage(&path, &cli.storage_url).await.unwrap_or_else(|e| {
                    eprintln!("❌ IDL upload failed: {:#}", e);
                    std::process::exit(1);
                })
            } else {
                idl_cid
            };

            let registry_state_id = compute_registry_state_pda(&registry_program_id);
            let entry_pda_id = compute_program_entry_pda(&registry_program_id, &prog_id);

            println!("🔄 Updating program entry...");
            println!("   Entry PDA: {}", entry_pda_id);

            let instruction = Instruction::Update {
                program_id: prog_id,
                version,
                idl_cid: resolved_cid,
                description,
                tags: tag,
            };

            submit_signed_tx(
                &wallet_core,
                registry_program_id,
                vec![registry_state_id, author_id, entry_pda_id],
                author_id,
                instruction,
                "Update program entry",
            )
            .await
            .unwrap_or_else(|e| {
                eprintln!("❌ Update failed: {:#}", e);
                std::process::exit(1);
            });

            println!("\n✅ Program entry updated successfully!");
        }

        // ── List ────────────────────────────────────────────────────────
        Commands::List { registry_program } => {
            let registry_program_id = parse_program_id(&registry_program).unwrap_or_else(|e| {
                eprintln!("❌ Invalid --registry-program: {}", e);
                std::process::exit(1);
            });

            let registry_state_id = compute_registry_state_pda(&registry_program_id);
            println!("📋 Fetching registry state ({})...", registry_state_id);

            let state: Option<RegistryState> = fetch_account_data(&wallet_core, registry_state_id, "registry state")
                .await
                .unwrap_or_else(|e| {
                    eprintln!("❌ {:#}", e);
                    std::process::exit(1);
                });

            match state {
                None => println!("Registry not yet initialized (no programs registered)."),
                Some(s) => {
                    println!("📦 Registry — {} program(s) registered", s.program_count);
                    println!("   Authority: {}", s.authority);
                    println!();
                    println!("ℹ️  To look up a specific program use: registry info --program-id <hex>");
                    println!("   Full enumeration requires off-chain indexing in v1.");
                }
            }
        }

        // ── Info ────────────────────────────────────────────────────────
        Commands::Info {
            registry_program,
            program_id,
        } => {
            let registry_program_id = parse_program_id(&registry_program).unwrap_or_else(|e| {
                eprintln!("❌ Invalid --registry-program: {}", e);
                std::process::exit(1);
            });
            let prog_id = parse_program_id(&program_id).unwrap_or_else(|e| {
                eprintln!("❌ Invalid --program-id: {}", e);
                std::process::exit(1);
            });

            let entry_pda_id = compute_program_entry_pda(&registry_program_id, &prog_id);
            println!("🔍 Looking up program entry ({})...", entry_pda_id);

            let entry: Option<ProgramEntry> = fetch_account_data(&wallet_core, entry_pda_id, "program entry")
                .await
                .unwrap_or_else(|e| {
                    eprintln!("❌ {:#}", e);
                    std::process::exit(1);
                });

            match entry {
                None => println!("No program entry found for program_id '{}'.", program_id),
                Some(e) => {
                    println!();
                    print_entry(&e);
                }
            }
        }

        // Already handled above (no wallet needed)
        Commands::Completions { .. }
        | Commands::Status { .. }
        | Commands::UploadIdl { .. }
        | Commands::FetchIdl { .. } => unreachable!(),
    }
}
