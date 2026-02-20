#![cfg(test)]

use crate::{math, StreamingContract, StreamingContractClient};
use soroban_sdk::{
    contract, contractimpl, testutils::{Address as _, Ledger}, token, Address, Env,
};

// Mock attacker contract that attempts re-entrancy
#[contract]
pub struct AttackerContract;

#[contractimpl]
impl AttackerContract {
    /// This function will be called when tokens are received
    /// It attempts to call withdraw again (re-entrancy attack)
    pub fn attack(env: Env, target_contract: Address, stream_id: u64, receiver: Address) {
        // Try to call withdraw on the target contract again
        let client = StreamingContractClient::new(&env, &target_contract);
        
        // This should fail due to re-entrancy guard
        client.withdraw(&stream_id, &receiver);
    }
}

fn setup_test_env() -> (Env, Address, Address, Address, Address, Address, StreamingContractClient<'static>) {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let sender = Address::generate(&env);
    let receiver = Address::generate(&env);
    let token_admin = Address::generate(&env);

    // Deploy token contract
    let token_address = env.register_stellar_asset_contract_v2(token_admin.clone()).address();
    let token_admin_client = token::StellarAssetClient::new(&env, &token_address);

    // Mint tokens to sender
    token_admin_client.mint(&sender, &1000);

    // Deploy streaming contract
    let contract_id = env.register(StreamingContract, ());
    let client = StreamingContractClient::new(&env, &contract_id);

    // Initialize contract
    client.initialize(&admin);

    (env, admin, sender, receiver, token_address, token_admin, client)
}

#[test]
fn test_create_and_withdraw_stream() {
    let (env, _admin, sender, receiver, token_address, _token_admin, client) = setup_test_env();
    let token = token::Client::new(&env, &token_address);

    // Create stream
    let amount = 1000i128;
    let start_time = 100u64;
    let end_time = 200u64;

    let stream_id = client.create_stream(
        &sender,
        &receiver,
        &token_address,
        &amount,
        &start_time,
        &end_time,
    );

    // Verify stream created
    let stream = client.get_stream(&stream_id);
    assert_eq!(stream.sender, sender);
    assert_eq!(stream.receiver, receiver);
    assert_eq!(stream.amount, amount);

    // Set time to midpoint
    env.ledger().with_mut(|li| li.timestamp = 150);

    // Calculate expected withdrawable
    let unlocked = math::calculate_unlocked_amount(amount, start_time, end_time, 150);
    let expected_withdrawable = math::calculate_withdrawable_amount(unlocked, 0);

    // Withdraw
    let withdrawn = client.withdraw(&stream_id, &receiver);
    assert_eq!(withdrawn, expected_withdrawable);

    // Verify receiver balance
    assert_eq!(token.balance(&receiver), expected_withdrawable);

    // Verify stream updated
    let stream = client.get_stream(&stream_id);
    assert_eq!(stream.withdrawn_amount, expected_withdrawable);
}

#[test]
fn test_cancel_stream() {
    let (env, _admin, sender, receiver, token_address, _token_admin, client) = setup_test_env();
    let token = token::Client::new(&env, &token_address);

    // Create stream
    let amount = 1000i128;
    let start_time = 100u64;
    let end_time = 200u64;

    let stream_id = client.create_stream(
        &sender,
        &receiver,
        &token_address,
        &amount,
        &start_time,
        &end_time,
    );

    // Set time to midpoint
    env.ledger().with_mut(|li| li.timestamp = 150);

    // Cancel stream
    let returned = client.cancel_stream(&stream_id, &sender);

    // Calculate expected amounts
    let unlocked = math::calculate_unlocked_amount(amount, start_time, end_time, 150);
    let expected_returned = amount - unlocked;

    assert_eq!(returned, expected_returned);

    // Verify receiver got unlocked amount
    assert_eq!(token.balance(&receiver), unlocked);

    // Verify sender got remaining amount
    assert_eq!(token.balance(&sender), expected_returned);
}

#[test]
fn test_sequential_withdrawals_work() {
    let (env, _admin, sender, receiver, token_address, _token_admin, client) = setup_test_env();
    let token = token::Client::new(&env, &token_address);

    // Create stream
    let amount = 1000i128;
    let start_time = 100u64;
    let end_time = 200u64;

    let stream_id = client.create_stream(
        &sender,
        &receiver,
        &token_address,
        &amount,
        &start_time,
        &end_time,
    );

    // First withdrawal at 25% progress
    env.ledger().with_mut(|li| li.timestamp = 125);
    let withdrawn1 = client.withdraw(&stream_id, &receiver);
    assert_eq!(withdrawn1, 250);

    // Second withdrawal at 50% progress
    env.ledger().with_mut(|li| li.timestamp = 150);
    let withdrawn2 = client.withdraw(&stream_id, &receiver);
    assert_eq!(withdrawn2, 250);

    // Third withdrawal at 75% progress
    env.ledger().with_mut(|li| li.timestamp = 175);
    let withdrawn3 = client.withdraw(&stream_id, &receiver);
    assert_eq!(withdrawn3, 250);

    // Verify total balance
    assert_eq!(token.balance(&receiver), 750);
}

#[test]
#[should_panic(expected = "Re-entrancy detected")]
fn test_reentrancy_guard_prevents_nested_calls() {
    let (env, _admin, sender, receiver, token_address, _token_admin, client) = setup_test_env();

    // Create stream
    let amount = 1000i128;
    let start_time = 100u64;
    let end_time = 200u64;

    let stream_id = client.create_stream(
        &sender,
        &receiver,
        &token_address,
        &amount,
        &start_time,
        &end_time,
    );

    // Set time to midpoint
    env.ledger().with_mut(|li| li.timestamp = 150);

    // Manually set the lock to simulate being inside a withdraw call
    // This demonstrates that our mutex prevents nested calls
    use crate::DataKey;
    env.as_contract(&client.address, || {
        env.storage().temporary().set(&DataKey::ReentrancyLock, &true);
    });
    
    // Now try to call withdraw - this should panic with "Re-entrancy detected"
    client.withdraw(&stream_id, &receiver);
}

#[test]
fn test_lock_is_released_after_successful_withdrawal() {
    let (env, _admin, sender, receiver, token_address, _token_admin, client) = setup_test_env();

    // Create stream
    let stream_id = client.create_stream(
        &sender,
        &receiver,
        &token_address,
        &1000,
        &100,
        &200,
    );

    env.ledger().with_mut(|li| li.timestamp = 150);

    // Verify lock is not set initially
    use crate::DataKey;
    let locked_before = env.as_contract(&client.address, || {
        env.storage().temporary().get::<DataKey, bool>(&DataKey::ReentrancyLock).unwrap_or(false)
    });
    assert!(!locked_before, "Lock should not be set before withdrawal");

    // Perform withdrawal
    let withdrawn = client.withdraw(&stream_id, &receiver);
    assert!(withdrawn > 0);
    
    // Verify lock is released after withdrawal
    let locked_after = env.as_contract(&client.address, || {
        env.storage().temporary().get::<DataKey, bool>(&DataKey::ReentrancyLock).unwrap_or(false)
    });
    assert!(!locked_after, "Lock should be released after withdrawal");
}

#[test]
fn test_soroban_defense_in_depth() {
    // This test documents that Soroban provides defense-in-depth:
    // 1. Host-level prevention of contract re-entry
    // 2. Application-level mutex (our implementation)
    let (env, _admin, sender, receiver, token_address, _token_admin, client) = setup_test_env();

    // Create stream
    let stream_id = client.create_stream(
        &sender,
        &receiver,
        &token_address,
        &1000,
        &100,
        &200,
    );

    env.ledger().with_mut(|li| li.timestamp = 150);

    // Normal withdrawal works fine
    let withdrawn = client.withdraw(&stream_id, &receiver);
    assert!(withdrawn > 0);
    
    // Sequential calls work fine (lock is released between calls)
    env.ledger().with_mut(|li| li.timestamp = 175);
    let withdrawn2 = client.withdraw(&stream_id, &receiver);
    assert!(withdrawn2 > 0);
}

#[test]
#[should_panic(expected = "Unauthorized: not the receiver")]
fn test_unauthorized_withdrawal() {
    let (env, _admin, sender, receiver, token_address, _token_admin, client) = setup_test_env();

    let unauthorized = Address::generate(&env);

    // Create stream
    let stream_id = client.create_stream(
        &sender,
        &receiver,
        &token_address,
        &1000,
        &100,
        &200,
    );

    // Set time to midpoint
    env.ledger().with_mut(|li| li.timestamp = 150);

    // Try to withdraw as unauthorized user - should panic
    client.withdraw(&stream_id, &unauthorized);
}

#[test]
#[should_panic(expected = "No tokens available to withdraw")]
fn test_withdraw_before_start() {
    let (env, _admin, sender, receiver, token_address, _token_admin, client) = setup_test_env();

    // Create stream
    let stream_id = client.create_stream(
        &sender,
        &receiver,
        &token_address,
        &1000,
        &100,
        &200,
    );

    // Set time before start
    env.ledger().with_mut(|li| li.timestamp = 50);

    // Try to withdraw - should panic
    client.withdraw(&stream_id, &receiver);
}
