#![no_std]

pub mod math;
mod test;
mod types;

#[cfg(test)]
mod upgrade_test;

#[cfg(test)]
mod rbac_test;

use soroban_sdk::{contract, contractimpl, symbol_short, token, Address, Env, Vec};
pub use types::{DataKey, Role, Stream, StreamRequest};

const THRESHOLD: u32 = 518400; // ~30 days
const LIMIT: u32 = 1036800; // ~60 days

#[contract]
pub struct StellarStream;

#[contractimpl]
impl StellarStream {
    // ========== RBAC Functions ==========

    /// Initialize the contract with an admin who has all roles
    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();

        // Grant all roles to the initial admin
        Self::grant_role_internal(&env, &admin, Role::Admin);
        Self::grant_role_internal(&env, &admin, Role::Pauser);
        Self::grant_role_internal(&env, &admin, Role::TreasuryManager);

        // Set legacy admin for backward compatibility
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::IsPaused, &false);
    }

    /// Grant a role to an address (Admin only)
    pub fn grant_role(env: Env, admin: Address, account: Address, role: Role) {
        admin.require_auth();

        // Only Admin role can grant roles
        if !Self::has_role(&env, &admin, Role::Admin) {
            panic!("Unauthorized: Only Admin can grant roles");
        }

        Self::grant_role_internal(&env, &account, role.clone());

        env.events()
            .publish((symbol_short!("grant"), admin), (account, role));
    }

    /// Revoke a role from an address (Admin only)
    pub fn revoke_role(env: Env, admin: Address, account: Address, role: Role) {
        admin.require_auth();

        // Only Admin role can revoke roles
        if !Self::has_role(&env, &admin, Role::Admin) {
            panic!("Unauthorized: Only Admin can revoke roles");
        }

        Self::revoke_role_internal(&env, &account, role.clone());

        env.events()
            .publish((symbol_short!("revoke"), admin), (account, role));
    }

    /// Check if an address has a specific role (internal helper)
    fn has_role(env: &Env, account: &Address, role: Role) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Role(account.clone(), role))
            .unwrap_or(false)
    }

    /// Check if an address has a specific role (public query function)
    pub fn check_role(env: Env, account: Address, role: Role) -> bool {
        Self::has_role(&env, &account, role)
    }

    // Internal helper to grant role
    fn grant_role_internal(env: &Env, account: &Address, role: Role) {
        env.storage()
            .instance()
            .set(&DataKey::Role(account.clone(), role), &true);
    }

    // Internal helper to revoke role
    fn revoke_role_internal(env: &Env, account: &Address, role: Role) {
        env.storage()
            .instance()
            .remove(&DataKey::Role(account.clone(), role));
    }

    // ========== Fee Management (TreasuryManager role) ==========

    pub fn initialize_fee(env: Env, admin: Address, fee_bps: u32, treasury: Address) {
        admin.require_auth();

        // Check if caller has TreasuryManager role
        if !Self::has_role(&env, &admin, Role::TreasuryManager) {
            panic!("Unauthorized: Only TreasuryManager can set fees");
        }

        if fee_bps > 1000 {
            panic!("Fee cannot exceed 10%");
        }
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
        env.storage().instance().set(&DataKey::Treasury, &treasury);
    }

    pub fn update_fee(env: Env, manager: Address, fee_bps: u32) {
        manager.require_auth();

        // Check if caller has TreasuryManager role
        if !Self::has_role(&env, &manager, Role::TreasuryManager) {
            panic!("Unauthorized: Only TreasuryManager can update fee");
        }

        if fee_bps > 1000 {
            panic!("Fee cannot exceed 10%");
        }
        env.storage().instance().set(&DataKey::FeeBps, &fee_bps);
    }

    pub fn update_treasury(env: Env, manager: Address, treasury: Address) {
        manager.require_auth();

        // Check if caller has TreasuryManager role
        if !Self::has_role(&env, &manager, Role::TreasuryManager) {
            panic!("Unauthorized: Only TreasuryManager can update treasury");
        }

        env.storage().instance().set(&DataKey::Treasury, &treasury);
    }

    // ========== Pause Management (Pauser role) ==========

    pub fn set_pause(env: Env, pauser: Address, paused: bool) {
        pauser.require_auth();

        // Check if caller has Pauser role
        if !Self::has_role(&env, &pauser, Role::Pauser) {
            panic!("Unauthorized: Only Pauser can pause/unpause");
        }

        env.storage().instance().set(&DataKey::IsPaused, &paused);

        env.events()
            .publish((symbol_short!("pause"), pauser), paused);
    }

    fn check_not_paused(env: &Env) {
        let is_paused: bool = env
            .storage()
            .instance()
            .get(&DataKey::IsPaused)
            .unwrap_or(false);
        if is_paused {
            panic!("Contract is paused");
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_stream(
        env: Env,
        sender: Address,
        receiver: Address,
        token: Address,
        amount: i128,
        start_time: u64,
        cliff_time: u64,
        end_time: u64,
    ) -> u64 {
        Self::check_not_paused(&env);
        sender.require_auth();

        if end_time <= start_time {
            panic!("End time must be after start time");
        }
        if cliff_time < start_time || cliff_time >= end_time {
            panic!("Cliff time must be between start and end time");
        }
        if amount <= 0 {
            panic!("Amount must be greater than zero");
        }

        let token_client = token::Client::new(&env, &token);
        let fee_bps: u32 = env.storage().instance().get(&DataKey::FeeBps).unwrap_or(0);
        let fee_amount = (amount * fee_bps as i128) / 10000;
        let principal = amount - fee_amount;

        token_client.transfer(&sender, &env.current_contract_address(), &principal);

        if fee_amount > 0 {
            let treasury: Address = env
                .storage()
                .instance()
                .get(&DataKey::Treasury)
                .expect("Treasury not set");
            token_client.transfer(&sender, &treasury, &fee_amount);
        }

        let mut stream_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::StreamId)
            .unwrap_or(0);
        stream_id += 1;
        env.storage().instance().set(&DataKey::StreamId, &stream_id);
        env.storage().instance().extend_ttl(THRESHOLD, LIMIT);

        let stream = Stream {
            sender: sender.clone(),
            receiver,
            token,
            amount: principal,
            start_time,
            cliff_time,
            end_time,
            withdrawn_amount: 0,
        };

        let stream_key = DataKey::Stream(stream_id);
        env.storage().persistent().set(&stream_key, &stream);
        env.storage()
            .persistent()
            .extend_ttl(&stream_key, THRESHOLD, LIMIT);

        env.events()
            .publish((symbol_short!("create"), sender), stream_id);

        stream_id
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_batch_streams(
        env: Env,
        sender: Address,
        token: Address,
        requests: Vec<StreamRequest>,
    ) -> Vec<u64> {
        sender.require_auth();

        let mut total_amount: i128 = 0;
        for request in requests.iter() {
            if request.end_time <= request.start_time {
                panic!("End time must be after start time");
            }
            if request.amount <= 0 {
                panic!("Amount must be greater than zero");
            }
            total_amount += request.amount;
        }

        let token_client = token::Client::new(&env, &token);
        token_client.transfer(&sender, &env.current_contract_address(), &total_amount);

        let mut stream_ids = Vec::new(&env);
        let mut stream_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::StreamId)
            .unwrap_or(0);

        for request in requests.iter() {
            stream_id += 1;

            let stream = Stream {
                sender: sender.clone(),
                receiver: request.receiver.clone(),
                token: token.clone(),
                amount: request.amount,
                start_time: request.start_time,
                cliff_time: request.cliff_time,
                end_time: request.end_time,
                withdrawn_amount: 0,
            };

            env.storage()
                .persistent()
                .set(&DataKey::Stream(stream_id), &stream);

            env.events()
                .publish((symbol_short!("create"), sender.clone()), stream_id);

            stream_ids.push_back(stream_id);
        }

        env.storage().instance().set(&DataKey::StreamId, &stream_id);

        stream_ids
    }

    pub fn withdraw(env: Env, stream_id: u64, receiver: Address) -> i128 {
        Self::check_not_paused(&env);
        receiver.require_auth();

        let stream_key = DataKey::Stream(stream_id);
        let mut stream: Stream = env
            .storage()
            .persistent()
            .get(&stream_key)
            .expect("Stream does not exist");

        if receiver != stream.receiver {
            panic!("Unauthorized: You are not the receiver of this stream");
        }

        let now = env.ledger().timestamp();
        let total_unlocked = math::calculate_unlocked(
            stream.amount,
            stream.start_time,
            stream.cliff_time,
            stream.end_time,
            now,
        );

        let withdrawable_amount = total_unlocked - stream.withdrawn_amount;

        if withdrawable_amount <= 0 {
            panic!("No funds available to withdraw at this time");
        }

        let token_client = token::Client::new(&env, &stream.token);
        token_client.transfer(
            &env.current_contract_address(),
            &receiver,
            &withdrawable_amount,
        );

        stream.withdrawn_amount += withdrawable_amount;
        env.storage().persistent().set(&stream_key, &stream);
        env.storage()
            .persistent()
            .extend_ttl(&stream_key, THRESHOLD, LIMIT);

        env.events().publish(
            (symbol_short!("withdraw"), receiver),
            (stream_id, withdrawable_amount),
        );

        withdrawable_amount
    }

    pub fn cancel_stream(env: Env, stream_id: u64) {
        Self::check_not_paused(&env);
        let stream_key = DataKey::Stream(stream_id);
        let stream: Stream = env
            .storage()
            .persistent()
            .get(&stream_key)
            .expect("Stream does not exist");

        stream.sender.require_auth();

        let now = env.ledger().timestamp();

        if now >= stream.end_time {
            panic!("Stream has already completed and cannot be cancelled");
        }

        let total_unlocked = math::calculate_unlocked(
            stream.amount,
            stream.start_time,
            stream.cliff_time,
            stream.end_time,
            now,
        );

        let withdrawable_to_receiver = total_unlocked - stream.withdrawn_amount;
        let refund_to_sender = stream.amount - total_unlocked;

        let token_client = token::Client::new(&env, &stream.token);
        let contract_address = env.current_contract_address();

        if withdrawable_to_receiver > 0 {
            token_client.transfer(
                &contract_address,
                &stream.receiver,
                &withdrawable_to_receiver,
            );
        }

        if refund_to_sender > 0 {
            token_client.transfer(&contract_address, &stream.sender, &refund_to_sender);
        }

        env.storage().persistent().remove(&stream_key);

        env.events()
            .publish((symbol_short!("cancel"), stream_id), stream.sender);
    }

    pub fn transfer_receiver(env: Env, stream_id: u64, new_receiver: Address) {
        let mut stream: Stream = env
            .storage()
            .persistent()
            .get(&DataKey::Stream(stream_id))
            .expect("Stream does not exist");

        stream.receiver.require_auth();

        stream.receiver = new_receiver.clone();
        env.storage()
            .persistent()
            .set(&DataKey::Stream(stream_id), &stream);

        env.events()
            .publish((symbol_short!("transfer"), stream_id), new_receiver);
    }

    pub fn extend_stream_ttl(env: Env, stream_id: u64) {
        let stream_key = DataKey::Stream(stream_id);
        env.storage()
            .persistent()
            .extend_ttl(&stream_key, THRESHOLD, LIMIT);
    }

    /// Upgrade the contract to a new WASM hash
    /// Upgrade the contract to a new WASM hash
    /// Only addresses with Admin role can perform this operation
    pub fn upgrade(env: Env, admin: Address, new_wasm_hash: soroban_sdk::BytesN<32>) {
        admin.require_auth();

        // Check if caller has Admin role
        if !Self::has_role(&env, &admin, Role::Admin) {
            panic!("Unauthorized: Only Admin can upgrade contract");
        }

        // Update the contract WASM
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());

        // Emit upgrade event with new WASM hash
        env.events()
            .publish((symbol_short!("upgrade"), admin), new_wasm_hash);
    }

    /// Get the current admin address (for backward compatibility)
    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("Admin not set")
    }
}
