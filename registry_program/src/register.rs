// register.rs — handler for the Register instruction.
//
// Creates a new ProgramEntry PDA and increments the RegistryState counter.
//
// Expected accounts:
// - accounts[0]: registry_state PDA (initialized singleton, mutable)
// - accounts[1]: author account (must be authorized signer)
// - accounts[2]: program_entry PDA (must be uninitialized — Account::default())

use spel_framework::prelude::{Account, AccountWithMetadata};
use spel_framework::prelude::{AccountPostState, ChainedCall, Claim};
use registry_core::{ProgramEntry, RegisterArgs, RegistryState, registry_state_pda_seed, program_entry_pda_seed};

/// Handle the Register instruction.
///
/// Authorization: the author must sign the transaction (`is_authorized = true`).
/// The registry is permissionless — anyone can register a program.
///
/// Note: PDA derivation correctness is enforced by the LEZ framework via the
/// `pda` attribute in the guest binary. The handler just trusts account ordering.
pub fn handle(
    accounts: &[AccountWithMetadata],
    args: &RegisterArgs,
    timestamp: u64,
) -> (Vec<AccountPostState>, Vec<ChainedCall>) {
    assert!(
        accounts.len() >= 3,
        "Register requires registry_state + author + program_entry accounts, got {}",
        accounts.len()
    );

    let registry_state_account = &accounts[0];
    let author_account = &accounts[1];
    let program_entry_account = &accounts[2];

    // Author must sign
    assert!(
        author_account.is_authorized,
        "Author must sign the Register transaction"
    );

    // program_entry PDA must be uninitialized
    assert!(
        program_entry_account.account == Account::default(),
        "program_entry PDA must be uninitialized"
    );

    // Validate inputs
    assert!(!args.name.is_empty(), "Program name must not be empty");
    assert!(!args.version.is_empty(), "Program version must not be empty");

    // Deserialize registry state (may be default/empty on first use)
    let state_data: Vec<u8> = registry_state_account.account.data.clone().into();
    let mut state: RegistryState = if state_data.is_empty() {
        RegistryState::new(author_account.account_id.clone())
    } else {
        borsh::from_slice(&state_data).expect("Failed to deserialize RegistryState")
    };

    // Increment program count
    state.increment();

    // Build the new ProgramEntry
    let entry = ProgramEntry::new(
        args.program_id,
        args.name.clone(),
        args.version.clone(),
        author_account.account_id.clone(),
        args.idl_cid.clone(),
        args.description.clone(),
        timestamp,
        args.tags.clone(),
    );

    // Serialize updated registry state
    let state_bytes = borsh::to_vec(&state).expect("Failed to serialize RegistryState");
    let mut registry_state_post = registry_state_account.account.clone();
    registry_state_post.data = state_bytes.try_into().expect("RegistryState too large");

    // Serialize new program entry
    let entry_bytes = borsh::to_vec(&entry).expect("Failed to serialize ProgramEntry");
    let mut program_entry_post = Account::default();
    program_entry_post.data = entry_bytes.try_into().expect("ProgramEntry too large");

    // Author account passes through unchanged
    let author_post = author_account.account.clone();

    // Use new_claimed_if_default for registry_state so the first call (when the
    // account is still Account::default()) properly claims it. Subsequent calls
    // where the account is already owned by this program will use new() semantics.
    (
        vec![
            AccountPostState::new_claimed_if_default(registry_state_post, Claim::Pda(registry_state_pda_seed())),
            AccountPostState::new(author_post),
            AccountPostState::new_claimed(program_entry_post, Claim::Pda(program_entry_pda_seed(&args.program_id))),
        ],
        vec![],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nssa_core::account::{Account, AccountId};
    use registry_core::{ProgramEntry, RegisterArgs, RegistryState};

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

    fn default_args() -> RegisterArgs {
        RegisterArgs {
            program_id: test_program_id(),
            name: "test-program".to_string(),
            version: "0.1.0".to_string(),
            idl_cid: "bafy2bzacedqwerty".to_string(),
            description: "A test program".to_string(),
            tags: vec!["test".to_string(), "example".to_string()],
        }
    }

    /// Build the three standard accounts needed for Register tests.
    /// The program_entry_pda account ID is set to a dummy value — PDA correctness
    /// is validated by the LEZ framework, not by the handler.
    fn make_test_accounts(state_data: Vec<u8>, author_id: &[u8; 32], authorized: bool) -> Vec<AccountWithMetadata> {
        vec![
            make_account(&[10u8; 32], state_data, false), // registry_state
            make_account(author_id, vec![], authorized),  // author
            AccountWithMetadata {
                account_id: AccountId::new([20u8; 32]), // program_entry PDA (dummy id)
                account: Account::default(),
                is_authorized: false,
            },
        ]
    }

    #[test]
    fn test_register_creates_entry_and_increments_count() {
        let args = default_args();
        let author_id = [1u8; 32];

        let accounts = make_test_accounts(vec![], &author_id, true);
        let (post_states, chained) = handle(&accounts, &args, 12345);

        assert!(chained.is_empty());
        assert_eq!(post_states.len(), 3);

        // Verify registry state was updated
        let state: RegistryState = borsh::from_slice(&Vec::from(post_states[0].account().data.clone())).unwrap();
        assert_eq!(state.program_count, 1);

        // Verify program entry was written correctly
        let entry: ProgramEntry = borsh::from_slice(&Vec::from(post_states[2].account().data.clone())).unwrap();
        assert_eq!(entry.name, "test-program");
        assert_eq!(entry.version, "0.1.0");
        assert_eq!(entry.idl_cid, "bafy2bzacedqwerty");
        assert_eq!(entry.description, "A test program");
        assert_eq!(entry.registered_at, 12345);
        assert_eq!(entry.tags, vec!["test".to_string(), "example".to_string()]);
        assert_eq!(entry.author, AccountId::new(author_id));
    }

    #[test]
    fn test_register_increments_existing_count() {
        let args = default_args();

        // Pre-existing state with count=3
        let existing_state = RegistryState {
            authority: AccountId::new([0u8; 32]),
            program_count: 3,
        };
        let state_data = borsh::to_vec(&existing_state).unwrap();
        let accounts = make_test_accounts(state_data, &[1u8; 32], true);

        let (post_states, _) = handle(&accounts, &args, 99999);

        let state: RegistryState = borsh::from_slice(&Vec::from(post_states[0].account().data.clone())).unwrap();
        assert_eq!(state.program_count, 4);
    }

    #[test]
    #[should_panic(expected = "Author must sign")]
    fn test_register_unsigned_fails() {
        let args = default_args();
        let accounts = make_test_accounts(vec![], &[1u8; 32], false); // NOT authorized
        handle(&accounts, &args, 0);
    }

    #[test]
    #[should_panic(expected = "must not be empty")]
    fn test_register_empty_name_fails() {
        let mut args = default_args();
        args.name = String::new();
        let accounts = make_test_accounts(vec![], &[1u8; 32], true);
        handle(&accounts, &args, 0);
    }

    #[test]
    #[should_panic(expected = "must be uninitialized")]
    fn test_register_already_initialized_pda_fails() {
        let args = default_args();

        // Pre-initialized entry account
        let mut existing_account = Account::default();
        existing_account.data = vec![1u8, 2, 3].try_into().unwrap();

        let accounts = vec![
            make_account(&[10u8; 32], vec![], false),
            make_account(&[1u8; 32], vec![], true),
            AccountWithMetadata {
                account_id: AccountId::new([20u8; 32]),
                account: existing_account,
                is_authorized: false,
            },
        ];
        handle(&accounts, &args, 0);
    }
}
