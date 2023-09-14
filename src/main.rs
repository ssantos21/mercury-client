mod deposit;
mod key_derivation;
mod error;
mod electrum;
mod wallet;
mod transaction;

use clap::{Parser, Subcommand};
use serde::{Serialize, Deserialize};
use serde_json::json;
use sqlx::{Sqlite, migrate::MigrateDatabase, SqlitePool};


#[derive(Parser)]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create Aggregated Public Key
    Deposit { token_id: String, amount: u64 },
    /// Get a wallet balance
    GetBalance { },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {

    println!("uuid: {}", uuid::Uuid::new_v4().to_string());

    // let network = bitcoin::Network::Bitcoin;
    let network = bitcoin::Network::Signet;

    let client = electrum_client::Client::new("tcp://127.0.0.1:50001").unwrap();

    let cli = Cli::parse();

    if !Sqlite::database_exists("wallet.db").await.unwrap_or(false) {
        match Sqlite::create_database("wallet.db").await {
            Ok(_) => println!("Create db success"),
            Err(error) => panic!("error: {}", error),
        }
    }

    let pool = SqlitePool::connect("wallet.db").await.unwrap();

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .unwrap();

    match cli.command {
        Commands::Deposit { token_id, amount } => {



            let token_id = uuid::Uuid::new_v4() ; // uuid::Uuid::parse_str(&token_id).unwrap();
            deposit::execute(&pool, token_id, amount, network).await;
        },
        Commands::GetBalance {  } => {

            #[derive(Serialize, Deserialize, Debug)]
            struct Balance {
                address: String,
                balance: u64,
                unconfirmed_balance: i64,
            }

            let addresses = wallet::get_all_addresses(&pool, network).await;
            let result: Vec<Balance> = addresses.iter().map(|address| {
                let balance_res = electrum::get_address_balance(&client, &address);
                Balance {
                    address: address.to_string(),
                    balance: balance_res.confirmed,
                    unconfirmed_balance: balance_res.unconfirmed,
                }
            }).collect();

            println!("{}", serde_json::to_string_pretty(&json!(result)).unwrap());
        },
    };

    pool.close().await;

    println!("Hello, world!");
}
