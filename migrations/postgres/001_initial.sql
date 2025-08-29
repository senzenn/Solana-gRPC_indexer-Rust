-- PostgreSQL initial migration
-- Create slots table
CREATE TABLE IF NOT EXISTS slots (
    slot BIGINT PRIMARY KEY,
    blockhash TEXT NOT NULL,
    parent_slot BIGINT NOT NULL,
    finalized BOOLEAN NOT NULL DEFAULT FALSE,
    timestamp TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Create transactions table
CREATE TABLE IF NOT EXISTS transactions (
    signature TEXT PRIMARY KEY,
    slot BIGINT NOT NULL,
    fee BIGINT NOT NULL,
    status TEXT NOT NULL,
    program_ids JSONB NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    FOREIGN KEY (slot) REFERENCES slots(slot)
);

-- Create slot_leaders table
CREATE TABLE IF NOT EXISTS slot_leaders (
    slot BIGINT PRIMARY KEY,
    leader_pubkey TEXT NOT NULL,
    validator_name TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW(),
    FOREIGN KEY (slot) REFERENCES slots(slot)
);

-- Create accounts table
CREATE TABLE IF NOT EXISTS accounts (
    address TEXT PRIMARY KEY,
    lamports BIGINT NOT NULL,
    owner TEXT NOT NULL,
    executable BOOLEAN NOT NULL DEFAULT FALSE,
    slot BIGINT NOT NULL,
    data_size INTEGER DEFAULT 0,
    updated_at TIMESTAMPTZ DEFAULT NOW(),
    created_at TIMESTAMPTZ DEFAULT NOW(),
    FOREIGN KEY (slot) REFERENCES slots(slot)
);

-- Create indexes for better performance
CREATE INDEX IF NOT EXISTS idx_slots_timestamp ON slots(timestamp);
CREATE INDEX IF NOT EXISTS idx_slots_finalized ON slots(finalized);
CREATE INDEX IF NOT EXISTS idx_transactions_slot ON transactions(slot);
CREATE INDEX IF NOT EXISTS idx_transactions_timestamp ON transactions(timestamp);
CREATE INDEX IF NOT EXISTS idx_transactions_status ON transactions(status);
CREATE INDEX IF NOT EXISTS idx_transactions_program_ids ON transactions USING GIN(program_ids);
CREATE INDEX IF NOT EXISTS idx_slot_leaders_pubkey ON slot_leaders(leader_pubkey);
CREATE INDEX IF NOT EXISTS idx_accounts_owner ON accounts(owner);
CREATE INDEX IF NOT EXISTS idx_accounts_updated_at ON accounts(updated_at);
