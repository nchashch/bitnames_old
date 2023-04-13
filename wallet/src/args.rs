use bitnames_types::{bitcoin, sdk_types};
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[clap(author, version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Update,
    Balance,
    #[command(subcommand)]
    Name(Name),
    #[command(subcommand)]
    Address(Address),
    // #[command(subcommand)]
    // Funds(Funds),
}

#[derive(Debug, Subcommand)]
pub enum Name {
    Commit { name: String },
    Claim { name: String },
    Set { name: String, value: String },
}

#[derive(Debug, Subcommand)]
pub enum Address {
    Get,
}

#[derive(Debug, Subcommand)]
pub enum Funds {
    Send {
        to: sdk_types::Address,
        amount: bitcoin::Amount,
    },
    Withdraw {
        to: bitcoin::Address,
        amount: bitcoin::Amount,
    },
}
