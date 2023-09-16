use std::{str::FromStr, thread, time::Duration};

use bitcoin::{Network, secp256k1, hashes::sha256, Address, address, TxOut, blockdata::fee_rate};
use secp256k1_zkp::{Secp256k1, Message, PublicKey, musig::MusigKeyAggCache, SecretKey, XOnlyPublicKey};
use serde::{Serialize, Deserialize};
use sqlx::Sqlite;

use crate::{key_derivation, error::CError, electrum};

const TX_SIZE: u64 = 112; // virtual size one input P2TR and one output P2TR
// 163 is the real size one input P2TR and one output P2TR

#[derive(Debug, Serialize, Deserialize)]
pub struct DepositRequestPayload {
    amount: u64,
    auth_key: String,
    token_id: String,
    signed_token_id: String,
}

pub async fn execute(pool: &sqlx::Pool<Sqlite>, token_id: uuid::Uuid, amount: u64, network: Network) -> Result<(), CError> {

    let (statechain_id, client_secret_key, client_pubkey_share, server_pubkey_share) = init(pool, token_id, amount, network).await;
    let (aggregate_pub_key, address) = create_agg_pub_key(pool, &client_pubkey_share, &server_pubkey_share, network).await?;

    let client = electrum_client::Client::new("tcp://127.0.0.1:50001").unwrap();
    // let mut history = electrum::get_address_history(&client, &address);

    println!("address: {}", address.to_string());

/*     println!("waiting for deposit ....");
    while history.len() == 0 {
        history = electrum::get_address_history(&client, &address);
    }

    println!("deposit received");

    let hitory_res = history.pop().unwrap();

    println!("tx_hash: {}", hitory_res.tx_hash); */

    println!("waiting for deposit ....");

    let mut utxo_list = electrum::get_script_list_unspent(&client, &address);

    let delay = Duration::from_secs(5);

    while utxo_list.len() == 0 {
        utxo_list = electrum::get_script_list_unspent(&client, &address);
        thread::sleep(delay);
    }

    let utxo = utxo_list.pop().unwrap();

    println!("utxo: {:?}", utxo);

    let fee_rate_btc_per_kb = electrum::estimate_fee(&client, 1);
    let fee_rate_sats_per_byte = (fee_rate_btc_per_kb * 100000.0) as u64;

    let absolute_fee: u64 = TX_SIZE * fee_rate_sats_per_byte; 
    let amount_out = utxo.value - absolute_fee;

    let to_address = Address::p2tr(&Secp256k1::new(), client_pubkey_share.x_only_public_key().0, None, network);

    let tx_out = TxOut { value: amount_out, script_pubkey: to_address.script_pubkey() };

    let block_header = electrum::block_headers_subscribe_raw(&client);
    let mut block_height = block_header.height;

    block_height = block_height + 12000;

    println!("block_height: {}", block_height);

    let tx = crate::transaction::create(
        block_height as u32,
        &statechain_id,
        &client_secret_key,
        &client_pubkey_share,
        &server_pubkey_share,
        utxo.tx_hash, 
        utxo.tx_pos as u32, 
        &aggregate_pub_key, 
        &address.script_pubkey(), 
        utxo.value, 
        tx_out).await.unwrap();

    println!("tx size: {}", tx.vsize());

    let tx_bytes = bitcoin::consensus::encode::serialize(&tx);
    let txid = electrum::transaction_broadcast_raw(&client, &tx_bytes);

    println!("txid sent: {}", txid);

    Ok(())

}

pub async fn init(pool: &sqlx::Pool<Sqlite>, token_id: uuid::Uuid, amount: u64, network: Network) -> (String, SecretKey, PublicKey, PublicKey) {
    println!("deposit {} {}", token_id, amount);

    let derivation_path = "m/86h/0h/0h";
    let change_index = 0;
    let address_index = key_derivation::get_next_address_index(pool, change_index).await;
    let mut agg_key_data = key_derivation::generate_new_key(pool, derivation_path, change_index, address_index, network).await;
    agg_key_data.token_id = Some(token_id);
    key_derivation::insert_agg_key_data(pool, &agg_key_data).await;

    let client_secret_key = agg_key_data.secret_key;
    let client_pubkey_share = agg_key_data.public_key;

    let derivation_path = "m/89h/0h/0h";
    let mut auth_key_data = key_derivation::generate_new_key(pool, derivation_path, change_index, address_index, network).await;
    auth_key_data.token_id = Some(token_id);

    assert!(auth_key_data.fingerprint == agg_key_data.fingerprint);
    assert!(auth_key_data.address_index == agg_key_data.address_index);
    assert!(auth_key_data.change_index == agg_key_data.change_index);
    assert!(auth_key_data.derivation_path != agg_key_data.derivation_path);

    key_derivation::update_auth_key_data(pool, &auth_key_data, &client_pubkey_share).await;

    let msg = Message::from_hashed_data::<sha256::Hash>(token_id.to_string().as_bytes());

    let secp = Secp256k1::new();
    let auth_secret_key = auth_key_data.secret_key;
    let keypair = secp256k1::KeyPair::from_seckey_slice(&secp, auth_secret_key.as_ref()).unwrap();
    let signed_token_id = secp.sign_schnorr(&msg, &keypair);
    
    let deposit_request_payload = DepositRequestPayload {
        amount,
        auth_key: auth_key_data.public_key.x_only_public_key().0.to_string(),
        token_id: token_id.to_string(),
        signed_token_id: signed_token_id.to_string(),
    };

    let endpoint = "http://127.0.0.1:8000";
    let path = "deposit/init/pod";

    let client: reqwest::Client = reqwest::Client::new();
    let request = client.post(&format!("{}/{}", endpoint, path));

    let value = match request.json(&deposit_request_payload).send().await {
        Ok(response) => {
            let text = response.text().await.unwrap();
            text
        },
        Err(err) => {
            // return Err(CError::Generic(err.to_string()));
            panic!("error: {}", err);
        },
    };

    println!("value: {}", value);

    #[derive(Serialize, Deserialize)]
    pub struct PublicNonceRequestPayload<'r> {
        server_pubkey: &'r str,
        statechain_id: &'r str,
    }

    let response: PublicNonceRequestPayload = serde_json::from_str(value.as_str()).expect(&format!("failed to parse: {}", value.as_str()));

    let server_pubkey_share = PublicKey::from_str(&response.server_pubkey).unwrap();

    let statechain_id = response.statechain_id.to_string();

    update_statechain_id(pool, statechain_id.clone(), &client_pubkey_share).await;

    (statechain_id, client_secret_key, client_pubkey_share, server_pubkey_share)
}

pub async fn update_statechain_id(pool: &sqlx::Pool<Sqlite>, statechain_id: String, client_pubkey: &PublicKey) {
    let query = "\
        UPDATE signer_data \
        SET statechain_id = $1 \
        WHERE client_pubkey_share = $2";

    let _ = sqlx::query(query)
        .bind(&statechain_id)
        .bind(&client_pubkey.serialize().to_vec())
        .execute(pool)
        .await
        .unwrap();
}

pub async fn create_agg_pub_key(pool: &sqlx::Pool<Sqlite>, client_pubkey: &PublicKey, server_pubkey: &PublicKey, network: Network) -> Result<(XOnlyPublicKey, Address), CError> {

    let secp = Secp256k1::new();

    let key_agg_cache = MusigKeyAggCache::new(&secp, &[client_pubkey.to_owned(), server_pubkey.to_owned()]);
    let agg_pk = key_agg_cache.agg_pk();

    let address = Address::p2tr(&Secp256k1::new(), agg_pk, None, network);

    let query = "\
        UPDATE signer_data \
        SET server_pubkey_share= $1, aggregated_pubkey = $2, p2tr_agg_address = $3 \
        WHERE client_pubkey_share = $4";

    let _ = sqlx::query(query)
        .bind(&server_pubkey.serialize().to_vec())
        .bind(&agg_pk.serialize().to_vec())
        .bind(&address.to_string())
        .bind(&client_pubkey.serialize().to_vec())
        .execute(pool)
        .await
        .unwrap();

    Ok((agg_pk,address))

}