use bitnames_types::{
    sdk_authorization_ed25519_dalek::{authorize, Keypair},
    sdk_types::{Address, GetAddress as _},
    AuthorizedTransaction, Output, Transaction,
};

use std::collections::HashMap;

pub fn authorize_transaction(
    keypairs: &HashMap<Address, Keypair>,
    spent_utxos: &[Output],
    transaction: Transaction,
) -> AuthorizedTransaction {
    let addresses_keypairs: Vec<(Address, &Keypair)> = spent_utxos
        .iter()
        .map(|utxo| {
            let address = utxo.get_address();
            (address, &keypairs[&address])
        })
        .collect();
    authorize(&addresses_keypairs, transaction).unwrap()
}