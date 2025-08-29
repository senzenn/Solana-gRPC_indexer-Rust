-- MySQL initial migration
-- Create slots table
CREATE TABLE IF NOT EXISTS slots (
    slot BIGINT PRIMARY KEY,
    blockhash VARCHAR(255) NOT NULL,
    parent_slot BIGINT NOT NULL,
    finalized BOOLEAN NOT NULL DEFAULT FALSE,
    timestamp TIMESTAMP NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Create transactions table
CREATE TABLE IF NOT EXISTS transactions (
    signature VARCHAR(255) PRIMARY KEY,
    slot BIGINT NOT NULL,
    fee BIGINT NOT NULL,
    status VARCHAR(50) NOT NULL,
    program_ids JSON NOT NULL,
    timestamp TIMESTAMP NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (slot) REFERENCES slots(slot)
);

-- Create slot_leaders table
CREATE TABLE IF NOT EXISTS slot_leaders (
    slot BIGINT PRIMARY KEY,
    leader_pubkey VARCHAR(255) NOT NULL,
    validator_name VARCHAR(255),
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (slot) REFERENCES slots(slot)
);

-- Create accounts table
CREATE TABLE IF NOT EXISTS accounts (
    address VARCHAR(255) PRIMARY KEY,
    lamports BIGINT NOT NULL,
    owner VARCHAR(255) NOT NULL,
    executable BOOLEAN NOT NULL DEFAULT FALSE,
    slot BIGINT NOT NULL,
    data_size INTEGER DEFAULT 0,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (slot) REFERENCES slots(slot)
);

-- Create indexes for better performance
CREATE INDEX idx_slots_timestamp ON slots(timestamp);
CREATE INDEX idx_slots_finalized ON slots(finalized);
CREATE INDEX idx_transactions_slot ON transactions(slot);
CREATE INDEX idx_transactions_timestamp ON transactions(timestamp);
CREATE INDEX idx_transactions_status ON transactions(status);
CREATE INDEX idx_slot_leaders_pubkey ON slot_leaders(leader_pubkey);
CREATE INDEX idx_accounts_owner ON accounts(owner);
CREATE INDEX idx_accounts_updated_at ON accounts(updated_at);
