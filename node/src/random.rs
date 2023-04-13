use bitnames_types::{sdk_authorization_ed25519_dalek, sdk_types};
use fake::{Fake, Faker};
use sdk_authorization_ed25519_dalek::{get_address, Keypair};
use sdk_types::bitcoin::hashes::Hash as _;
use sdk_types::*;
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
