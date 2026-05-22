// update.rs — handler for the Update instruction.
//
// Updates metadata on an existing ProgramEntry PDA.
// Only the original author (signer) is allowed to update.
//
// Expected accounts:
// - accounts[0]: registry_state PDA (read-only in v1)
// - accounts[1]: author account (must be authorized signer and match stored author)
// - accounts[2]: program_entry PDA (must be initialized, author must match)

use spel_framework::prelude::AccountWithMetadata;
use spel_framework::prelude::{AccountPostState, ChainedCall};
use registry_core::{ProgramEntry, UpdateArgs};

/// Handle the Update instruction.
///
/// Authorization:
/// - `author_account.is_authorized` must be true (transaction signature check).
/// - The stored `ProgramEntry.author` must equal `author_account.account_id` (ownership check).
///
/// Note: PDA derivation correctness is enforced by the LEZ framework.
pub fn handle(accounts: &[AccountWithMetadata], args: &UpdateArgs) -> (Vec<AccountPostState>, Vec<ChainedCall>) {
    assert!(
        accounts.len() >= 3,
        "Update requires registry_state + author + program_entry accounts, got {}",
        accounts.len()
    );

    let registry_state_account = &accounts[0];
    let author_account = &accounts[1];
    let program_entry_account = &accounts[2];

    // Author must sign
    assert!(author_account.is_authorized, "Author must sign the Update transaction");

    // Deserialize existing program entry — must be initialized
    let entry_data: Vec<u8> = program_entry_account.account.data.clone().into();
    assert!(
        !entry_data.is_empty(),
        "program_entry PDA is not initialized — register the program first"
    );
    let mut entry: ProgramEntry = borsh::from_slice(&entry_data).expect("Failed to deserialize ProgramEntry");

    // Verify author matches stored entry
    assert_eq!(
        entry.author, author_account.account_id,
        "Only the original author can update this program entry"
    );

    // Apply updates (empty string = no change)
    if !args.version.is_empty() {
        entry.version = args.version.clone();
    }
    if !args.idl_cid.is_empty() {
        entry.idl_cid = args.idl_cid.clone();
    }
    if !args.description.is_empty() {
        entry.description = args.description.clone();
    }
    if !args.tags.is_empty() {
        entry.tags = args.tags.clone();
    }

    // Serialize updated entry
    let entry_bytes = borsh::to_vec(&entry).expect("Failed to serialize ProgramEntry");
    let mut program_entry_post = program_entry_account.account.clone();
    program_entry_post.data = entry_bytes.try_into().expect("ProgramEntry too large");

    // Registry state passes through unchanged (no counter update for updates)
    let registry_state_post = registry_state_account.account.clone();
    let author_post = author_account.account.clone();

    (
        vec![
            AccountPostState::new(registry_state_post),
            AccountPostState::new(author_post),
            AccountPostState::new(program_entry_post),
        ],
        vec![],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nssa_core::account::{Account, AccountId};
    use registry_core::{ProgramEntry, UpdateArgs};

    fn make_account(id: &[u8; 32], data: Vec<u8>, authorized: bool) -> AccountWithMetadata {
        let mut account = Account::default();
        if !data.is_empty() {
            account.data = data.try_into().unwrap();
        }
        AccountWithMetadata {
            account_id: AccountId::new(*id),
            account,
            is_authorized: authorized,
        }
    }

    fn test_program_id() -> nssa_core::program::ProgramId {
        [1u32, 2, 3, 4, 5, 6, 7, 8]
    }

    fn make_entry(author_id: &[u8; 32]) -> ProgramEntry {
        ProgramEntry::new(
            test_program_id(),
            "test-program".to_string(),
            "0.1.0".to_string(),
            AccountId::new(*author_id),
            "bafy_original_cid".to_string(),
            "Original description".to_string(),
            1000,
            vec!["original_tag".to_string()],
        )
    }

    /// Build three accounts for update tests.
    /// PDA ID is a dummy — framework validates real PDA correctness.
    fn make_test_accounts(author_id: &[u8; 32], entry: &ProgramEntry, authorized: bool) -> Vec<AccountWithMetadata> {
        let entry_bytes = borsh::to_vec(entry).unwrap();
        vec![
            make_account(&[10u8; 32], vec![], false), // registry_state (pass-through)
            make_account(author_id, vec![], authorized),
            AccountWithMetadata {
                account_id: AccountId::new([20u8; 32]), // program_entry PDA (dummy id)
                account: {
                    let mut a = Account::default();
                    a.data = entry_bytes.try_into().unwrap();
                    a
                },
                is_authorized: false,
            },
        ]
    }

    fn default_update_args() -> UpdateArgs {
        UpdateArgs {
            program_id: test_program_id(),
            version: "0.2.0".to_string(),
            idl_cid: "bafy_new_cid".to_string(),
            description: "Updated description".to_string(),
            tags: vec!["updated".to_string()],
        }
    }

    #[test]
    fn test_update_changes_metadata() {
        let author_id = [1u8; 32];
        let entry = make_entry(&author_id);
        let args = default_update_args();

        let accounts = make_test_accounts(&author_id, &entry, true);
        let (post_states, chained) = handle(&accounts, &args);

        assert!(chained.is_empty());
        assert_eq!(post_states.len(), 3);

        let updated: ProgramEntry = borsh::from_slice(&Vec::from(post_states[2].account().data.clone())).unwrap();

        assert_eq!(updated.version, "0.2.0");
        assert_eq!(updated.idl_cid, "bafy_new_cid");
        assert_eq!(updated.description, "Updated description");
        assert_eq!(updated.tags, vec!["updated".to_string()]);
        // Immutable fields unchanged
        assert_eq!(updated.name, "test-program");
        assert_eq!(updated.registered_at, 1000);
        assert_eq!(updated.author, AccountId::new(author_id));
    }

    #[test]
    fn test_update_preserves_fields_when_empty() {
        let author_id = [1u8; 32];
        let entry = make_entry(&author_id);

        // Only update version; leave other fields empty → they should be preserved
        let args = UpdateArgs {
            program_id: test_program_id(),
            version: "0.3.0".to_string(),
            idl_cid: String::new(),     // empty → keep original
            description: String::new(), // empty → keep original
            tags: vec![],               // empty → keep original
        };

        let accounts = make_test_accounts(&author_id, &entry, true);
        let (post_states, _) = handle(&accounts, &args);

        let updated: ProgramEntry = borsh::from_slice(&Vec::from(post_states[2].account().data.clone())).unwrap();

        assert_eq!(updated.version, "0.3.0");
        assert_eq!(updated.idl_cid, "bafy_original_cid"); // unchanged
        assert_eq!(updated.description, "Original description"); // unchanged
        assert_eq!(updated.tags, vec!["original_tag".to_string()]); // unchanged
    }

    #[test]
    #[should_panic(expected = "Author must sign")]
    fn test_update_unsigned_fails() {
        let author_id = [1u8; 32];
        let entry = make_entry(&author_id);
        let args = default_update_args();

        let accounts = make_test_accounts(&author_id, &entry, false); // not authorized
        handle(&accounts, &args);
    }

    #[test]
    #[should_panic(expected = "Only the original author")]
    fn test_update_wrong_author_fails() {
        let original_author = [1u8; 32];
        let entry = make_entry(&original_author);
        let args = default_update_args();

        let wrong_author = [99u8; 32];
        let accounts = make_test_accounts(&wrong_author, &entry, true);
        handle(&accounts, &args);
    }

    #[test]
    #[should_panic(expected = "not initialized")]
    fn test_update_uninitialized_entry_fails() {
        let args = default_update_args();

        let accounts = vec![
            make_account(&[10u8; 32], vec![], false),
            make_account(&[1u8; 32], vec![], true),
            AccountWithMetadata {
                account_id: AccountId::new([20u8; 32]),
                account: Account::default(), // empty / uninitialized
                is_authorized: false,
            },
        ];

        handle(&accounts, &args);
    }
}
