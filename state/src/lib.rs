use sdk_authorization_ed25519_dalek::verify_authorizations;
use sdk_types::{validate_body, validate_transaction};
use std::collections::{HashMap, HashSet};

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

    // TODO: Include commitment to spent inputs in withdrawal bundle, without it
    // there is ambiguity
    //
    // TODO: Get rid of redundant data for withdrawal bundles
    //
    // TODO: Lock withdrawals independently of mainchain, make locking of
    // withdrawals deterministic.
    pub last_withdrawal_bundle: Database<OwnedType<u32>, SerdeBincode<WithdrawalBundle>>,
    pub last_withdrawal_bundle_failure_height: Database<OwnedType<u32>, OwnedType<u32>>,
    pub last_deposit_block: Database<OwnedType<u32>, SerdeBincode<bitcoin::BlockHash>>,

    pub utxos: Database<SerdeBincode<OutPoint>, SerdeBincode<Output>>,
    // Should headers be a part of the state?
    pub headers: Database<OwnedType<u32>, SerdeBincode<Header>>,
}

impl BitNamesState {
    pub const NUM_DBS: u32 = 10;
    pub const WITHDRAWAL_BUNDLE_FAILURE_GAP: u32 = 100;

    pub fn new(env: &heed::Env) -> Result<Self, Error> {
        let key_to_value = env.create_database(Some("key_to_value"))?;
        let commitment_to_height = env.create_database(Some("commitment_to_height"))?;
        let commitment_to_outpoint = env.create_database(Some("commitment_to_outpoint"))?;
        let key_to_commitment = env.create_database(Some("key_to_commitment"))?;
        let commitment_to_key = env.create_database(Some("commitment_to_key"))?;

        let last_withdrawal_bundle = env.create_database(Some("last_withdrawal_bundle"))?;

        let last_withdrawal_bundle_failure_height =
            env.create_database(Some("last_withdrawal_bundle_failure_height"))?;
        let last_deposit_block = env.create_database(Some("last_deposit_block"))?;

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
            last_withdrawal_bundle,
            last_withdrawal_bundle_failure_height,
            last_deposit_block,
            utxos,
            headers,
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

    pub fn get_pending_withdrawal_bundle(
        &self,
        txn: &RoTxn,
    ) -> Result<Option<WithdrawalBundle>, Error> {
        Ok(self.last_withdrawal_bundle.get(txn, &0)?)
    }

    fn collect_withdrawal_bundle(&self, txn: &RoTxn) -> Result<Option<WithdrawalBundle>, Error> {
        use bitcoin::blockdata::{opcodes, script};
        // Weight of a bundle with 0 outputs.
        const BUNDLE_0_WEIGHT: usize = 504;
        // Weight of a single output.
        const OUTPUT_WEIGHT: usize = 128;
        // Turns out to be 3121.
        const MAX_BUNDLE_OUTPUTS: usize =
            (bitcoin::policy::MAX_STANDARD_TX_WEIGHT as usize - BUNDLE_0_WEIGHT) / OUTPUT_WEIGHT;

        // Aggregate all outputs by destination.
        // destination -> (value, mainchain fee, spent_utxos)
        let mut address_to_aggregated_withdrawal =
            HashMap::<bitcoin::Address, AggregatedWithdrawal>::new();
        for item in self.utxos.iter(txn)? {
            let (outpoint, output) = item?;
            if let Content::Withdrawal {
                value,
                ref main_address,
                main_fee,
            } = output.content
            {
                let aggregated = address_to_aggregated_withdrawal
                    .entry(main_address.clone())
                    .or_insert(AggregatedWithdrawal {
                        spent_utxos: HashMap::new(),
                        main_address: main_address.clone(),
                        value: 0,
                        main_fee: 0,
                    });
                // Add up all values.
                aggregated.value += value;
                // Set maximum mainchain fee.
                if main_fee > aggregated.main_fee {
                    aggregated.main_fee = main_fee;
                }
                aggregated.spent_utxos.insert(outpoint, output);
            }
        }
        if address_to_aggregated_withdrawal.is_empty() {
            return Ok(None);
        }
        let mut aggregated_withdrawals: Vec<_> =
            address_to_aggregated_withdrawal.into_values().collect();
        aggregated_withdrawals.sort_by_key(|a| std::cmp::Reverse(a.clone()));
        let mut fee = 0;
        let mut spent_utxos = HashMap::<OutPoint, Output>::new();
        let mut bundle_outputs = vec![];
        for aggregated in &aggregated_withdrawals {
            if bundle_outputs.len() > MAX_BUNDLE_OUTPUTS {
                break;
            }
            let bundle_output = bitcoin::TxOut {
                value: aggregated.value,
                script_pubkey: aggregated.main_address.script_pubkey(),
            };
            spent_utxos.extend(aggregated.spent_utxos.clone());
            bundle_outputs.push(bundle_output);
            fee += aggregated.main_fee;
        }
        let txin = bitcoin::TxIn {
            script_sig: script::Builder::new()
                // OP_FALSE == OP_0
                .push_opcode(opcodes::OP_FALSE)
                .into_script(),
            ..bitcoin::TxIn::default()
        };
        // Create return dest output.
        // The destination string for the change of a WT^
        const SIDECHAIN_WTPRIME_RETURN_DEST: &[u8] = b"D";
        let script = script::Builder::new()
            .push_opcode(opcodes::all::OP_RETURN)
            .push_slice(SIDECHAIN_WTPRIME_RETURN_DEST)
            .into_script();
        let return_dest_txout = bitcoin::TxOut {
            value: 0,
            script_pubkey: script,
        };
        // Create mainchain fee output.
        let script = script::Builder::new()
            .push_opcode(opcodes::all::OP_RETURN)
            .push_slice(fee.to_le_bytes().as_ref())
            .into_script();
        let mainchain_fee_txout = bitcoin::TxOut {
            value: 0,
            script_pubkey: script,
        };
        // Create inputs commitment.
        let inputs: Vec<OutPoint> = spent_utxos.keys().copied().collect();
        let commitment = hash(&inputs);
        let script = script::Builder::new()
            .push_opcode(opcodes::all::OP_RETURN)
            .push_slice(&commitment)
            .into_script();
        let inputs_commitment_txout = bitcoin::TxOut {
            value: 0,
            script_pubkey: script,
        };
        let transaction = bitcoin::Transaction {
            version: 2,
            lock_time: bitcoin::PackedLockTime(0),
            input: vec![txin],
            output: [
                vec![
                    return_dest_txout,
                    mainchain_fee_txout,
                    inputs_commitment_txout,
                ],
                bundle_outputs,
            ]
            .concat(),
        };
        if transaction.weight() > bitcoin::policy::MAX_STANDARD_TX_WEIGHT as usize {
            Err(BitNamesError::BundleTooHeavy {
                weight: transaction.weight(),
                max_weight: bitcoin::policy::MAX_STANDARD_TX_WEIGHT as usize,
            })?;
        }
        Ok(Some(WithdrawalBundle {
            spent_utxos,
            transaction,
        }))
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

    pub fn get_last_deposit_block_hash(
        &self,
        rtxn: &RoTxn,
    ) -> Result<Option<bitcoin::BlockHash>, Error> {
        Ok(self.last_deposit_block.get(&rtxn, &0)?)
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

        // Handle deposits.
        if let Some(deposit_block_hash) = two_way_peg_data.deposit_block_hash {
            self.last_deposit_block.put(wtxn, &0, &deposit_block_hash)?;
        }
        for (outpoint, deposit) in &two_way_peg_data.deposits {
            self.utxos.put(wtxn, outpoint, deposit)?;
        }

        // Handle withdrawals.
        let last_withdrawal_bundle_failure_height = self
            .last_withdrawal_bundle_failure_height
            .get(wtxn, &0)?
            .unwrap_or(0);
        if (block_height + 1) - last_withdrawal_bundle_failure_height
            > Self::WITHDRAWAL_BUNDLE_FAILURE_GAP
        {
            if let Some(bundle) = self.collect_withdrawal_bundle(wtxn)? {
                for outpoint in bundle.spent_utxos.keys() {
                    self.utxos.delete(wtxn, outpoint)?;
                }
                self.last_withdrawal_bundle.put(wtxn, &0, &bundle)?;
            }
        }
        for (txid, status) in &two_way_peg_data.bundle_statuses {
            if let Some(bundle) = self.last_withdrawal_bundle.get(wtxn, &0)? {
                if bundle.transaction.txid() != *txid {
                    continue;
                }
                match status {
                    WithdrawalBundleStatus::Failed => {
                        self.last_withdrawal_bundle_failure_height.put(
                            wtxn,
                            &0,
                            &(block_height + 1),
                        )?;
                        for (outpoint, output) in &bundle.spent_utxos {
                            self.utxos.put(wtxn, outpoint, output)?;
                        }
                    }
                    WithdrawalBundleStatus::Confirmed => {
                        self.last_withdrawal_bundle.delete(wtxn, &0)?;
                    }
                }
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
                // Update utxos.
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
    #[error("bundle too heavy {weight} > {max_weight}")]
    BundleTooHeavy { weight: usize, max_weight: usize },
}
