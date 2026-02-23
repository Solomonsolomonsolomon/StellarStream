#![no_std]
#![allow(clippy::too_many_arguments)]

mod errors;
mod flash_loan;
mod interest;
mod math;
mod oracle;
mod storage;
mod types;
mod vault;
mod voting;

#[cfg(test)]
mod allowlist_test;
#[cfg(test)]
mod clawback_test;
#[cfg(test)]
mod dispute_test;
#[cfg(test)]
mod soulbound_test;
#[cfg(test)]
mod topup_test;
#[cfg(test)]
mod vault_test;
#[cfg(test)]
mod voting_test;

// #[cfg(test)]
// mod interest_test;

// #[cfg(test)]
// mod mock_vault;

// #[cfg(test)]
// mod vault_integration_test;

#[cfg(test)]
mod ttl_stress_test;

use errors::Error;
use soroban_sdk::{contract, contractimpl, symbol_short, token, Address, Env, Vec};
use storage::{PROPOSAL_COUNT, RECEIPT, STREAM_COUNT};
use types::{
    ClawbackEvent, ContributorRequest, CurveType, DataKey, Milestone, ProposalApprovedEvent,
    ProposalCreatedEvent, ReceiptMetadata, ReceiptTransferredEvent, RequestCreatedEvent,
    RequestExecutedEvent, RequestKey, RequestStatus, Role, Stream, StreamCancelledEvent,
    StreamClaimEvent, StreamCreatedEvent, StreamPausedEvent, StreamProposal, StreamReceipt,
    StreamUnpausedEvent,
};

#[contract]
pub struct StellarStreamContract;

#[contractimpl]
impl StellarStreamContract {
    pub fn create_proposal(
        env: Env,
        sender: Address,
        receiver: Address,
        token: Address,
        total_amount: i128,
        start_time: u64,
        end_time: u64,
        required_approvals: u32,
        deadline: u64,
    ) -> Result<u64, Error> {
        sender.require_auth();

        // Validate time range
        if start_time >= end_time {
            return Err(Error::InvalidTimeRange);
        }
        if total_amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if required_approvals == 0 {
            return Err(Error::InvalidApprovalThreshold);
        }
        if deadline <= env.ledger().timestamp() {
            return Err(Error::ProposalExpired);
        }

        let proposal_id: u64 = env.storage().instance().get(&PROPOSAL_COUNT).unwrap_or(0);
        let next_id = proposal_id + 1;

        let proposal = StreamProposal {
            sender: sender.clone(),
            receiver: receiver.clone(),
            token: token.clone(),
            total_amount,
            start_time,
            end_time,
            approvers: Vec::new(&env),
            required_approvals,
            deadline,
            executed: false,
        };

        env.storage()
            .instance()
            .set(&(PROPOSAL_COUNT, proposal_id), &proposal);
        env.storage().instance().set(&PROPOSAL_COUNT, &next_id);

        // Emit ProposalCreatedEvent
        env.events().publish(
            (symbol_short!("create"), sender.clone()),
            ProposalCreatedEvent {
                proposal_id,
                sender: sender.clone(),
                receiver: receiver.clone(),
                token: token.clone(),
                total_amount,
                start_time,
                end_time,
                required_approvals,
                deadline,
                timestamp: env.ledger().timestamp(),
            },
        );

        Ok(proposal_id)
    }

    pub fn approve_proposal(env: Env, proposal_id: u64, approver: Address) -> Result<(), Error> {
        approver.require_auth();

        let key = (PROPOSAL_COUNT, proposal_id);
        let mut proposal: StreamProposal = env
            .storage()
            .instance()
            .get(&key)
            .ok_or(Error::ProposalNotFound)?;

        if proposal.executed {
            return Err(Error::ProposalAlreadyExecuted);
        }
        if env.ledger().timestamp() > proposal.deadline {
            return Err(Error::ProposalExpired);
        }

        for existing_approver in proposal.approvers.iter() {
            if existing_approver == approver {
                return Err(Error::AlreadyApproved);
            }
        }

        proposal.approvers.push_back(approver.clone());
        let approval_count = proposal.approvers.len();

        if approval_count >= proposal.required_approvals {
            proposal.executed = true;
            env.storage().instance().set(&key, &proposal);
            Self::execute_proposal(&env, proposal.clone())?;
        } else {
            env.storage().instance().set(&key, &proposal);
        }

        // Emit ProposalApprovedEvent
        env.events().publish(
            (symbol_short!("approve"), approver.clone()),
            ProposalApprovedEvent {
                proposal_id,
                approver: approver.clone(),
                approval_count,
                required_approvals: proposal.required_approvals,
                timestamp: env.ledger().timestamp(),
            },
        );

        Ok(())
    }

    fn execute_proposal(env: &Env, proposal: StreamProposal) -> Result<u64, Error> {
        // Transfer tokens from proposer to contract
        let token_client = token::Client::new(env, &proposal.token);
        token_client.transfer(
            &proposal.sender,
            &env.current_contract_address(),
            &proposal.total_amount,
        );

        // Allocate next stream id
        let stream_id: u64 = env.storage().instance().get(&STREAM_COUNT).unwrap_or(0);
        let next_id = stream_id + 1;

        let stream = Stream {
            sender: proposal.sender.clone(),
            receiver: proposal.receiver.clone(),
            token: proposal.token.clone(),
            total_amount: proposal.total_amount,
            start_time: proposal.start_time,
            end_time: proposal.end_time,
            withdrawn_amount: 0,
            interest_strategy: 0,
            vault_address: None,
            deposited_principal: proposal.total_amount,
            metadata: None,
            withdrawn: 0,
            cancelled: false,
            receipt_owner: proposal.receiver.clone(),
            is_paused: false,
            paused_time: 0,
            total_paused_duration: 0,
            milestones: Vec::new(env),
            curve_type: CurveType::Linear,
            is_usd_pegged: false,
            usd_amount: 0,
            oracle_address: proposal.sender.clone(),
            oracle_max_staleness: 0,
            price_min: 0,
            price_max: 0,
            is_soulbound: false,     // Proposals default to non-soulbound
            clawback_enabled: false, // Check at runtime if needed
            arbiter: None,
            is_frozen: false,
        };

        env.storage()
            .instance()
            .set(&(STREAM_COUNT, stream_id), &stream);
        env.storage().instance().set(&STREAM_COUNT, &next_id);

        // Emit StreamCreatedEvent
        env.events().publish(
            (symbol_short!("create"), proposal.sender.clone()),
            StreamCreatedEvent {
                stream_id,
                sender: proposal.sender.clone(),
                receiver: proposal.receiver.clone(),
                token: proposal.token,
                total_amount: proposal.total_amount,
                start_time: proposal.start_time,
                end_time: proposal.end_time,
                timestamp: env.ledger().timestamp(),
            },
        );
        Self::mint_receipt(env, stream_id, &proposal.receiver);

        Ok(stream_id)
    }

    /// Create a new stream with optional soulbound locking
    ///
    /// # Parameters
    /// - `is_soulbound`: Set to true to permanently bind this stream to the receiver's address.
    ///   Cannot be changed after stream creation. Irreversible.
    pub fn create_stream(
        env: Env,
        sender: Address,
        receiver: Address,
        token: Address,
        total_amount: i128,
        start_time: u64,
        end_time: u64,
        curve_type: CurveType,
        is_soulbound: bool,
    ) -> Result<u64, Error> {
        let milestones = Vec::new(&env);
        Self::create_stream_with_milestones(
            env,
            sender,
            receiver,
            token,
            total_amount,
            start_time,
            end_time,
            milestones,
            curve_type,
            is_soulbound,
            None, // No vault
        )
    }

    /// Create a new stream with milestones and optional soulbound locking
    ///
    /// # Parameters
    /// - `is_soulbound`: Set to true to permanently bind this stream to the receiver's address.
    ///   Cannot be changed after stream creation. Irreversible.
    pub fn create_stream_with_milestones(
        env: Env,
        sender: Address,
        receiver: Address,
        token: Address,
        total_amount: i128,
        start_time: u64,
        end_time: u64,
        milestones: Vec<Milestone>,
        curve_type: CurveType,
        is_soulbound: bool,
        vault_address: Option<Address>,
    ) -> Result<u64, Error> {
        sender.require_auth();

        // Validate time range
        if start_time >= end_time {
            return Err(Error::InvalidTimeRange);
        }
        if total_amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        // Validate vault if provided
        let vault_shares = if let Some(ref vault) = vault_address {
            // Check if vault is approved
            if !Self::is_vault_approved(env.clone(), vault.clone()) {
                return Err(Error::Unauthorized);
            }

            // Transfer tokens to contract first
            let token_client = token::Client::new(&env, &token);
            token_client.transfer(&sender, &env.current_contract_address(), &total_amount);

            // Deposit to vault and get shares
            vault::deposit_to_vault(&env, vault, &token, total_amount)
                .map_err(|_| Error::InvalidAmount)?
        } else {
            // Standard stream without vault
            let token_client = token::Client::new(&env, &token);
            token_client.transfer(&sender, &env.current_contract_address(), &total_amount);
            0
        };

        let stream_id: u64 = env.storage().instance().get(&STREAM_COUNT).unwrap_or(0);
        let next_id = stream_id + 1;

        let stream = Stream {
            sender: sender.clone(),
            receiver: receiver.clone(),
            token: token.clone(),
            total_amount,
            start_time,
            end_time,
            withdrawn_amount: 0,
            interest_strategy: 0,
            vault_address: vault_address.clone(),
            deposited_principal: total_amount,
            metadata: None,
            withdrawn: 0,
            cancelled: false,
            receipt_owner: receiver.clone(),
            is_paused: false,
            paused_time: 0,
            total_paused_duration: 0,
            milestones,
            curve_type,
            is_usd_pegged: false,
            usd_amount: 0,
            oracle_address: sender.clone(),
            oracle_max_staleness: 0,
            price_min: 0,
            price_max: 0,
            is_soulbound,
            clawback_enabled: false, // TODO: Check token flags
            arbiter: None,
            is_frozen: false,
        };

        let stream_key = (STREAM_COUNT, stream_id);
        
        // Extend contract instance TTL to ensure long-term accessibility
        Self::extend_contract_ttl(&env);
        
        env.storage()
            .instance()
            .set(&stream_key, &stream);
        env.storage().instance().set(&STREAM_COUNT, &next_id);

        // Store vault shares if vault is used
        if vault_shares > 0 {
            env.storage()
                .instance()
                .set(&DataKey::VaultShares(stream_id), &vault_shares);
        }

        // If soulbound, emit event and add to index
        if is_soulbound {
            env.events().publish(
                (symbol_short!("soulbound"), symbol_short!("locked")),
                (stream_id, receiver.clone()),
            );

            // Add to soulbound streams index
            let mut soulbound_streams: Vec<u64> = env
                .storage()
                .persistent()
                .get(&DataKey::SoulboundStreams)
                .unwrap_or(Vec::new(&env));
            soulbound_streams.push_back(stream_id);
            env.storage()
                .persistent()
                .set(&DataKey::SoulboundStreams, &soulbound_streams);
        }

        env.events().publish(
            (symbol_short!("create"), sender.clone()),
            StreamCreatedEvent {
                stream_id,
                sender: sender.clone(),
                receiver: receiver.clone(),
                token,
                total_amount,
                start_time,
                end_time,
                timestamp: env.ledger().timestamp(),
            },
        );
        Self::mint_receipt(&env, stream_id, &receiver);

        Ok(stream_id)
    }

    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();
        
        // Set admin role
        env.storage().instance().set(&DataKey::Admin, &admin);
        
        // Grant all roles to admin
        env.storage().instance().set(&DataKey::Role(admin.clone(), Role::Admin), &true);
        env.storage().instance().set(&DataKey::Role(admin.clone(), Role::Pauser), &true);
        env.storage().instance().set(&DataKey::Role(admin.clone(), Role::TreasuryManager), &true);
    }

    pub fn grant_role(env: Env, admin: Address, target: Address, role: Role) {
        admin.require_auth();
        
        // Check if admin has Admin role
        let has_admin_role: bool = env
            .storage()
            .instance()
            .get(&DataKey::Role(admin, Role::Admin))
            .unwrap_or(false);
            
        if !has_admin_role {
            panic!("Unauthorized");
        }
        
        env.storage().instance().set(&DataKey::Role(target, role), &true);
    }

    pub fn revoke_role(env: Env, admin: Address, target: Address, role: Role) {
        admin.require_auth();
        
        // Check if admin has Admin role
        let has_admin_role: bool = env
            .storage()
            .instance()
            .get(&DataKey::Role(admin, Role::Admin))
            .unwrap_or(false);
            
        if !has_admin_role {
            panic!("Unauthorized");
        }
        
        env.storage().instance().remove(&DataKey::Role(target, role));
    }

    pub fn check_role(env: Env, address: Address, role: Role) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Role(address, role))
            .unwrap_or(false)
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set")
    }

    fn mint_receipt(env: &Env, stream_id: u64, owner: &Address) {
        let receipt = StreamReceipt {
            stream_id,
            owner: owner.clone(),
            minted_at: env.ledger().timestamp(),
        };
        env.storage()
            .instance()
            .set(&(RECEIPT, stream_id), &receipt);
    }

    pub fn get_stream(env: Env, stream_id: u64) -> Result<Stream, Error> {
        env.storage()
            .instance()
            .get(&(STREAM_COUNT, stream_id))
            .ok_or(Error::StreamNotFound)
    }

    pub fn get_soulbound_streams(env: Env) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::SoulboundStreams)
            .unwrap_or(Vec::new(&env))
    }

    pub fn transfer_receiver(
        env: Env,
        stream_id: u64,
        caller: Address,
        new_receiver: Address,
    ) -> Result<(), Error> {
        caller.require_auth();

        let stream_key = (STREAM_COUNT, stream_id);
        let mut stream: Stream = env
            .storage()
            .instance()
            .get(&stream_key)
            .ok_or(Error::StreamNotFound)?;

        // SOULBOUND CHECK FIRST
        if stream.is_soulbound {
            return Err(Error::StreamIsSoulbound);
        }

        // Authorization check: only sender can transfer receiver
        if stream.sender != caller {
            return Err(Error::Unauthorized);
        }

        if stream.cancelled {
            return Err(Error::AlreadyCancelled);
        }

        // Update receiver
        stream.receiver = new_receiver.clone();
        env.storage().instance().set(&stream_key, &stream);

        Ok(())
    }

    /// Top up an active stream with additional funds
    pub fn top_up_stream(
        env: Env,
        stream_id: u64,
        sender: Address,
        amount: i128,
    ) -> Result<(), Error> {
        sender.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let key = (STREAM_COUNT, stream_id);
        let mut stream: Stream = env
            .storage()
            .instance()
            .get(&key)
            .ok_or(Error::StreamNotFound)?;

        if stream.sender != sender {
            return Err(Error::Unauthorized);
        }

        if stream.cancelled {
            return Err(Error::AlreadyCancelled);
        }

        let current_time = env.ledger().timestamp();
        if current_time >= stream.end_time {
            return Err(Error::StreamEnded);
        }

        // Transfer tokens from sender
        let token_client = token::Client::new(&env, &stream.token);
        token_client.transfer(&sender, &env.current_contract_address(), &amount);

        // Calculate new end time based on flow rate
        let total_duration = stream.end_time.saturating_sub(stream.start_time);
        let flow_rate = stream.total_amount / total_duration as i128;

        let new_total = stream.total_amount + amount;
        let additional_duration = amount / flow_rate;
        let new_end_time = stream.end_time + additional_duration as u64;

        stream.total_amount = new_total;
        stream.end_time = new_end_time;
        env.storage().instance().set(&key, &stream);

        env.events().publish(
            (symbol_short!("topup"), stream_id),
            types::StreamToppedUpEvent {
                stream_id,
                sender,
                amount,
                new_total,
                new_end_time,
                timestamp: current_time,
            },
        );

        Ok(())
    }

    pub fn pause_stream(env: Env, stream_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();

        let key = (STREAM_COUNT, stream_id);
        let mut stream: Stream = env
            .storage()
            .instance()
            .get(&key)
            .ok_or(Error::StreamNotFound)?;

        if stream.sender != caller {
            return Err(Error::Unauthorized);
        }
        if stream.cancelled {
            return Err(Error::AlreadyCancelled);
        }
        if stream.is_paused {
            return Ok(());
        }

        stream.is_paused = true;
        stream.paused_time = env.ledger().timestamp();
        env.storage().instance().set(&key, &stream);

        Ok(())
    }

    pub fn unpause_stream(env: Env, stream_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();

        let key = (STREAM_COUNT, stream_id);
        let mut stream: Stream = env
            .storage()
            .instance()
            .get(&key)
            .ok_or(Error::StreamNotFound)?;

        if stream.sender != caller {
            return Err(Error::Unauthorized);
        }
        if stream.cancelled {
            return Err(Error::AlreadyCancelled);
        }
        if !stream.is_paused {
            return Ok(());
        }

        let current_time = env.ledger().timestamp();
        let pause_duration = current_time - stream.paused_time;
        stream.total_paused_duration += pause_duration;
        stream.is_paused = false;
        stream.paused_time = 0;

        env.storage().instance().set(&key, &stream);

        Ok(())
    }

    pub fn withdraw(env: Env, stream_id: u64, caller: Address) -> Result<i128, Error> {
        caller.require_auth();

        let key = (STREAM_COUNT, stream_id);
        let mut stream: Stream = env
            .storage()
            .instance()
            .get(&key)
            .ok_or(Error::StreamNotFound)?;

        if stream.receiver != caller {
            return Err(Error::Unauthorized);
        }

        if stream.cancelled {
            return Err(Error::AlreadyCancelled);
        }
        if stream.is_paused {
            return Err(Error::StreamPaused);
        }

        let current_time = env.ledger().timestamp();
        let unlocked = Self::calculate_unlocked(&stream, current_time);
        let to_withdraw = unlocked - stream.withdrawn_amount;

        if to_withdraw <= 0 {
            return Err(Error::InsufficientBalance);
        }

        stream.withdrawn_amount += to_withdraw;
        env.storage().instance().set(&key, &stream);

        let token_client = token::Client::new(&env, &stream.token);
        token_client.transfer(
            &env.current_contract_address(),
            &stream.receiver,
            &to_withdraw,
        );

        Ok(to_withdraw)
    }

    pub fn cancel(env: Env, stream_id: u64, caller: Address) -> Result<(), Error> {
        caller.require_auth();

        let key = (STREAM_COUNT, stream_id);
        let mut stream: Stream = env
            .storage()
            .instance()
            .get(&key)
            .ok_or(Error::StreamNotFound)?;

        if stream.sender != caller && stream.receiver != caller {
            return Err(Error::Unauthorized);
        }
        if stream.cancelled {
            return Err(Error::AlreadyCancelled);
        }

        let current_time = env.ledger().timestamp();
        let unlocked = Self::calculate_unlocked(&stream, current_time);
        let to_receiver = unlocked - stream.withdrawn_amount;
        let to_sender = stream.total_amount - unlocked;

        stream.cancelled = true;
        stream.withdrawn_amount = unlocked;
        env.storage().instance().set(&key, &stream);

        let token_client = token::Client::new(&env, &stream.token);
        if to_receiver > 0 {
            token_client.transfer(
                &env.current_contract_address(),
                &stream.receiver,
                &to_receiver,
            );
        }
        if to_sender > 0 {
            token_client.transfer(&env.current_contract_address(), &stream.sender, &to_sender);
        }

        Ok(())
    }

    fn calculate_unlocked(stream: &Stream, current_time: u64) -> i128 {
        if current_time <= stream.start_time {
            return 0;
        }

        let mut effective_time = current_time;
        if stream.is_paused {
            effective_time = stream.paused_time;
        }

        let adjusted_end = stream.end_time + stream.total_paused_duration;
        if effective_time >= adjusted_end {
            return stream.total_amount;
        }

        let elapsed = (effective_time - stream.start_time) as i128;
        let paused = stream.total_paused_duration as i128;
        let effective_elapsed = elapsed - paused;

        if effective_elapsed <= 0 {
            return 0;
        }

        let duration = (stream.end_time - stream.start_time) as i128;

        // Calculate base unlocked amount based on curve type
        match stream.curve_type {
            CurveType::Linear => (stream.total_amount * effective_elapsed) / duration,
            CurveType::Exponential => {
                // Use exponential curve with overflow protection
                let adjusted_start = stream.start_time;
                let adjusted_current = stream.start_time + effective_elapsed as u64;

                math::calculate_exponential_unlocked(
                    stream.total_amount,
                    adjusted_start,
                    stream.end_time,
                    adjusted_current,
                )
                .unwrap_or((stream.total_amount * effective_elapsed) / duration)
            }
        }
    }
}