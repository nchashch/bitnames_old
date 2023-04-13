mod args;
mod signer;
mod state;
mod wallet;

use anyhow::Result;
use args::{Address, Cli, Command, Name};
use bitcoin::hashes::{sha256, Hash};
use bitnames_types::*;
use clap::Parser;
use wallet::Wallet;

/// Format `str_dest` with the proper `s{sidechain_number}_` prefix and a
/// checksum postfix for calling createsidechaindeposit on mainchain.
pub fn format_deposit_address(str_dest: &str) -> String {
    let this_sidechain = 0;
    let deposit_address: String = format!("s{}_{}_", this_sidechain, str_dest);
    let hash = sha256::Hash::hash(deposit_address.as_bytes()).to_string();
    let hash: String = hash[..6].into();
    format!("{}{}", deposit_address, hash)
}

fn address(command: Address, wallet: &mut Wallet) -> Result<()> {
    match command {
        Address::Get => {
            let address = wallet.get_new_address()?;
            let address = format_deposit_address(&format!("{}", address));
            println!("{}", address);
        }
    }
    Ok(())
}

async fn name(command: Name, wallet: &mut Wallet) -> Result<()> {
    match command {
        Name::Commit { name } => {
            let key: Key = sdk_types::hash(&name).into();
            wallet.commit(&key).await?;
        }
        Name::Claim { name } => {
            let key: Key = sdk_types::hash(&name).into();
            wallet.claim(&key).await?;
        }
        Name::Set { name, value } => {
            let key: Key = sdk_types::hash(&name).into();
            let value: Value = sdk_types::hash(&value).into();
            wallet.set(&key, &value).await?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let mnemonic = bip39::Mnemonic::parse(
        "sudden member wrestle fruit apology woman glow crop play what target supreme",
    )?;
    let seed = mnemonic.to_seed("");

    let config = wallet::Config {
        db_path: "./target/wallet.mdb/".into(),
    };

    let mut wallet = Wallet::new(seed, &config).await?;

    let args = Cli::parse();
    match args.command {
        Command::Name(command) => name(command, &mut wallet).await?,
        Command::Address(command) => address(command, &mut wallet)?,
        Command::Update => wallet.update().await?,
        Command::Balance => {
            let balance = wallet.get_balance()?;
            let balance = bitcoin::Amount::from_sat(balance);
            println!("{}", balance);
        }
    }
    Ok(())
}
