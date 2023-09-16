use std::{str::FromStr, collections::BTreeMap};

use bitcoin::{Txid, ScriptBuf, Transaction, absolute, TxIn, OutPoint, Witness, TxOut, psbt::{Psbt, Input, PsbtSighashType}, sighash::{TapSighashType, SighashCache, self, TapSighash}, secp256k1, taproot::{TapTweakHash, self}, hashes::Hash, locktime};
use secp256k1_zkp::{SecretKey, PublicKey, XOnlyPublicKey, Secp256k1, schnorr::Signature, Message, musig::{MusigSessionId, MusigPubNonce, MusigKeyAggCache, MusigAggNonce, BlindingFactor, MusigSession, MusigPartialSignature}, new_musig_nonce_pair, KeyPair};
use serde::{Serialize, Deserialize};

use crate::error::CError;

pub async fn create(
    block_height: u32,
    statechain_id: &str,
    client_seckey: &SecretKey,
    client_pubkey: &PublicKey,
    server_pubkey: &PublicKey,
    input_txid: Txid, 
    input_vout: u32, 
    input_pubkey: &XOnlyPublicKey, 
    input_scriptpubkey: &ScriptBuf, 
    input_amount: u64, 
    output: TxOut) -> Result<Transaction, Box<dyn std::error::Error>> {

    let outputs = [output].to_vec();

    let lock_time = absolute::LockTime::from_height(block_height).expect("valid height");

    let tx1 = Transaction {
        version: 2,
        lock_time,
        input: vec![TxIn {
            previous_output: OutPoint { txid: input_txid, vout: input_vout },
            script_sig: ScriptBuf::new(),
            sequence: bitcoin::Sequence(0xFFFFFFFF), // Ignore nSequence.
            witness: Witness::default(),
        }],
        output: outputs,
    };
    let mut psbt = Psbt::from_unsigned_tx(tx1)?;

    let mut input = Input {
        witness_utxo: Some(TxOut { value: input_amount, script_pubkey: input_scriptpubkey.to_owned() }),
        ..Default::default()
    };
    let ty = PsbtSighashType::from_str("SIGHASH_ALL")?;
    input.sighash_type = Some(ty);
    input.tap_internal_key = Some(input_pubkey.to_owned());
    psbt.inputs = vec![input];

    let secp = Secp256k1::new();
    
    let unsigned_tx = psbt.unsigned_tx.clone();
    for (vout, input) in psbt.inputs.iter_mut().enumerate() {

        let hash_ty = input
            .sighash_type
            .and_then(|psbt_sighash_type| psbt_sighash_type.taproot_hash_ty().ok())
            .unwrap_or(TapSighashType::All);

        let hash = SighashCache::new(&unsigned_tx).taproot_key_spend_signature_hash(
            vout,
            &sighash::Prevouts::All(&[TxOut {
                value: input.witness_utxo.as_ref().unwrap().value,
                script_pubkey: input.witness_utxo.as_ref().unwrap().script_pubkey.clone(),
            }]),
            hash_ty,
        ).unwrap();

        let sig = musig_sign_psbt_taproot(
            statechain_id,
            client_seckey,
            client_pubkey,
            server_pubkey,
            input_pubkey,
            hash,
            &secp,
        ).await.unwrap();

        println!("sig: {}", sig.to_string());

        let final_signature = taproot::Signature { sig, hash_ty };

        input.tap_key_sig = Some(final_signature);
    }

    // FINALIZER
    psbt.inputs.iter_mut().for_each(|input| {
        let mut script_witness: Witness = Witness::new();
        script_witness.push(input.tap_key_sig.unwrap().to_vec());
        input.final_script_witness = Some(script_witness);

        // Clear all the data fields as per the spec.
        input.partial_sigs = BTreeMap::new();
        input.sighash_type = None;
        input.redeem_script = None;
        input.witness_script = None;
        input.bip32_derivation = BTreeMap::new();
    });

    let tx = psbt.extract_tx();

    tx.verify(|_| {
        Some(TxOut {
            value: input_amount,
            script_pubkey: input_scriptpubkey.to_owned(),
        })
    })
    .expect("failed to verify transaction");


    Ok(tx)
}


#[derive(Serialize, Deserialize)]
pub struct PublicNonceRequestPayload<'r> {
    statechain_id: &'r str,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ServerPublicNonceResponsePayload<'r> {
    server_pubnonce: &'r str,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PartialSignatureRequestPayload<'r> {
    statechain_id: &'r str,
    keyaggcoef: &'r str,
    negate_seckey: u8,
    session: &'r str,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PartialSignatureResponsePayload<'r> {
    partial_sig: &'r str,
}

async fn musig_sign_psbt_taproot(
    statechain_id: &str,
    client_seckey: &SecretKey,
    client_pubkey: &PublicKey,
    server_pubkey: &PublicKey,
    aggregated_pubkey: &XOnlyPublicKey,
    hash: TapSighash,
    secp: &Secp256k1<secp256k1::All>,
)  -> Result<Signature, CError>  {
    let msg: Message = hash.into();

    // let msg = Message::from_hashed_data::<sha256::Hash>(hash.as_ref());

    let msg_hex = hex::encode(msg.as_ref());
    println!("msg: {}", msg_hex);

    let client_session_id = MusigSessionId::new(&mut rand::thread_rng());

    let (client_sec_nonce, client_pub_nonce) = new_musig_nonce_pair(&secp, client_session_id, None, Some(client_seckey.to_owned()), client_pubkey.to_owned(), None, None).unwrap();

    let endpoint = "http://127.0.0.1:8000";
    let path = "public_nonce";

    let client: reqwest::Client = reqwest::Client::new();
    let request = client.post(&format!("{}/{}", endpoint, path));

    let payload = PublicNonceRequestPayload {
        statechain_id,
    };

    let value = match request.json(&payload).send().await {
        Ok(response) => {
            let text = response.text().await.unwrap();
            text
        },
        Err(err) => {
            return Err(CError::Generic(err.to_string()));
        },
    };

    let response: ServerPublicNonceResponsePayload = serde_json::from_str(value.as_str()).expect(&format!("failed to parse: {}", value.as_str()));

    let mut server_pubnonce_hex = response.server_pubnonce.to_string();

    if server_pubnonce_hex.starts_with("0x") {
        server_pubnonce_hex = server_pubnonce_hex[2..].to_string();
    }

    let server_pub_nonce_bytes = hex::decode(server_pubnonce_hex).unwrap();
    
    let server_pub_nonce = MusigPubNonce::from_slice(server_pub_nonce_bytes.as_slice()).unwrap();

    let mut key_agg_cache = MusigKeyAggCache::new(&secp, &[client_pubkey.to_owned(), server_pubkey.to_owned()]);

    let tap_tweak = TapTweakHash::from_key_and_tweak(key_agg_cache.agg_pk(), None);
    let tap_tweak_bytes = tap_tweak.as_byte_array();

    // tranform tweak: Scalar to SecretKey
    let tweak = SecretKey::from_slice(tap_tweak_bytes).unwrap();

    let tweaked_pubkey = key_agg_cache.pubkey_xonly_tweak_add(secp, tweak).unwrap();

    let aggnonce = MusigAggNonce::new(&secp, &[client_pub_nonce, server_pub_nonce]);

    let blinding_factor = BlindingFactor::new(&mut rand::thread_rng());

    let session = MusigSession::new_blinded(
        &secp,
        &key_agg_cache,
        aggnonce,
        msg,
        &blinding_factor
    );

    let client_keypair = KeyPair::from_secret_key(&secp, &client_seckey);

    let client_partial_sig = session.partial_sign(
        &secp,
        client_sec_nonce,
        &client_keypair,
        &key_agg_cache,
    ).unwrap();

    assert!(session.partial_verify(
        &secp,
        &key_agg_cache,
        client_partial_sig,
        client_pub_nonce,
        client_pubkey.to_owned(),
    ));

    let (key_agg_coef, negate_seckey) = session.get_keyaggcoef_and_negation_seckey(&secp, &key_agg_cache, &server_pubkey);

    let negate_seckey = match negate_seckey {
        true => 1,
        false => 0,
    };

    let payload = PartialSignatureRequestPayload {
        statechain_id,
        keyaggcoef: &hex::encode(key_agg_coef.serialize()),
        negate_seckey,
        session: &hex::encode(session.serialize()),
    };

    println!("payload signature: {:?}", payload);

    let endpoint = "http://127.0.0.1:8000";
    let path = "partial_signature";

    let client: reqwest::Client = reqwest::Client::new();
    let request = client.post(&format!("{}/{}", endpoint, path));

    let value = match request.json(&payload).send().await {
        Ok(response) => {
            let text = response.text().await.unwrap();
            text
        },
        Err(err) => {
            return Err(CError::Generic(err.to_string()));
        },
    };

    let response: PartialSignatureResponsePayload = serde_json::from_str(value.as_str()).expect(&format!("failed to parse: {}", value.as_str()));

    let mut server_partial_sig_hex = response.partial_sig.to_string();

    if server_partial_sig_hex.starts_with("0x") {
        server_partial_sig_hex = server_partial_sig_hex[2..].to_string();
    }

    let server_partial_sig_bytes = hex::decode(server_partial_sig_hex).unwrap();

    let server_partial_sig = MusigPartialSignature::from_slice(server_partial_sig_bytes.as_slice()).unwrap();

    assert!(session.partial_verify(
        &secp,
        &key_agg_cache,
        server_partial_sig,
        server_pub_nonce,
        server_pubkey.to_owned(),
    ));

    let sig = session.partial_sig_agg(&[client_partial_sig, server_partial_sig]);
    let agg_pk = key_agg_cache.agg_pk();

    assert!(agg_pk.eq(aggregated_pubkey));

    assert!(secp.verify_schnorr(&sig, &msg, &tweaked_pubkey.x_only_public_key().0).is_ok());

    println!("aggregated_pubkey: {}", aggregated_pubkey.to_string());
    println!("agg_pk: {}           ", agg_pk .to_string());
   
    Ok(sig)
}