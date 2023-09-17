CREATE TABLE IF NOT EXISTS signer_seed (
    seed BLOB NOT NULL,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS signer_data (

    statechain_id TEXT,
    token_id TEXT,

    client_seckey_share BLOB UNIQUE,
    client_pubkey_share BLOB UNIQUE,
    backup_address TEXT,

    server_pubkey_share BLOB,
    aggregated_pubkey BLOB,
    p2tr_agg_address TEXT,

    agg_key_derivation_path TEXT,
    auth_derivation_path TEXT,
    change_index INT,
    address_index INT,

    auth_seckey BLOB UNIQUE,
    auth_pubkey BLOB UNIQUE,
    
    fingerprint TEXT,
    
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);