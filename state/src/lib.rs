use sdk_authorization_ed25519_dalek::verify_authorizations;
use sdk_types::{validate_body, validate_transaction};
use std::collections::HashSet;

pub use bitnames_types::*;
use heed::types::*;
use heed::{Database, RoTxn, RwTxn};

#[derive(Clone)]
pub struct BitNamesState {
    pub key_to_value: Database<SerdeBincode<Key>, SerdeBincode<Value>>,

    pub commitment_to_height: Database<SerdeBincode<Commitment>, OwnedType<u32>>,
    pub commitment_to_outpoint: Database<SerdeBincode<Commitment>, SerdeBincode<OutPoint>>,
    pub key_to_commitment: Database<SerdeBincode<Key>, SerdeBincode<Commitment>>,
    pub commitment_to_key: Database<SerdeBincode<Commitment>, SerdeBincode<Key>>,

    // Should bundle include a commitment to merkle root of withdrawal outpoints
    // it spends? Yes. Without it bundles can be ambiguous.
    //
    // Should withdrawal bundles be included in the state?
    //
    pub withdrawal_bundles: Database<SerdeBincode<bitcoin::Txid>, SerdeBincode<Vec<OutPoint>>>,
    pub pending_withdrawals: Database<SerdeBincode<OutPoint>, SerdeBincode<Output>>,
    // pub last_deposit_block: Database<OwnedType<u32>, SerdeBincode<bitcoin::BlockHash>>,
    pub utxos: Database<SerdeBincode<OutPoint>, SerdeBincode<Output>>,

    // Should headers be a part of the state?
    pub headers: Database<OwnedType<u32>, SerdeBincode<Header>>,
}

impl BitNamesState {
    pub const NUM_DBS: u32 = 9;

    pub fn new(env: &heed::Env) -> Result<Self, Error> {
        let key_to_value = env.create_database(Some("key_to_value"))?;
        let commitment_to_height = env.create_database(Some("commitment_to_height"))?;
        let commitment_to_outpoint = env.create_database(Some("commitment_to_outpoint"))?;
        let key_to_commitment = env.create_database(Some("key_to_commitment"))?;
        let commitment_to_key = env.create_database(Some("commitment_to_key"))?;
        let pending_withdrawals = env.create_database(Some("pending_withdrawals"))?;
        let withdrawal_bundles = env.create_database(Some("withdrawal_bundles"))?;
        let utxos = env.create_database(Some("utxos"))?;

        let headers: Database<OwnedType<u32>, SerdeBincode<Header>> =
            env.create_database(Some("headers"))?;

        {
            let mut wtxn = env.write_txn()?;
            if headers.is_empty(&wtxn)? {
                headers.append(&mut wtxn, &0, &Header::genesis())?;
                wtxn.commit()?;
            }
        }

        Ok(Self {
            key_to_value,
            commitment_to_height,
            commitment_to_outpoint,
            key_to_commitment,
            commitment_to_key,
            pending_withdrawals,
            withdrawal_bundles,
            headers,
            utxos,
        })
    }

    pub fn get_value(&self, rtxn: &RoTxn, key: &Key) -> Result<Option<Value>, Error> {
        Ok(self.key_to_value.get(rtxn, key)?)
    }

    pub fn get_utxo(&self, rtxn: &RoTxn, outpoint: &OutPoint) -> Result<Option<Output>, Error> {
        Ok(self.utxos.get(rtxn, outpoint)?)
    }

    fn get_utxos(&self, txn: &RoTxn, inputs: &[OutPoint]) -> Result<Vec<Output>, Error> {
        let spent_utxos: Vec<_> = inputs
            .iter()
            .map(|outpoint| {
                Ok(self.utxos.get(txn, outpoint)?.ok_or::<Error>(
                    sdk_types::Error::UtxoDoesNotExist {
                        outpoint: *outpoint,
                    }
                    .into(),
                )?)
            })
            .collect::<Result<_, Error>>()?;
        Ok(spent_utxos)
    }

    pub fn get_utxos_by_addresses(
        &self,
        txn: &RoTxn,
        addresses: &HashSet<Address>,
    ) -> Result<Vec<(OutPoint, Output)>, Error> {
        let mut utxos = vec![];
        for item in self.utxos.iter(txn)? {
            let utxo @ (_outpoint, output) = &item?;
            if addresses.contains(&output.address) {
                utxos.push(utxo.clone());
            }
        }
        Ok(utxos)
    }

    pub fn get_best_header(&self, rtxn: &RoTxn) -> Result<(u32, Header), Error> {
        Ok(self.headers.last(rtxn)?.unwrap())
    }

    pub fn validate_body(&self, rtxn: &RoTxn, body: &Body) -> Result<u64, Error> {
        let (block_height, _) = self.headers.last(rtxn)?.unwrap();
        verify_authorizations(body)?;
        let inputs: Vec<OutPoint> = body
            .transactions
            .iter()
            .flat_map(|transaction| transaction.inputs.iter())
            .copied()
            .collect();
        let spent_utxos = self.get_utxos(rtxn, &inputs)?;
        {
            let mut index = 0;
            for transaction in &body.transactions {
                let spent_utxos = &spent_utxos[index..transaction.inputs.len()];
                self.validate_transaction_pure(rtxn, spent_utxos, block_height, transaction)?;
                index += transaction.inputs.len();
            }
        }
        Ok(validate_body(spent_utxos.as_slice(), body)?)
    }

    fn validate_transaction_pure(
        &self,
        txn: &RoTxn,
        spent_utxos: &[Output],
        block_height: u32,
        transaction: &Transaction,
    ) -> Result<(), Error> {
        let spent_commitments: HashSet<Commitment> = spent_utxos
            .iter()
            .filter_map(|utxo| match utxo.content {
                Content::Custom(BitNamesOutput::Commitment(commitment)) => Some(commitment),
                _ => None,
            })
            .collect();
        let spent_keys: HashSet<Key> = spent_utxos
            .iter()
            .filter_map(|utxo| match utxo.content {
                Content::Custom(BitNamesOutput::Reveal { key, .. }) => Some(key),
                Content::Custom(BitNamesOutput::KeyValue { key, .. }) => Some(key),
                _ => None,
            })
            .collect();
        for commitment in &spent_commitments {
            let height = self.get_commitment_height(txn, commitment)?;
            if block_height - height > COMMITMENT_MAX_AGE {
                Err(BitNamesError::RevealTooLate {
                    commitment: *commitment,
                    late_by: block_height - height - COMMITMENT_MAX_AGE,
                })?;
            }
        }
        for output in &transaction.outputs {
            match output.content {
                Content::Custom(BitNamesOutput::Reveal { salt, key }) => {
                    let commitment = hmac(&key, &salt);
                    if !spent_commitments.contains(&commitment) {
                        Err(BitNamesError::InvalidNameCommitment {
                            key,
                            salt,
                            commitment,
                        })?;
                    }
                    if self.key_to_value.get(txn, &key)?.is_some() {
                        let commitment_height = self.get_commitment_height(txn, &commitment)?;
                        let prev_commitment_height = self.get_key_height(txn, &key)?;
                        if prev_commitment_height < commitment_height {
                            Err(BitNamesError::KeyAlreadyRegistered {
                                key,
                                prev_commitment_height,
                                commitment_height,
                            })?;
                        }
                    }
                }
                Content::Custom(BitNamesOutput::KeyValue { key, .. }) => {
                    if !spent_keys.contains(&key) {
                        Err(BitNamesError::InvalidKey { key })?;
                    }
                }
                Content::Custom(BitNamesOutput::Commitment(commitment)) => {
                    if self.commitment_to_outpoint.get(txn, &commitment)?.is_some() {
                        Err(BitNamesError::CommitmentAlreadyExists { commitment })?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn get_commitment_height(&self, txn: &RoTxn, commitment: &Commitment) -> Result<u32, Error> {
        Ok(self.commitment_to_height.get(txn, commitment)?.ok_or(
            BitNamesError::CommitmentNotFound {
                commitment: *commitment,
            },
        )?)
    }

    fn get_key_height(&self, txn: &RoTxn, key: &Key) -> Result<u32, Error> {
        let commitment = self
            .key_to_commitment
            .get(txn, key)?
            .ok_or(BitNamesError::KeyNotFound { key: *key })?;
        Ok(self
            .commitment_to_height
            .get(txn, &commitment)?
            .ok_or(BitNamesError::CommitmentNotFound { commitment })?)
    }

    pub fn validate_transaction(
        &self,
        rtxn: &RoTxn,
        transaction: &Transaction,
    ) -> Result<u64, Error> {
        let spent_utxos = self.get_utxos(rtxn, &transaction.inputs)?;
        // TODO: Add an error for this case, don't unwrap.
        let (best_block_height, _) = self.headers.last(rtxn)?.unwrap();
        // Will this transaction be valid, if included in next block?
        self.validate_transaction_pure(rtxn, &spent_utxos, best_block_height + 1, transaction)?;
        Ok(validate_transaction(&spent_utxos, transaction)?)
    }

    pub fn validate_block(
        &self,
        rtxn: &RoTxn,
        header: &Header,
        body: &Body,
        // TODO: Validate two way peg data.
        two_way_peg_data: &TwoWayPegData,
    ) -> Result<(), Error> {
        let (_, prev_header) = self.headers.last(rtxn)?.unwrap();
        let prev_side_block_hash = prev_header.block_hash();
        if header.prev_side_block_hash != prev_side_block_hash {
            Err(HeaderError::InvalidPrevSideBlockHash)?;
        }
        let merkle_root = body.compute_merkle_root();
        if header.merkle_root != merkle_root {
            Err(HeaderError::InvalidMerkleRoot)?;
        }
        self.validate_body(rtxn, body)?;
        Ok(())
    }

    pub fn disconnect_block(
        &self,
        _wtxn: &mut RwTxn,
        _disconnect_data: &DisconnectData,
    ) -> Result<(), Error> {
        todo!();
    }

    pub fn connect_block(
        &self,
        wtxn: &mut RwTxn,
        header: &Header,
        body: &Body,
        two_way_peg_data: &TwoWayPegData,
    ) -> Result<(), Error> {
        // Connect header.
        let (block_height, _) = self.headers.last(wtxn)?.unwrap();
        self.headers
            .append(wtxn, &(block_height + 1), &header.clone())?;

        // Connect deposits.
        for (outpoint, deposit) in &two_way_peg_data.deposits {
            self.utxos.put(wtxn, outpoint, deposit)?;
        }
        // Move pending withdrawals out of the utxo set.
        for (bundle, outpoints) in &two_way_peg_data.pending_bundles {
            self.withdrawal_bundles.put(wtxn, bundle, outpoints)?;
            for outpoint in outpoints {
                let output = self.utxos.get(wtxn, outpoint)?.unwrap();
                self.pending_withdrawals.put(wtxn, outpoint, &output)?;
                self.utxos.delete(wtxn, outpoint)?;
            }
        }
        // Remove spent withdrawals from the pending withdrawals set.
        for bundle in &two_way_peg_data.failed_bundles {
            let spent_withdrawals = self.withdrawal_bundles.get(wtxn, bundle)?.unwrap();
            for outpoint in &spent_withdrawals {
                self.pending_withdrawals.delete(wtxn, outpoint)?;
            }
            self.withdrawal_bundles.delete(wtxn, bundle)?;
        }
        // Move failed withdrawals back into the utxo set.
        for bundle in &two_way_peg_data.failed_bundles {
            let failed_withdrawals = self.withdrawal_bundles.get(wtxn, bundle)?.unwrap();
            for outpoint in &failed_withdrawals {
                let output = self.pending_withdrawals.get(wtxn, outpoint)?.unwrap();
                self.utxos.put(wtxn, outpoint, &output)?;
                self.pending_withdrawals.delete(wtxn, outpoint)?;
            }
        }

        // Connect body.
        let block_height = block_height + 1;
        for transaction in &body.transactions {
            let spent_utxos = self.get_utxos(wtxn, &transaction.inputs)?;
            // Delete spent utxos from utxo set.
            for (input, output) in transaction.inputs.iter().zip(spent_utxos.iter()) {
                // Update BitNames specific caches.
                match &output.content {
                    Content::Custom(BitNamesOutput::KeyValue { key, .. }) => {
                        self.key_to_value.delete(wtxn, key)?;
                    }
                    Content::Custom(BitNamesOutput::Commitment(commitment)) => {
                        self.commitment_to_key.delete(wtxn, commitment)?;
                    }
                    _ => {}
                }
                self.utxos.delete(wtxn, input)?;
            }
            let txid = transaction.txid();
            for vout in 0..transaction.outputs.len() {
                let outpoint = OutPoint::Regular {
                    txid,
                    vout: vout as u32,
                };
                let output = transaction.outputs[vout].clone();
                // Update BitNames specific caches.
                match &output.content {
                    Content::Custom(BitNamesOutput::KeyValue { key, value }) => {
                        self.key_to_value.put(wtxn, key, value)?;
                    }
                    Content::Custom(BitNamesOutput::Reveal { key, salt }) => {
                        let commitment = hmac(key, salt);
                        self.key_to_commitment.put(wtxn, key, &commitment)?;
                        self.commitment_to_key.put(wtxn, &commitment, key)?;
                        self.key_to_value.put(wtxn, key, &Value::from([0; 32]))?;
                    }
                    Content::Custom(BitNamesOutput::Commitment(commitment)) => {
                        self.commitment_to_height
                            .put(wtxn, commitment, &block_height)?;
                        self.commitment_to_outpoint
                            .put(wtxn, commitment, &outpoint)?;
                    }
                    _ => {}
                }
                // Update utxo
                self.utxos.put(wtxn, &outpoint, &output)?;
            }
        }
        let mut expired_commitments: Vec<Commitment> = vec![];
        for item in self.commitment_to_height.iter(wtxn)? {
            let (commitment, height) = item?;
            if block_height - height > COMMITMENT_MAX_AGE {
                expired_commitments.push(commitment);
            }
        }
        for commitment in &expired_commitments {
            if let Some(key) = self.commitment_to_key.get(wtxn, commitment)? {
                self.key_to_commitment.delete(wtxn, &key)?;
                self.commitment_to_key.delete(wtxn, commitment)?;
            }
            let outpoint = self.commitment_to_outpoint.get(wtxn, commitment)?.ok_or(
                BitNamesError::CommitmentNotFound {
                    commitment: *commitment,
                },
            )?;
            self.utxos.delete(wtxn, &outpoint)?;
            self.commitment_to_height.delete(wtxn, commitment)?;
            self.commitment_to_outpoint.delete(wtxn, commitment)?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("authorization error")]
    Authorization(#[from] sdk_authorization_ed25519_dalek::Error),
    #[error("sdk error")]
    Sdk(#[from] sdk_types::Error),
    #[error("bitnames error")]
    BitNames(#[from] BitNamesError),
    #[error("header")]
    Header(#[from] HeaderError),
    #[error("heed error")]
    Heed(#[from] heed::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum HeaderError {
    #[error("invalid merkle root")]
    InvalidMerkleRoot,
    #[error("invalid previous side block hash")]
    InvalidPrevSideBlockHash,
}

const COMMITMENT_MAX_AGE: u32 = 10;
#[derive(Debug, thiserror::Error)]
pub enum BitNamesError {
    #[error("invalid name commitment")]
    InvalidNameCommitment {
        key: Key,
        salt: Salt,
        commitment: Commitment,
    },
    #[error("key {key} was already registered with an older commitment: prev commitment height {prev_commitment_height} < commitment height {commitment_height}")]
    KeyAlreadyRegistered {
        key: Key,
        prev_commitment_height: u32,
        commitment_height: u32,
    },
    #[error("commitment {commitment} not found")]
    CommitmentNotFound { commitment: Commitment },
    #[error("commitment {commitment} already exists")]
    CommitmentAlreadyExists { commitment: Commitment },
    #[error("key {key} not found")]
    KeyNotFound { key: Key },
    #[error("commitment {commitment} is late by {late_by}")]
    RevealTooLate {
        commitment: Commitment,
        late_by: u32,
    },
    #[error("invalid key {key}")]
    InvalidKey { key: Key },
}
