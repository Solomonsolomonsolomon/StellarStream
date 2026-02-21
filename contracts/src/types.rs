use soroban_sdk::{contracttype, Address};

// Role definitions for RBAC
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Role {
    Admin,           // Can grant/revoke roles, upgrade contract
    Pauser,          // Can pause/unpause contract
    TreasuryManager, // Can update fees and treasury address
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stream {
    pub sender: Address,
    pub receiver: Address,
    pub token: Address,
    pub amount: i128,
    pub start_time: u64,
    pub cliff_time: u64,
    pub end_time: u64,
    pub withdrawn_amount: i128,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamRequest {
    pub receiver: Address,
    pub amount: i128,
    pub start_time: u64,
    pub cliff_time: u64,
    pub end_time: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataKey {
    Stream(u64),
    StreamId,
    Admin, // Kept for backward compatibility
    FeeBps,
    Treasury,
    IsPaused,
    Role(Address, Role), // New: Store roles per address
}
