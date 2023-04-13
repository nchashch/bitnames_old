use bitnames_types::bitcoin;
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[clap(author, version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Blind merged mining commands.
    #[command(subcommand)]
    Bmm(Bmm),
}

#[derive(Debug, Subcommand)]
pub enum Bmm {
    /// Create a bmm request.
    Attempt {
        /// Amount to be paid to mainchain miners for including the bmm commitment.
        #[arg(value_parser = btc_amount_parser)]
        amount: bitcoin::Amount,
    },
    /// Check if the bmm request was successful, and then connect the block.
    Confirm,
    /// Create a bmm request, generate a mainchain block (only works in regtest mode), confirm bmm.
    Generate {
        /// Amount to be paid to mainchain miners for including the bmm commitment.
        #[arg(value_parser = btc_amount_parser)]
        amount: bitcoin::Amount,
    },
}

fn btc_amount_parser(s: &str) -> Result<bitcoin::Amount, bitcoin::util::amount::ParseAmountError> {
    bitcoin::Amount::from_str_in(s, bitcoin::Denomination::Bitcoin)
}
