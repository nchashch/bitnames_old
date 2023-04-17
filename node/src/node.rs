use crate::drivechain::Drivechain;
use anyhow::Result;
use bitnames_api::{
    bit_names_server::{BitNames, BitNamesServer},
    *,
};
use bitnames_state::*;
use core::str::FromStr;
use futures::executor::block_on;
use std::collections::{HashMap, HashSet};

pub struct Node {
    env: heed::Env,
    state: BitNamesState,
    mempool: HashMap<Txid, AuthorizedTransaction>,
    drivechain: Drivechain,
}

impl Node {
    pub fn new() -> Result<Self> {
        let env = new_env();
        let drivechain = Drivechain::new()?;
        let state = BitNamesState::new(&env)?;
        Ok(Self {
            env,
            state,
            mempool: HashMap::new(),
            drivechain,
        })
    }

    pub fn get_utxos_by_addresses(&self, addresses: &[Address]) -> Result<Vec<(OutPoint, Output)>> {
        let addresses: HashSet<_> = addresses.iter().copied().collect();
        let rtxn = self.env.read_txn().unwrap();
        let utxos = self.state.get_utxos_by_addresses(&rtxn, &addresses)?;
        Ok(utxos)
    }

    pub fn submit_transaction(
        &mut self,
        transaction: AuthorizedTransaction,
    ) -> Result<(bool, u64)> {
        let rtxn = self.env.read_txn().unwrap();
        // FIXME: check signatures here
        let result = self
            .state
            .validate_transaction(&rtxn, &transaction.transaction);
        let (valid, fee) = match result {
            Ok(fee) => (true, fee),
            Err(_) => (false, 0),
        };
        if valid {
            self.mempool
                .insert(transaction.transaction.txid(), transaction);
        }
        Ok((valid, fee))
    }

    pub fn generate_block(&self) -> Result<(Header, Body)> {
        let transactions = self.mempool.values().cloned().collect();
        let body = Body::new(transactions, vec![]);
        let rtxn = self.env.read_txn().unwrap();
        let (_, prev_header) = self.state.get_best_header(&rtxn).unwrap();
        let prev_side_block_hash = prev_header.block_hash();
        let prev_main_block_hash = block_on(self.drivechain.get_mainchain_tip()).unwrap();
        let header = Header {
            merkle_root: body.compute_merkle_root(),
            prev_side_block_hash,
            prev_main_block_hash,
        };
        Ok((header, body))
    }

    pub async fn attempt_bmm(&mut self, amount: u64) -> Result<()> {
        let (header, body) = self.generate_block()?;
        self.drivechain.attempt_bmm(amount, header, body).await?;
        Ok(())
    }

    pub fn connect_block(&mut self, header: &Header, body: &Body) -> Result<()> {
        let mut wtxn = self.env.write_txn().unwrap();
        let start = self.state.get_last_deposit_block_hash(&wtxn).unwrap();
        let end = header.prev_main_block_hash;
        let two_way_peg_data = block_on(self.drivechain.get_two_way_peg_data(end, start)).unwrap();
        dbg!(&header, &two_way_peg_data, &body);
        let txids = body
            .transactions
            .iter()
            .map(Transaction::txid)
            .collect::<Vec<_>>();
        self.state
            .connect_block(&mut wtxn, &header, &body, &two_way_peg_data)
            .unwrap();
        if let Some(bundle) = self.state.get_pending_withdrawal_bundle(&wtxn).unwrap() {
            block_on(
                self.drivechain
                    .broadcast_withdrawal_bundle(bundle.transaction),
            )
            .unwrap();
        }
        for txid in &txids {
            self.mempool.remove(txid);
        }
        wtxn.commit().unwrap();
        Ok(())
    }

    pub async fn confirm_bmm(&mut self) -> Result<bool> {
        match self.drivechain.confirm_bmm().await? {
            Some((header, body)) => {
                self.connect_block(&header, &body)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }
}

fn new_env() -> heed::Env {
    let env_path = std::path::Path::new("target").join("clear-database.mdb");
    //let _ = std::fs::remove_dir_all(&env_path);
    std::fs::create_dir_all(&env_path).unwrap();
    let env = heed::EnvOpenOptions::new()
        .map_size(10 * 1024 * 1024) // 10MB
        .max_dbs(BitNamesState::NUM_DBS)
        .open(env_path)
        .unwrap();
    env
}
