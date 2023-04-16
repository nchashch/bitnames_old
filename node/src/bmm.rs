use crate::mainchain_client::MainClient;
use anyhow::Result;
use bitcoin::util::psbt::serialize::Deserialize;
use bitnames_state::{Address, Body, Content, Header, OutPoint, Output};
use bitnames_types::{bitcoin, bitcoin::hashes::Hash as _, WithdrawalBundleStatus};
use jsonrpsee::http_client::{HeaderMap, HttpClient, HttpClientBuilder};
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Clone)]
pub struct Bmm {
    client: HttpClient,
    block: Option<(Header, Body)>,
}

impl Bmm {
    pub fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        let auth = format!("{}:{}", "user", "password");
        let header_value = format!("Basic {}", base64::encode(auth)).parse()?;
        headers.insert("authorization", header_value);
        let client = HttpClientBuilder::default()
            .set_headers(headers.clone())
            .build("http://127.0.0.1:18443")?;

        Ok(Self {
            client,
            block: None,
        })
    }

    pub async fn get_mainchain_tip(&self) -> Result<bitcoin::BlockHash> {
        Ok(self.client.getbestblockhash().await?)
    }

    pub async fn get_withdrawal_bundle_statuses(
        &mut self,
    ) -> Result<HashMap<bitcoin::Txid, WithdrawalBundleStatus>> {
        todo!();
    }

    pub async fn get_deposit_outputs(
        &mut self,
        end: bitcoin::BlockHash,
        start: Option<bitcoin::BlockHash>,
    ) -> Result<(HashMap<OutPoint, Output>, Option<bitcoin::BlockHash>)> {
        let deposits = self
            .client
            .listsidechaindepositsbyblock(0, Some(end), start)
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

    pub async fn verify_bmm(&self, header: &Header) -> Result<()> {
        let prev_main_block_hash = header.prev_main_block_hash;
        let block_hash = self
            .client
            .getblock(&prev_main_block_hash, None)
            .await?
            .nextblockhash
            .ok_or(BmmError::NoNextBlock {
                prev_main_block_hash,
            })?;
        let value = self
            .client
            .verifybmm(&block_hash, &header.block_hash().into(), 0)
            .await?;
        Ok(())
    }

    /// This is called by sidechain "miners" to get a sidechain block BMMed.
    pub async fn attempt_bmm(
        &mut self,
        amount: u64,
        header: Header,
        body: Body,
    ) -> Result<bitcoin::Txid> {
        let str_hash_prev = header.prev_main_block_hash.to_string();
        let critical_hash: [u8; 32] = header.block_hash().into();
        let critical_hash = bitcoin::BlockHash::from_inner(critical_hash);
        const THIS_SIDECHAIN: u32 = 0;
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
        Ok(txid)
    }

    pub async fn confirm_bmm(&mut self) -> Option<(Header, Body)> {
        if let Some((header, _)) = &self.block {
            if self.verify_bmm(header).await.is_ok() {
                let block = self.block.clone();
                self.block = None;
                return block;
            }
        }
        None
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BmmError {
    #[error("no next block for prev_main_block_hash = {prev_main_block_hash}")]
    NoNextBlock {
        prev_main_block_hash: bitcoin::BlockHash,
    },
}
