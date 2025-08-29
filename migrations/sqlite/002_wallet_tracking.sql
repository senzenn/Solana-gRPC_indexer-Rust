-- Wallet tracking migration
-- Create tracked_wallets table
CREATE TABLE IF NOT EXISTS tracked_wallets (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    address TEXT NOT NULL UNIQUE,
    name TEXT,
    created_at DATETIME NOT NULL,
    is_active BOOLEAN DEFAULT TRUE,
    last_activity DATETIME,
    activity_count INTEGER DEFAULT 0,
    notes TEXT,
    tags TEXT -- JSON array as string for wallet tags
);

-- Create wallet_activities table
CREATE TABLE IF NOT EXISTS wallet_activities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    activity_type TEXT NOT NULL,
    transaction_signature TEXT NOT NULL,
    amount REAL,
    token_symbol TEXT,
    counterparty TEXT,
    timestamp DATETIME NOT NULL,
    block_slot INTEGER NOT NULL,
    fee INTEGER NOT NULL,
    status TEXT NOT NULL,
    details TEXT, -- JSON details as string
    program_id TEXT,
    instruction_type TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES tracked_wallets (address),
    FOREIGN KEY (transaction_signature) REFERENCES transactions (signature),
    FOREIGN KEY (block_slot) REFERENCES slots (slot)
);

-- Create wallet_balances table for balance tracking
CREATE TABLE IF NOT EXISTS wallet_balances (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    wallet_address TEXT NOT NULL,
    token_mint TEXT, -- NULL for SOL, mint address for SPL tokens
    token_symbol TEXT,
    balance REAL NOT NULL,
    timestamp DATETIME NOT NULL,
    slot INTEGER NOT NULL,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (wallet_address) REFERENCES tracked_wallets (address),
    FOREIGN KEY (slot) REFERENCES slots (slot)
);

-- Create wallet_labels table for address labeling
CREATE TABLE IF NOT EXISTS wallet_labels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    address TEXT NOT NULL,
    label TEXT NOT NULL,
    label_type TEXT NOT NULL, -- 'exchange', 'dex', 'defi', 'nft', 'gaming', 'custom'
    source TEXT, -- 'user', 'system', 'api'
    confidence REAL DEFAULT 1.0,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Create performance indexes
CREATE INDEX IF NOT EXISTS idx_tracked_wallets_address ON tracked_wallets(address);
CREATE INDEX IF NOT EXISTS idx_tracked_wallets_active ON tracked_wallets(is_active);
CREATE INDEX IF NOT EXISTS idx_tracked_wallets_last_activity ON tracked_wallets(last_activity);

CREATE INDEX IF NOT EXISTS idx_wallet_activities_address ON wallet_activities(wallet_address);
CREATE INDEX IF NOT EXISTS idx_wallet_activities_timestamp ON wallet_activities(timestamp);
CREATE INDEX IF NOT EXISTS idx_wallet_activities_type ON wallet_activities(activity_type);
CREATE INDEX IF NOT EXISTS idx_wallet_activities_signature ON wallet_activities(transaction_signature);
CREATE INDEX IF NOT EXISTS idx_wallet_activities_slot ON wallet_activities(block_slot);

CREATE INDEX IF NOT EXISTS idx_wallet_balances_address ON wallet_balances(wallet_address);
CREATE INDEX IF NOT EXISTS idx_wallet_balances_token ON wallet_balances(token_mint);
CREATE INDEX IF NOT EXISTS idx_wallet_balances_timestamp ON wallet_balances(timestamp);

CREATE INDEX IF NOT EXISTS idx_wallet_labels_address ON wallet_labels(address);
CREATE INDEX IF NOT EXISTS idx_wallet_labels_type ON wallet_labels(label_type);
