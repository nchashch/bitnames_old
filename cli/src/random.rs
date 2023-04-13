use bitnames_types::{
    bitcoin::hashes::Hash as _,
    sdk_authorization_ed25519_dalek::{get_address, Keypair},
    sdk_types::*,
};

use crate::authorization::*;
use anyhow::Result;
use fake::{Fake, Faker};
use std::collections::HashMap;

pub fn random_keypairs(num_keypairs: usize) -> HashMap<Address, Keypair> {
    let mut csprng = rand::thread_rng();
    (0..num_keypairs)
        .map(|_| {
            let keypair = Keypair::generate(&mut csprng);
            (get_address(&keypair.public), keypair)
        })
        .collect()
}

pub fn random_deposits<C>(
    addresses: &[Address],
    value: u64,
    num_deposits: usize,
) -> HashMap<OutPoint, Output<C>> {
    (0..num_deposits)
        .map(|_| {
            let outpoint = {
                let txid: [u8; 32] = Faker.fake();
                let txid = bitcoin::Txid::from_inner(txid);
                let vout: u32 = (0..4).fake();
                OutPoint::Deposit(bitcoin::OutPoint { txid, vout })
            };
            let output = {
                let index: usize = (0..addresses.len()).fake();
                Output {
                    address: addresses[index],
                    content: Content::Value(value),
                }
            };
            (outpoint, output)
        })
        .collect()
}

pub fn random_inputs<C: Sized + GetValue + Clone>(
    utxos: &HashMap<OutPoint, Output<C>>,
    num_inputs: usize,
) -> (Vec<OutPoint>, Vec<Output<C>>, u64) {
    let outpoints: Vec<OutPoint> = utxos.keys().copied().collect();
    let inputs: Vec<OutPoint> = outpoints.iter().copied().take(num_inputs).collect();
    let spent_utxos: Vec<Output<C>> = inputs.iter().map(|input| utxos[input].clone()).collect();
    let value_in: u64 = spent_utxos.iter().map(|utxo| utxo.get_value()).sum();
    (inputs, spent_utxos, value_in)
}

fn create_transaction() -> Result<bitnames_types::AuthorizedTransaction> {
    const NUM_KEYPAIRS: usize = 10;
    const NUM_DEPOSITS: usize = 2;
    const DEPOSIT_VALUE: u64 = 100;

    const NUM_INPUTS: usize = 1;

    let key: bitnames_types::Key = hash(&"nytimes.com").into();
    let value: bitnames_types::Value = hash(&"151.101.193.164").into();
    let salt: [u8; 32] = Faker.fake();
    let salt: bitnames_types::Salt = salt.into();

    let keypairs = random_keypairs(NUM_KEYPAIRS);
    let addresses: Vec<Address> = keypairs.keys().copied().collect();
    let commitment_transaction = {
        let commitment = bitnames_types::hmac(&key, &salt);
        let outputs = vec![Output {
            address: addresses[0],
            content: Content::Custom(bitnames_types::BitNamesOutput::Commitment(commitment)),
        }];
        let unsigned_transaction = Transaction {
            inputs: vec![],
            outputs,
        };
        authorize_transaction(&keypairs, &[], unsigned_transaction)
    };
    Ok(commitment_transaction)
}
