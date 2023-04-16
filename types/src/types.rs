use crate::hashes::*;
use bitcoin::hashes::Hash as _;
use sdk_authorization_ed25519_dalek::Authorization;
use sdk_types::*;
pub use sdk_types::{Address, BlockHash, Content, MerkleRoot, OutPoint, Txid};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BitNamesOutput {
    Commitment(Commitment),
    Reveal { salt: Salt, key: Key },
    KeyValue { key: Key, value: Value },
}

pub type Output = sdk_types::Output<BitNamesOutput>;
pub type Transaction = sdk_types::Transaction<BitNamesOutput>;
pub type AuthorizedTransaction = sdk_types::AuthorizedTransaction<Authorization, BitNamesOutput>;
pub type Body = sdk_types::Body<Authorization, BitNamesOutput>;

impl GetValue for BitNamesOutput {
    #[inline(always)]
    fn get_value(&self) -> u64 {
        0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub prev_side_block_hash: BlockHash,
    pub prev_main_block_hash: bitcoin::BlockHash,
    pub merkle_root: MerkleRoot,
}

impl Header {
    pub fn block_hash(&self) -> BlockHash {
        hash(self).into()
    }

    pub fn genesis() -> Self {
        Self {
            prev_main_block_hash: bitcoin::BlockHash::from_inner([0; 32]),
            prev_side_block_hash: Default::default(),
            merkle_root: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WithdrawalBundleStatus {
    Failed,
    Confirmed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawalBundle {
    pub spent_utxos: HashMap<OutPoint, Output>,
    pub transaction: bitcoin::Transaction,
}

#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct TwoWayPegData {
    pub deposits: HashMap<OutPoint, Output>,
    pub deposit_block_hash: Option<bitcoin::BlockHash>,
    pub bundle_statuses: HashMap<bitcoin::Txid, WithdrawalBundleStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisconnectData {
    pub spent_utxos: HashMap<OutPoint, Output>,
    pub deposits: Vec<OutPoint>,
    pub pending_bundles: Vec<bitcoin::Txid>,
    pub spent_bundles: HashMap<bitcoin::Txid, Vec<OutPoint>>,
    pub spent_withdrawals: HashMap<OutPoint, Output>,
    pub failed_withdrawals: Vec<bitcoin::Txid>,
}

#[derive(Default)]
pub struct TransactionBuilder {
    inputs: Vec<OutPoint>,
    outputs: Vec<Output>,
}

impl TransactionBuilder {
    pub fn spend(self, outpoint: OutPoint) -> Self {
        let mut inputs = self.inputs;
        let outputs = self.outputs;
        inputs.push(outpoint);
        Self { inputs, outputs }
    }

    pub fn output(self, address: Address, content: BitNamesOutput) -> TransactionBuilder {
        let inputs = self.inputs;
        let mut outputs = self.outputs;
        let output = Output {
            address,
            content: Content::Custom(content),
        };
        outputs.push(output);
        Self { inputs, outputs }
    }

    pub fn value(self, address: Address, value: u64) -> TransactionBuilder {
        let inputs = self.inputs;
        let mut outputs = self.outputs;
        let output = Output {
            address,
            content: Content::Value(value),
        };
        outputs.push(output);
        Self { inputs, outputs }
    }

    pub fn withdraw(
        self,
        address: Address,
        main_address: bitcoin::Address,
        value: u64,
        main_fee: u64,
    ) -> TransactionBuilder {
        let inputs = self.inputs;
        let mut outputs = self.outputs;
        let output = Output {
            address,
            content: Content::Withdrawal {
                value,
                main_fee,
                main_address,
            },
        };
        outputs.push(output);
        Self { inputs, outputs }
    }

    pub fn commit(self, address: Address, key: Key, salt: Salt) -> Self {
        let commitment = BitNamesOutput::Commitment(hmac(&key, &salt));
        self.output(address, commitment)
    }

    pub fn reveal(self, address: Address, key: Key, salt: Salt) -> TransactionBuilder {
        let reveal = BitNamesOutput::Reveal { key, salt };
        self.output(address, reveal)
    }

    pub fn set(self, address: Address, key: Key, value: Value) -> TransactionBuilder {
        let key_value = BitNamesOutput::KeyValue { key, value };
        self.output(address, key_value)
    }

    pub fn build(self) -> Transaction {
        Transaction {
            inputs: self.inputs,
            outputs: self.outputs,
        }
    }
}

use std::cmp::{Eq, Ord, Ordering, PartialEq, PartialOrd};

#[derive(Eq, PartialEq, Clone, Debug)]
pub struct AggregatedWithdrawal {
    pub spent_utxos: HashMap<OutPoint, Output>,
    pub main_address: bitcoin::Address,
    pub value: u64,
    pub main_fee: u64,
}

impl Ord for AggregatedWithdrawal {
    fn cmp(&self, other: &Self) -> Ordering {
        if self == other {
            Ordering::Equal
        } else if self.main_fee > other.main_fee
            || self.value > other.value
            || self.main_address > other.main_address
        {
            Ordering::Greater
        } else {
            Ordering::Less
        }
    }
}

impl PartialOrd for AggregatedWithdrawal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
