use std::str::FromStr;

use bitcoin::{Network, bip32::{ExtendedPrivKey, DerivationPath, ExtendedPubKey, ChildNumber}};
use secp256k1_zkp::{PublicKey, ffi::types::AlignedType, Secp256k1, SecretKey};
use sqlx::{Sqlite, Row};
use uuid::Uuid;

async fn generate_or_get_seed(pool: &sqlx::Pool<Sqlite>) -> [u8; 32] {

    let rows = sqlx::query("SELECT * FROM signer_seed")
        .fetch_all(pool)
        .await
        .unwrap();

    if rows.len() > 1 {
        panic!("More than one seed in database");
    }

    if rows.len() == 1 {
        let row = rows.get(0).unwrap();
        let seed = row.get::<Vec<u8>, _>("seed");
        let mut seed_array = [0u8; 32];
        seed_array.copy_from_slice(&seed);
        return seed_array;
    } else {
        let mut seed = [0u8; 32];  // 256 bits
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut seed);
        
        let query = "INSERT INTO signer_seed (seed) VALUES ($1)";
        let _ = sqlx::query(query)
            .bind(seed.to_vec())
            .execute(pool)
            .await
            .unwrap();

        seed
    }   
}

pub async fn get_next_address_index(pool: &sqlx::Pool<Sqlite>, change_index: u32) -> u32 {

    let row = sqlx::query("SELECT MAX(address_index) FROM signer_data WHERE change_index = $1")
        .bind(change_index)
        .fetch_one(pool)
        .await
        .unwrap();

    let index = row.get::<Option<u32>, _>(0);

    if index.is_some() {
        return index.unwrap() + 1;
    } else {
        return 0;
    }
}

pub async fn generate_new_key(pool: &sqlx::Pool<Sqlite>, derivation_path: &str, change_index: u32, address_index:u32, network: Network) -> KeyData {

    let seed = generate_or_get_seed(pool).await;

    // we need secp256k1 context for key derivation
    let mut buf: Vec<AlignedType> = Vec::new();
    buf.resize(Secp256k1::preallocate_size(), AlignedType::zeroed());
    let secp = Secp256k1::preallocated_new(buf.as_mut_slice()).unwrap();

    // calculate root key from seed
    let root = ExtendedPrivKey::new_master(network, &seed).unwrap();

    let fingerprint = root.fingerprint(&secp).to_string();

    // derive child xpub
    let path = DerivationPath::from_str(derivation_path).unwrap();
    let child = root.derive_priv(&secp, &path).unwrap();
    let xpub = ExtendedPubKey::from_priv(&secp, &child);

    // generate first receiving address at m/0/0
    // manually creating indexes this time
    let change_index_number = ChildNumber::from_normal_idx(change_index).unwrap();
    let address_index_number = ChildNumber::from_normal_idx(address_index).unwrap();

    let derivation_path = format!("{}/{}/{}", derivation_path, change_index, address_index );

    let secret_key = child.derive_priv(&secp, &[change_index_number, address_index_number]).unwrap().private_key;
    let public_key: secp256k1_zkp::PublicKey = xpub.derive_pub(&secp, &[change_index_number, address_index_number]).unwrap().public_key;

    KeyData {
        token_id: None,
        secret_key,
        public_key,
        fingerprint,
        derivation_path,
        change_index,
        address_index,
    }
}

pub async fn insert_agg_key_data(pool: &sqlx::Pool<Sqlite>, key_data: &KeyData)  {

    let query = 
        "INSERT INTO signer_data (token_id, client_seckey_share, client_pubkey_share, fingerprint, agg_key_derivation_path, change_index, address_index) \
        VALUES ($1, $2, $3, $4, $5, $6, $7)";

    let _ = sqlx::query(query)
        .bind(&key_data.token_id.unwrap().to_string())
        .bind(&key_data.secret_key.secret_bytes().to_vec())
        .bind(&key_data.public_key.serialize().to_vec())
        .bind(&key_data.fingerprint)
        .bind(&key_data.derivation_path)
        .bind(key_data.change_index)
        .bind(key_data.address_index)
        .execute(pool)
        .await
        .unwrap();
}

pub async fn update_auth_key_data(pool: &sqlx::Pool<Sqlite>, key_data: &KeyData, client_pubkey_share: &PublicKey)  {

    let query = "\
        UPDATE signer_data \
        SET auth_derivation_path = $1, auth_seckey = $2, auth_pubkey = $3 \
        WHERE client_pubkey_share = $4";

    let _ = sqlx::query(query)
        .bind(&key_data.derivation_path)
        .bind(&key_data.secret_key.secret_bytes().to_vec())
        .bind(&key_data.public_key.serialize().to_vec())
        .bind(&client_pubkey_share.serialize().to_vec())
        .execute(pool)
        .await
        .unwrap();
}

pub struct KeyData {
    pub token_id: Option<Uuid>,
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
    pub fingerprint: String,
    pub derivation_path: String,
    pub change_index: u32,
    pub address_index: u32,
}
