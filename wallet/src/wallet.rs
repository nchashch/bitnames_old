use crate::signer::Signer;
use crate::state::State;
use bitnames_api::bit_names_client::BitNamesClient;
use bitnames_api::*;
use bitnames_types::sdk_types::GetValue as _;
use bitnames_types::*;
use heed::{RoTxn, RwTxn};
use sdk_authorization_ed25519_dalek::Authorization;

pub struct Wallet {
    env: heed::Env,
    state: State,
    signer: Signer,
    client: BitNamesClient<bitnames_api::tonic::transport::Channel>,
}

pub struct Config {
    pub db_path: std::path::PathBuf,
}

impl Wallet {
    pub async fn new(seed: [u8; 64], config: &Config) -> Result<Self, Error> {
        std::fs::create_dir_all(&config.db_path)?;
        let env = heed::EnvOpenOptions::new()
            .map_size(10 * 1024 * 1024) // 10MB
            .max_dbs(State::NUM_DBS + Signer::NUM_DBS)
            .open(&config.db_path)
            .unwrap();
        let state = State::new(&env)?;
        let signer = Signer::new(seed, &env)?;
        let client = BitNamesClient::connect("http://[::1]:50051").await?;
        Ok(Self {
            env,
            state,
            signer,
            client,
        })
    }

    pub fn get_balance(&self) -> Result<u64, Error> {
        let rtxn = self.env.read_txn()?;
        Ok(self.state.get_balance(&rtxn)?)
    }

    pub async fn update(&mut self) -> Result<(), Error> {
        let mut wtxn = self.env.write_txn()?;
        let addresses = self
            .signer
            .get_addresses(&wtxn)?
            .iter()
            .map(|address| Ok(bincode::serialize(address)?))
            .collect::<Result<_, Error>>()?;
        let request = tonic::Request::new(GetUtxosByAddressesRequest { addresses });
        let response = self.client.get_utxos_by_addresses(request).await?;
        let utxos: Vec<(OutPoint, Output)> = response
            .into_inner()
            .utxos
            .iter()
            .map(|utxo| Ok(bincode::deserialize(utxo)?))
            .collect::<Result<_, Error>>()?;
        let mut balance = 0;
        for (_, output) in &utxos {
            balance += output.get_value();
        }
        dbg!(balance);
        self.state.add_utxos(&mut wtxn, &utxos)?;
        wtxn.commit()?;
        Ok(())
    }

    pub fn authorize(
        &self,
        txn: &RoTxn,
        transaction: Transaction,
    ) -> Result<AuthorizedTransaction, Error> {
        let addresses = self.state.get_addresses(txn, &transaction.inputs)?;
        dbg!(&addresses);
        let message = bincode::serialize(&transaction)?;
        let authorizations: Vec<_> = addresses
            .iter()
            .map(|address| {
                let (public_key, signature) = self.signer.sign(txn, address, &message)?;
                Ok(Authorization {
                    public_key,
                    signature,
                })
            })
            .collect::<Result<_, Error>>()?;
        let transaction = AuthorizedTransaction {
            authorizations,
            transaction,
        };
        Ok(transaction)
    }

    pub fn get_new_address(&self) -> Result<Address, Error> {
        let mut wtxn = self.env.write_txn()?;
        let address = self.signer.get_new_address(&mut wtxn)?;
        wtxn.commit()?;
        Ok(address)
    }

    pub async fn commit(&mut self, key: &Key) -> Result<(), Error> {
        let mut wtxn = self.env.write_txn()?;
        let address = self.signer.get_new_address(&mut wtxn)?;
        let salt = self.signer.salt(&wtxn, &address, key)?;
        let transaction = TransactionBuilder::default()
            .commit(address, *key, salt)
            .build();
        let outpoint = OutPoint::Regular {
            txid: transaction.txid(),
            vout: 0,
        };
        self.state.set_key_outpoint(&mut wtxn, key, &outpoint)?;
        let transaction = self.authorize(&wtxn, transaction)?;
        //dbg!(&transaction);
        self.state.connect(&mut wtxn, &transaction)?;
        let transaction = bincode::serialize(&transaction)?;
        let request = tonic::Request::new(SubmitTransactionRequest { transaction });
        let response = self.client.submit_transaction(request).await?;
        dbg!(response);
        wtxn.commit()?;
        Ok(())
    }

    pub async fn claim(&mut self, key: &Key) -> Result<(), Error> {
        let mut wtxn = self.env.write_txn()?;
        let outpoint = self.state.get_key_outpoint(&wtxn, key)?;
        dbg!(&outpoint);
        let address = self.state.get_outpoint_address(&wtxn, &outpoint)?;
        let salt = self.signer.salt(&wtxn, &address, key)?;
        let transaction = TransactionBuilder::default()
            .spend(outpoint)
            .reveal(address, *key, salt)
            .build();
        let outpoint = OutPoint::Regular {
            txid: transaction.txid(),
            vout: 0,
        };
        self.state.set_key_outpoint(&mut wtxn, key, &outpoint)?;
        let transaction = self.authorize(&wtxn, transaction)?;
        self.state.connect(&mut wtxn, &transaction)?;
        let transaction = bincode::serialize(&transaction)?;
        let request = tonic::Request::new(SubmitTransactionRequest { transaction });
        let response = self.client.submit_transaction(request).await?;
        dbg!(response);
        wtxn.commit()?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Key>, Error> {
        todo!();
    }

    pub async fn set(&mut self, key: &Key, value: &Value) -> Result<(), Error> {
        let mut wtxn = self.env.write_txn()?;
        let outpoint = self.state.get_key_outpoint(&wtxn, key)?;
        let address = self.state.get_outpoint_address(&wtxn, &outpoint)?;
        let transaction = TransactionBuilder::default()
            .spend(outpoint)
            .set(address, *key, *value)
            .build();
        let outpoint = OutPoint::Regular {
            txid: transaction.txid(),
            vout: 0,
        };
        self.state.set_key_outpoint(&mut wtxn, key, &outpoint)?;
        let transaction = self.authorize(&wtxn, transaction)?;
        self.state.connect(&mut wtxn, &transaction)?;
        let transaction = bincode::serialize(&transaction)?;
        let request = tonic::Request::new(SubmitTransactionRequest { transaction });
        let response = self.client.submit_transaction(request).await?;
        dbg!(response);
        wtxn.commit()?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("state error")]
    State(#[from] crate::state::Error),
    #[error("signer error")]
    Signer(#[from] crate::signer::Error),
    #[error("heed error")]
    Heed(#[from] heed::Error),
    #[error("io error")]
    Io(#[from] std::io::Error),
    #[error("tonic error")]
    Tonic(#[from] bitnames_api::tonic::transport::Error),
    #[error("binvode error")]
    Bincode(#[from] bincode::Error),
    #[error("rpc error")]
    Rpc(#[from] tonic::Status),
}
