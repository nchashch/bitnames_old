use crate::authorization::*;
use crate::nameserver::*;
use crate::random::*;
use anyhow::Result;
use fake::{Fake, Faker};
use bitnames_state::*;

fn random_test() -> Result<()> {
    let env = new_env();
    let mut state = BitNamesState::new(&env)?;

    const NUM_KEYPAIRS: usize = 10;
    const NUM_DEPOSITS: usize = 2;
    const DEPOSIT_VALUE: u64 = 100;

    const NUM_INPUTS: usize = 1;

    let keypairs = random_keypairs(NUM_KEYPAIRS);
    let addresses: Vec<Address> = keypairs.keys().copied().collect();
    let utxos = random_deposits(&addresses, DEPOSIT_VALUE, NUM_DEPOSITS);
    let (inputs, spent_utxos, value_in) = random_inputs(&utxos, NUM_INPUTS);

    let key: Key = hash(&"nytimes.com").into();
    let value: Value = hash(&"151.101.193.164").into();
    let salt: u64 = Faker.fake();

    state.connect_deposits(&utxos)?;

    let commitment_transaction = {
        let commitment = blake2b_hmac(&key, salt);
        let outputs = vec![
            Output {
                address: addresses[0],
                content: Content::Value(value_in - 10),
            },
            Output {
                address: addresses[1],
                content: Content::Custom(BitNamesOutput::Commitment(commitment)),
            },
        ];
        let unsigned_transaction = Transaction { inputs, outputs };
        state.validate_transaction(&unsigned_transaction)?;
        authorize_transaction(&keypairs, &spent_utxos, unsigned_transaction)
    };
    let body = Body::new(vec![commitment_transaction.clone()], vec![]);
    state.connect_body(&body)?;

    let reveal_transaction = {
        let commitment_outpoint = OutPoint::Regular {
            txid: commitment_transaction.transaction.txid(),
            vout: 1,
        };
        let spent_utxos = vec![state.get_utxo(&commitment_outpoint)?.unwrap()];
        let inputs = vec![commitment_outpoint];
        // let wrong_key: Key = hash(&"NyTimes.com").into();
        let outputs = vec![Output {
            address: addresses[2],
            content: Content::Custom(BitNamesOutput::Reveal { salt, key }),
        }];
        let unsigned_transaction = Transaction { inputs, outputs };
        state.validate_transaction(&unsigned_transaction)?;
        authorize_transaction(&keypairs, &spent_utxos, unsigned_transaction)
    };

    let body = Body::new(vec![reveal_transaction.clone()], vec![]);
    state.connect_body(&body)?;

    let key_value_transaction = {
        let reveal_outpoint = OutPoint::Regular {
            txid: reveal_transaction.transaction.txid(),
            vout: 0,
        };
        let spent_utxos = vec![state.get_utxo(&reveal_outpoint)?.unwrap()];
        let inputs = vec![reveal_outpoint];
        let outputs = vec![Output {
            address: addresses[3],
            content: Content::Custom(BitNamesOutput::KeyValue {
                key,
                value: Some(value),
            }),
        }];
        let unsigned_transaction = Transaction { inputs, outputs };
        state.validate_transaction(&unsigned_transaction)?;
        authorize_transaction(&keypairs, &spent_utxos, unsigned_transaction)
    };

    let body = Body::new(vec![key_value_transaction], vec![]);
    state.connect_body(&body)?;

    let mut nameserver = NameServer::default();
    nameserver
        .store(&state, "nytimes.com", "151.101.193.164")
        .unwrap();

    dbg!(&nameserver);

    let name = "nytimes.com";
    println!("looking up {name}");
    let value = nameserver.lookup(&state, name).unwrap();
    println!("value = {value}");
    Ok(())
}

fn new_env() -> heed::Env {
    let env_path = std::path::Path::new("target").join("clear-database.mdb");
    let _ = std::fs::remove_dir_all(&env_path);
    std::fs::create_dir_all(&env_path).unwrap();
    let env = heed::EnvOpenOptions::new()
        .map_size(10 * 1024 * 1024) // 10MB
        .max_dbs(6)
        .open(env_path)
        .unwrap();
    env
}
