use std::str::FromStr;

use bitcoin::{Network, Address};
use sqlx::{Sqlite, Row};

pub async fn get_all_addresses(pool: &sqlx::Pool<Sqlite>, network: Network) -> Vec::<Address>{
    let query = "SELECT p2tr_agg_address FROM signer_data";

    let rows = sqlx::query(query)
        .fetch_all(pool)
        .await
        .unwrap();

    let mut addresses = Vec::<Address>::new();

    for row in rows {

        let p2tr_agg_address = row.get::<String, _>("p2tr_agg_address");
        let address = Address::from_str(&p2tr_agg_address).unwrap().require_network(network).unwrap();
        addresses.push(address);
    }

    addresses
}