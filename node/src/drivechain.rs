use crate::mainchain_client::MainClient;
use anyhow::Result;
use bitcoin::hashes::Hash as _;
use bitcoin::util::psbt::serialize::{Deserialize, Serialize};
use bitnames_state::*;
use jsonrpsee::http_client::{HeaderMap, HttpClient, HttpClientBuilder};
use std::collections::HashMap;
use std::str::FromStr;
use tokio::sync::{mpsc, oneshot};

const BLOCK_SIZE_LIMIT: usize = 100 * 1024;
const THIS_SIDECHAIN: usize = 0;

pub struct Drivechain {
    client: HttpClient,
    block: Option<(Header, Body)>,
}

impl Drivechain {
    pub async fn attempt_bmm(&mut self, amount: u64, header: Header, body: Body) -> Result<()> {
        let str_hash_prev = header.prev_main_block_hash.to_string();
        let critical_hash: [u8; 32] = header.block_hash().into();
        let critical_hash = bitcoin::BlockHash::from_inner(critical_hash);
        let value = self
            .client
            .createbmmcriticaldatatx(
                bitcoin::Amount::from_sat(amount).into(),
                0,
                &critical_hash,
                THIS_SIDECHAIN,
                &str_hash_prev[str_hash_prev.len() - 8..],
            )
            .await?;
        let txid = bitcoin::Txid::from_str(value["txid"]["txid"].as_str().unwrap())?;
        assert_eq!(header.merkle_root, body.compute_merkle_root());
        self.block = Some((header, body));
        Ok(())
    }

    pub async fn confirm_bmm(&mut self) -> Result<Option<(Header, Body)>> {
        if let Some((header, body)) = &self.block {
            self.verify_bmm(header).await?;
            let block = self.block.clone();
            self.block = None;
            return Ok(block);
        }
        Ok(None)
    }

    pub async fn verify_bmm(&self, header: &Header) -> Result<()> {
        let prev_main_block_hash = header.prev_main_block_hash;
        let block_hash = self
            .client
            .getblock(&prev_main_block_hash, None)
            .await?
            .nextblockhash
            .ok_or(DrivechainError::NoNextBlock {
                prev_main_block_hash,
            })?;
        let value = self
            .client
            .verifybmm(&block_hash, &header.block_hash().into(), THIS_SIDECHAIN)
            .await?;
        Ok(())
    }

    pub async fn get_mainchain_tip(&self) -> Result<bitcoin::BlockHash> {
        Ok(self.client.getbestblockhash().await?)
    }

    pub async fn get_two_way_peg_data(
        &mut self,
        end: bitcoin::BlockHash,
        start: Option<bitcoin::BlockHash>,
    ) -> Result<TwoWayPegData> {
        let (deposits, deposit_block_hash) = self.get_deposit_outputs(end, start).await?;
        let bundle_statuses = self.get_withdrawal_bundle_statuses().await?;
        let two_way_peg_data = TwoWayPegData {
            deposits,
            deposit_block_hash,
            bundle_statuses,
        };
        Ok(two_way_peg_data)
    }

    pub async fn broadcast_withdrawal_bundle(
        &self,
        transaction: bitcoin::Transaction,
    ) -> Result<()> {
        let rawtx = transaction.serialize();
        let rawtx = hex::encode(&rawtx);
        self.client
            .receivewithdrawalbundle(THIS_SIDECHAIN, &rawtx)
            .await?;
        Ok(())
    }

    async fn get_deposit_outputs(
        &mut self,
        end: bitcoin::BlockHash,
        start: Option<bitcoin::BlockHash>,
    ) -> Result<(HashMap<OutPoint, Output>, Option<bitcoin::BlockHash>)> {
        let deposits = self
            .client
            .listsidechaindepositsbyblock(THIS_SIDECHAIN, Some(end), start)
            .await?;
        let mut last_block_hash = None;
        let mut last_total = 0;
        let mut outputs = HashMap::new();
        dbg!(last_total);
        for deposit in &deposits {
            let transaction = hex::decode(&deposit.txhex)?;
            let transaction = bitcoin::Transaction::deserialize(transaction.as_slice())?;
            if let Some(start) = start {
                if deposit.hashblock == start {
                    last_total = transaction.output[deposit.nburnindex].value;
                    continue;
                }
            }
            let total = transaction.output[deposit.nburnindex].value;
            let value = total - last_total;
            let address: Address = deposit.strdest.parse()?;
            let output = Output {
                address,
                content: Content::Value(value),
            };
            let outpoint = OutPoint::Deposit(bitcoin::OutPoint {
                txid: transaction.txid(),
                vout: deposit.nburnindex as u32,
            });
            outputs.insert(outpoint, output);
            last_total = total;
            last_block_hash = Some(deposit.hashblock);
        }
        Ok((outputs, last_block_hash))
    }
    async fn get_withdrawal_bundle_statuses(
        &mut self,
    ) -> Result<HashMap<bitcoin::Txid, WithdrawalBundleStatus>> {
        let mut statuses = HashMap::new();
        for spent in &self.client.listspentwithdrawals().await? {
            if spent.nsidechain == THIS_SIDECHAIN {
                statuses.insert(spent.hash, WithdrawalBundleStatus::Confirmed);
            }
        }
        for failed in &self.client.listfailedwithdrawals().await? {
            statuses.insert(failed.hash, WithdrawalBundleStatus::Failed);
        }
        Ok(statuses)
    }

    pub fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        let auth = format!("{}:{}", "user", "password");
        let header_value = format!("Basic {}", base64::encode(auth)).parse()?;
        headers.insert("authorization", header_value);
        let client = HttpClientBuilder::default()
            .set_headers(headers.clone())
            .build("http://127.0.0.1:18443")?;
        Ok(Drivechain {
            client,
            block: None,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DrivechainError {
    #[error("no next block for prev_main_block_hash = {prev_main_block_hash}")]
    NoNextBlock {
        prev_main_block_hash: bitcoin::BlockHash,
    },
}
