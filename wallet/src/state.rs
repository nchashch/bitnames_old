use bitnames_types::{sdk_types::GetValue as _, *};
use heed::types::*;
use heed::{Database, RoTxn, RwTxn};

pub struct State {
    key_to_outpoint: Database<SerdeBincode<Key>, SerdeBincode<OutPoint>>,
    outpoint_to_address: Database<SerdeBincode<OutPoint>, SerdeBincode<Address>>,
    utxos: Database<SerdeBincode<OutPoint>, SerdeBincode<Output>>,
}

impl State {
    pub const NUM_DBS: u32 = 3;

    pub fn new(env: &heed::Env) -> Result<Self, Error> {
        let utxos = env.create_database(Some("utxos"))?;
        let outpoint_to_address = env.create_database(Some("outpoint_to_address"))?;
        let key_to_outpoint = env.create_database(Some("key_to_outpoint"))?;
        Ok(Self {
            outpoint_to_address,
            key_to_outpoint,
            utxos,
        })
    }

    pub fn get_addresses(&self, txn: &RoTxn, inputs: &[OutPoint]) -> Result<Vec<Address>, Error> {
        let addresses: Vec<_> = inputs
            .iter()
            .map(|outpoint| {
                Ok(self.outpoint_to_address.get(txn, outpoint)?.ok_or(
                    Error::NoAddressForOutPoint {
                        outpoint: *outpoint,
                    },
                )?)
            })
            .collect::<Result<_, Error>>()?;
        Ok(addresses)
    }

    pub fn get_outpoint_address(&self, txn: &RoTxn, outpoint: &OutPoint) -> Result<Address, Error> {
        Ok(self
            .outpoint_to_address
            .get(txn, outpoint)?
            .ok_or(Error::NoAddressForOutPoint {
                outpoint: *outpoint,
            })?)
    }

    pub fn get_key_outpoint(&self, txn: &RoTxn, key: &Key) -> Result<OutPoint, Error> {
        self.key_to_outpoint
            .get(txn, key)?
            .ok_or(Error::NoOutPointForKey { key: *key })
    }

    pub fn set_key_outpoint(
        &self,
        txn: &mut RwTxn,
        key: &Key,
        outpoint: &OutPoint,
    ) -> Result<(), Error> {
        self.key_to_outpoint.put(txn, key, outpoint)?;
        Ok(())
    }

    pub fn get_balance(&self, txn: &RoTxn) -> Result<u64, Error> {
        let mut balance = 0;
        for item in self.utxos.iter(txn)? {
            let (_, output) = &item?;
            balance += output.get_value();
        }
        Ok(balance)
    }

    pub fn add_utxos(&self, txn: &mut RwTxn, utxos: &[(OutPoint, Output)]) -> Result<(), Error> {
        for (outpoint, output) in utxos {
            self.utxos.put(txn, outpoint, output)?;
        }
        Ok(())
    }

    pub fn connect(
        &self,
        txn: &mut RwTxn,
        transaction: &AuthorizedTransaction,
    ) -> Result<(), Error> {
        let transaction = &transaction.transaction;
        let txid = transaction.txid();
        for outpoint in &transaction.inputs {
            self.utxos.delete(txn, outpoint)?;
        }
        for (vout, output) in transaction.outputs.iter().enumerate() {
            let outpoint = OutPoint::Regular {
                txid,
                vout: vout as u32,
            };
            self.outpoint_to_address
                .put(txn, &outpoint, &output.address)?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("heed error")]
    Heed(#[from] heed::Error),
    #[error("no address for outpoint {outpoint}")]
    NoAddressForOutPoint { outpoint: OutPoint },
    #[error("no outpoint for key {key}")]
    NoOutPointForKey { key: Key },
}
