// registry_core — shared types and PDA derivation helpers for the Program Registry.
//
// The Program Registry is an on-chain directory for LEZ programs.  Developers
// register their deployed program IDs along with human-readable metadata and a
// Codex CID pointing to the program's IDL JSON stored on Logos Storage.
//
// Architecture mirrors multisig_core — all types are shared between the
// on-chain guest binary and the CLI.

use borsh::{BorshDeserialize, BorshSerialize};
use nssa_core::account::AccountId;
use nssa_core::program::{PdaSeed, ProgramId};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

/// Instructions accepted by the registry program.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Instruction {
    /// Register a new program entry.  Anyone can register; the signer becomes
    /// the author and only the author may later update the entry.
    Register {
        program_id: ProgramId,
        name: String,
        version: String,
        idl_cid: String,
        description: String,
        tags: Vec<String>,
    },

    /// Update metadata for an existing program entry.
    /// Only the original author (signer) is allowed.
    Update {
        program_id: ProgramId,
        version: String,
        idl_cid: String,
        description: String,
        tags: Vec<String>,
    },
}

/// Arguments for the `Register` instruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterArgs {
    /// Unique on-chain ID of the program being registered ([u32; 8]).
    pub program_id: ProgramId,
    /// Human-readable name, e.g. "lez-multisig".
    pub name: String,
    /// Semantic version string, e.g. "0.1.0".
    pub version: String,
    /// Codex CID of the IDL JSON stored on Logos Storage.
    pub idl_cid: String,
    /// Free-form description.
    pub description: String,
    /// Optional tags, e.g. ["governance", "multisig"].
    pub tags: Vec<String>,
}

/// Arguments for the `Update` instruction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateArgs {
    /// Program ID identifies which entry to update.
    pub program_id: ProgramId,
    /// New version string (optional — pass current to keep unchanged).
    pub version: String,
    /// New IDL CID (optional — pass current to keep unchanged).
    pub idl_cid: String,
    /// New description (optional — pass current to keep unchanged).
    pub description: String,
    /// New tags list (replaces existing).
    pub tags: Vec<String>,
}

// ---------------------------------------------------------------------------
// On-chain state types
// ---------------------------------------------------------------------------

/// Singleton PDA that tracks global registry statistics.
///
/// PDA derivation: seed = b"registry_state__" (16 bytes) XOR [0u8; 32]
/// (i.e. the tag bytes padded to 32 — no additional key needed since it is
/// truly a singleton per registry program deployment).
#[derive(Debug, Clone, Default, BorshSerialize, BorshDeserialize)]
pub struct RegistryState {
    /// The account that deployed / owns the registry (set at init time).
    /// Reserved for future permissioned operations; currently unused in v1.
    pub authority: AccountId,
    /// Counter incremented each time a program is successfully registered.
    pub program_count: u64,
}

impl RegistryState {
    pub fn new(authority: AccountId) -> Self {
        Self {
            authority,
            program_count: 0,
        }
    }

    /// Increment and return the new program count.
    pub fn increment(&mut self) -> u64 {
        self.program_count += 1;
        self.program_count
    }
}

/// Per-program entry stored in its own PDA.
///
/// PDA seed: XOR("program_entry___" padded to 32 bytes, program_id_as_bytes)
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct ProgramEntry {
    /// The on-chain program ID this entry describes.
    pub program_id: ProgramId,
    /// Human-readable name.
    pub name: String,
    /// Version string.
    pub version: String,
    /// The account that registered the program (only they can update).
    pub author: AccountId,
    /// Codex CID of the IDL JSON.
    pub idl_cid: String,
    /// Free-form description.
    pub description: String,
    /// Block timestamp at registration time (seconds since epoch, set by host).
    pub registered_at: u64,
    /// Searchable tags.
    pub tags: Vec<String>,
}

impl ProgramEntry {
    pub fn new(
        program_id: ProgramId,
        name: String,
        version: String,
        author: AccountId,
        idl_cid: String,
        description: String,
        registered_at: u64,
        tags: Vec<String>,
    ) -> Self {
        Self {
            program_id,
            name,
            version,
            author,
            idl_cid,
            description,
            registered_at,
            tags,
        }
    }
}

// ---------------------------------------------------------------------------
// PDA derivation helpers
// ---------------------------------------------------------------------------

/// Compute PDA seed for the singleton registry state account.
///
/// Uses the 16-byte ASCII tag "registry_state__" placed in the first 16 bytes
/// of a 32-byte seed (remaining bytes stay zero — no XOR key needed for the
/// singleton).
pub fn registry_state_pda_seed() -> PdaSeed {
    let tag = b"registry_state__"; // exactly 16 bytes
    let mut seed = [0u8; 32];
    seed[..tag.len()].copy_from_slice(tag);
    PdaSeed::new(seed)
}

/// Compute the on-chain AccountId (PDA) for the singleton registry state.
pub fn compute_registry_state_pda(program_id: &ProgramId) -> AccountId {
    AccountId::for_public_pda(program_id, &registry_state_pda_seed())
}

/// Convert a `ProgramId` ([u32; 8]) to a canonical 32-byte big-endian representation.
pub fn program_id_to_bytes(program_id: &ProgramId) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    for (i, word) in program_id.iter().enumerate() {
        let word_bytes = word.to_be_bytes();
        bytes[i * 4..(i + 1) * 4].copy_from_slice(&word_bytes);
    }
    bytes
}

/// Compute PDA seed for a program entry.
///
/// seed = XOR("program_entry___" padded to 32 bytes, program_id_bytes_first_32)
///
/// The tag "program_entry___" is 16 bytes; it occupies the first 16 bytes of
/// the 32-byte seed before XOR with the program ID bytes so that different
/// program IDs always produce different seeds.
pub fn program_entry_pda_seed(program_id: &ProgramId) -> PdaSeed {
    let tag = b"program_entry___"; // exactly 16 bytes
    let pid_bytes = program_id_to_bytes(program_id);

    let mut seed = [0u8; 32];
    // Place tag in first 16 bytes
    seed[..tag.len()].copy_from_slice(tag);
    // XOR with program ID bytes across all 32 bytes
    for i in 0..32 {
        seed[i] ^= pid_bytes[i];
    }
    PdaSeed::new(seed)
}

/// Compute the on-chain AccountId (PDA) for a program entry.
pub fn compute_program_entry_pda(registry_program_id: &ProgramId, program_id: &ProgramId) -> AccountId {
    AccountId::for_public_pda(registry_program_id, &program_entry_pda_seed(program_id))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_program_id(fill: u32) -> ProgramId {
        [fill; 8]
    }

    #[test]
    fn test_registry_state_pda_seed_is_deterministic() {
        let s1 = registry_state_pda_seed();
        let s2 = registry_state_pda_seed();
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_program_entry_pda_seed_differs_by_program_id() {
        let pid1 = dummy_program_id(1);
        let pid2 = dummy_program_id(2);
        let s1 = program_entry_pda_seed(&pid1);
        let s2 = program_entry_pda_seed(&pid2);
        assert_ne!(s1, s2);
    }

    #[test]
    fn test_program_id_to_bytes_roundtrip() {
        let pid: ProgramId = [0x01020304, 0x05060708, 0, 0, 0, 0, 0, 0];
        let bytes = program_id_to_bytes(&pid);
        assert_eq!(bytes[0], 0x01);
        assert_eq!(bytes[1], 0x02);
        assert_eq!(bytes[2], 0x03);
        assert_eq!(bytes[3], 0x04);
        assert_eq!(bytes[4], 0x05);
    }

    #[test]
    fn test_registry_state_increment() {
        let mut state = RegistryState::new(AccountId::new([0u8; 32]));
        assert_eq!(state.program_count, 0);
        assert_eq!(state.increment(), 1);
        assert_eq!(state.increment(), 2);
        assert_eq!(state.program_count, 2);
    }
}
