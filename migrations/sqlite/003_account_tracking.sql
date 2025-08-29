-- Account tracking migration
-- Create tracked_accounts table
CREATE TABLE IF NOT EXISTS tracked_accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    address TEXT NOT NULL UNIQUE,
    name TEXT,
    program_id TEXT,
    created_at DATETIME NOT NULL,
    is_active BOOLEAN DEFAULT TRUE,
    last_activity DATETIME,
    activity_count INTEGER DEFAULT 0,
    balance_threshold REAL,
    data_size_threshold INTEGER,
    notes TEXT,
    tags TEXT -- JSON array as string for account tags
);

-- Create account_activities table
CREATE TABLE IF NOT EXISTS account_activities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_address TEXT NOT NULL,
    activity_type TEXT NOT NULL,
    change_type TEXT NOT NULL,
    old_value TEXT NOT NULL,
    new_value TEXT NOT NULL,
    timestamp DATETIME NOT NULL,
    block_slot INTEGER NOT NULL,
    lamports_change INTEGER NOT NULL,
    data_size_change INTEGER NOT NULL,
    details TEXT, -- JSON details as string
    transaction_signature TEXT,
    program_id TEXT,
    instruction_type TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (account_address) REFERENCES tracked_accounts (address),
    FOREIGN KEY (block_slot) REFERENCES slots (slot)
);

-- Create account_snapshots table for periodic account state snapshots
CREATE TABLE IF NOT EXISTS account_snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_address TEXT NOT NULL,
    lamports INTEGER NOT NULL,
    data_size INTEGER NOT NULL,
    owner TEXT NOT NULL,
    executable BOOLEAN NOT NULL,
    rent_epoch INTEGER NOT NULL,
    timestamp DATETIME NOT NULL,
    slot INTEGER NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (account_address) REFERENCES tracked_accounts (address),
    FOREIGN KEY (slot) REFERENCES slots (slot)
);

-- Create account_labels table for address labeling
CREATE TABLE IF NOT EXISTS account_labels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    address TEXT NOT NULL,
    label TEXT NOT NULL,
    label_type TEXT NOT NULL, -- 'program', 'token', 'nft', 'dex', 'defi', 'gaming', 'custom'
    source TEXT, -- 'user', 'system', 'api'
    confidence REAL DEFAULT 1.0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Create performance indexes
CREATE INDEX IF NOT EXISTS idx_tracked_accounts_address ON tracked_accounts(address);
CREATE INDEX IF NOT EXISTS idx_tracked_accounts_active ON tracked_accounts(is_active);
CREATE INDEX IF NOT EXISTS idx_tracked_accounts_program ON tracked_accounts(program_id);
CREATE INDEX IF NOT EXISTS idx_tracked_accounts_last_activity ON tracked_accounts(last_activity);

CREATE INDEX IF NOT EXISTS idx_account_activities_address ON account_activities(account_address);
CREATE INDEX IF NOT EXISTS idx_account_activities_timestamp ON account_activities(timestamp);
CREATE INDEX IF NOT EXISTS idx_account_activities_type ON account_activities(activity_type);
CREATE INDEX IF NOT EXISTS idx_account_activities_slot ON account_activities(block_slot);

CREATE INDEX IF NOT EXISTS idx_account_snapshots_address ON account_snapshots(account_address);
CREATE INDEX IF NOT EXISTS idx_account_snapshots_timestamp ON account_snapshots(timestamp);
CREATE INDEX IF NOT EXISTS idx_account_snapshots_slot ON account_snapshots(slot);

CREATE INDEX IF NOT EXISTS idx_account_labels_address ON account_labels(address);
CREATE INDEX IF NOT EXISTS idx_account_labels_type ON account_labels(label_type);
