#![no_main]

use spel_framework::prelude::*;

risc0_zkvm::guest::entry!(main);

#[lez_program(instruction = "registry_core::Instruction")]
mod registry_program {
    #[allow(unused_imports)]
    use super::*;
    use ::registry_program as handlers;
    use spel_framework::prelude::AccountWithMetadata;

    /// Register a new program in the on-chain registry.
    ///
    /// Accounts:
    /// - registry_state: singleton PDA tracking global registry stats (mutable)
    /// - author: the signer who is registering the program (authorized)
    /// - program_entry_pda: the new per-program PDA to initialize (init, pda derived from program_id)
    #[instruction]
    pub fn register(
        #[account(mut)] registry_state: AccountWithMetadata,
        #[account(signer)] author: AccountWithMetadata,
        #[account(init)] program_entry_pda: AccountWithMetadata,
        program_id: ProgramId,
        name: String,
        version: String,
        idl_cid: String,
        description: String,
        tags: Vec<String>,
    ) -> SpelResult {
        let accounts = vec![registry_state, author, program_entry_pda];
        let args = registry_core::RegisterArgs {
            program_id,
            name,
            version,
            idl_cid,
            description,
            tags,
        };
        let (post_states, chained_calls) = handlers::register::handle(&accounts, &args, 0);
        let accs: Vec<Account> = post_states.iter().map(|ps| ps.account().clone()).collect();
        let claims: Vec<AutoClaim> = post_states.iter().map(|ps| {
            match ps.required_claim() {
                Some(claim) => AutoClaim::Claimed(claim),
                None => AutoClaim::None,
            }
        }).collect();
        Ok(SpelOutput::execute_with_claims(&accs, &claims, chained_calls))
    }

    /// Update metadata for an existing registered program.
    ///
    /// Accounts:
    /// - registry_state: singleton PDA (read-only in v1, pass-through)
    /// - author: must be the original registrant (authorized signer)
    /// - program_entry_pda: the existing per-program PDA (mutable)
    #[instruction]
    pub fn update(
        #[account(mut)] registry_state: AccountWithMetadata,
        #[account(signer)] author: AccountWithMetadata,
        #[account(mut)] program_entry_pda: AccountWithMetadata,
        program_id: ProgramId,
        version: String,
        idl_cid: String,
        description: String,
        tags: Vec<String>,
    ) -> SpelResult {
        let accounts = vec![registry_state, author, program_entry_pda];
        let args = registry_core::UpdateArgs {
            program_id,
            version,
            idl_cid,
            description,
            tags,
        };
        let (post_states, chained_calls) = handlers::update::handle(&accounts, &args);
        let accs: Vec<Account> = post_states.iter().map(|ps| ps.account().clone()).collect();
        let claims: Vec<AutoClaim> = post_states.iter().map(|ps| {
            match ps.required_claim() {
                Some(claim) => AutoClaim::Claimed(claim),
                None => AutoClaim::None,
            }
        }).collect();
        Ok(SpelOutput::execute_with_claims(&accs, &claims, chained_calls))
    }
}
