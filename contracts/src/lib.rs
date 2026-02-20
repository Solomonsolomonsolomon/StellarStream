#![no_std]

pub mod math;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, token, Address, Env};
pub use types::{DataKey, Stream};

#[contract]
pub struct StreamingContract;

#[contractimpl]
impl StreamingContract {
    /// Initialize the contract with an admin
    pub fn initialize(env: Env, admin: Address) {
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
    }

    /// Create a new payment stream
    pub fn create_stream(
        env: Env,
        sender: Address,
        receiver: Address,
        token: Address,
        amount: i128,
        start_time: u64,
        end_time: u64,
    ) -> u64 {
        sender.require_auth();

        // Validation
        if amount <= 0 {
            panic!("Amount must be positive");
        }
        if end_time <= start_time {
            panic!("End time must be after start time");
        }

        // Generate stream ID
        let stream_id = env.ledger().sequence() as u64;

        // Create stream
        let stream = Stream {
            sender: sender.clone(),
            receiver,
            token: token.clone(),
            amount,
            start_time,
            end_time,
            withdrawn_amount: 0,
        };

        // Store stream
        env.storage()
            .persistent()
            .set(&DataKey::Stream(stream_id), &stream);

        // Transfer tokens from sender to contract
        let client = token::Client::new(&env, &token);
        client.transfer(&sender, &env.current_contract_address(), &amount);

        stream_id
    }

    /// Withdraw unlocked tokens from a stream (with re-entrancy protection)
    pub fn withdraw(env: Env, stream_id: u64, receiver: Address) -> i128 {
        receiver.require_auth();

        // Re-entrancy guard: Check if locked
        if Self::is_locked(&env) {
            panic!("Re-entrancy detected");
        }

        // Set lock
        Self::set_lock(&env, true);

        // Execute withdrawal logic
        let withdrawn = Self::withdraw_internal(&env, stream_id, &receiver);

        // Release lock
        Self::set_lock(&env, false);

        withdrawn
    }

    /// Cancel a stream and return remaining tokens (with re-entrancy protection)
    pub fn cancel_stream(env: Env, stream_id: u64, sender: Address) -> i128 {
        sender.require_auth();

        // Re-entrancy guard: Check if locked
        if Self::is_locked(&env) {
            panic!("Re-entrancy detected");
        }

        // Set lock
        Self::set_lock(&env, true);

        // Execute cancellation logic
        let returned = Self::cancel_stream_internal(&env, stream_id, &sender);

        // Release lock
        Self::set_lock(&env, false);

        returned
    }

    /// Get stream details
    pub fn get_stream(env: Env, stream_id: u64) -> Stream {
        env.storage()
            .persistent()
            .get(&DataKey::Stream(stream_id))
            .unwrap_or_else(|| panic!("Stream not found"))
    }

    /// Calculate withdrawable amount for a stream
    pub fn get_withdrawable(env: Env, stream_id: u64) -> i128 {
        let stream: Stream = env
            .storage()
            .persistent()
            .get(&DataKey::Stream(stream_id))
            .unwrap_or_else(|| panic!("Stream not found"));

        let current_time = env.ledger().timestamp();
        let unlocked = math::calculate_unlocked_amount(
            stream.amount,
            stream.start_time,
            stream.end_time,
            current_time,
        );

        math::calculate_withdrawable_amount(unlocked, stream.withdrawn_amount)
    }

    // ========== Internal Functions ==========

    fn withdraw_internal(env: &Env, stream_id: u64, receiver: &Address) -> i128 {
        let mut stream: Stream = env
            .storage()
            .persistent()
            .get(&DataKey::Stream(stream_id))
            .unwrap_or_else(|| panic!("Stream not found"));

        // Verify receiver
        if stream.receiver != *receiver {
            panic!("Unauthorized: not the receiver");
        }

        // Calculate withdrawable amount
        let current_time = env.ledger().timestamp();
        let unlocked = math::calculate_unlocked_amount(
            stream.amount,
            stream.start_time,
            stream.end_time,
            current_time,
        );
        let withdrawable = math::calculate_withdrawable_amount(unlocked, stream.withdrawn_amount);

        if withdrawable <= 0 {
            panic!("No tokens available to withdraw");
        }

        // Update stream state
        stream.withdrawn_amount += withdrawable;
        env.storage()
            .persistent()
            .set(&DataKey::Stream(stream_id), &stream);

        // Transfer tokens to receiver
        let client = token::Client::new(env, &stream.token);
        client.transfer(&env.current_contract_address(), receiver, &withdrawable);

        withdrawable
    }

    fn cancel_stream_internal(env: &Env, stream_id: u64, sender: &Address) -> i128 {
        let stream: Stream = env
            .storage()
            .persistent()
            .get(&DataKey::Stream(stream_id))
            .unwrap_or_else(|| panic!("Stream not found"));

        // Verify sender
        if stream.sender != *sender {
            panic!("Unauthorized: not the sender");
        }

        // Calculate amounts
        let current_time = env.ledger().timestamp();
        let unlocked = math::calculate_unlocked_amount(
            stream.amount,
            stream.start_time,
            stream.end_time,
            current_time,
        );
        let withdrawable = math::calculate_withdrawable_amount(unlocked, stream.withdrawn_amount);
        let remaining = stream.amount - stream.withdrawn_amount;

        // Transfer withdrawable to receiver if any
        let client = token::Client::new(env, &stream.token);
        if withdrawable > 0 {
            client.transfer(&env.current_contract_address(), &stream.receiver, &withdrawable);
        }

        // Return remaining to sender
        let to_return = remaining - withdrawable;
        if to_return > 0 {
            client.transfer(&env.current_contract_address(), sender, &to_return);
        }

        // Delete stream
        env.storage().persistent().remove(&DataKey::Stream(stream_id));

        to_return
    }

    // ========== Re-entrancy Guard Functions ==========

    fn is_locked(env: &Env) -> bool {
        env.storage()
            .temporary()
            .get(&DataKey::ReentrancyLock)
            .unwrap_or(false)
    }

    fn set_lock(env: &Env, locked: bool) {
        if locked {
            env.storage()
                .temporary()
                .set(&DataKey::ReentrancyLock, &locked);
        } else {
            env.storage()
                .temporary()
                .remove(&DataKey::ReentrancyLock);
        }
    }
}
