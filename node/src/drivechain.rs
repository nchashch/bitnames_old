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

pub struct DrivechainActor {
    client: HttpClient,
    block: Option<(Header, Body)>,
    receiver: mpsc::Receiver<DrivechainMessage>,
}

#[derive(Clone)]
pub struct DrivechainHandle {
    sender: mpsc::Sender<DrivechainMessage>,
}

impl DrivechainHandle {
    pub async fn attempt_bmm(&self, amount: u64, header: Header, body: Body) {
        self.sender
            .send(DrivechainMessage::AttemptBmm {
                amount,
                header,
                body,
            })
            .await
            .unwrap();
    }

    pub async fn confirm_bmm(&self) -> Option<(Header, Body)> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(DrivechainMessage::ConfirmBmm { respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn verify_bmm(&self, header: Header) -> bool {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(DrivechainMessage::VerifyBmm { header, respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn get_mainchain_tip(&self) -> bitcoin::BlockHash {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(DrivechainMessage::GetMainchainTip { respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn get_two_way_peg_data(
        &self,
        end: bitcoin::BlockHash,
        start: Option<bitcoin::BlockHash>,
    ) -> TwoWayPegData {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(DrivechainMessage::GetTwoWayPegData {
                end,
                start,
                respond_to,
            })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn broadcast_withdrawal_bundle(
        &self,
        transaction: bitcoin::Transaction,
    ) -> Result<()> {
        self.sender
            .send(DrivechainMessage::BroadcastWithdrawalBundle { transaction })
            .await?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum DrivechainMessage {
    BroadcastWithdrawalBundle {
        transaction: bitcoin::Transaction,
    },
    GetTwoWayPegData {
        end: bitcoin::BlockHash,
        start: Option<bitcoin::BlockHash>,
        respond_to: oneshot::Sender<TwoWayPegData>,
    },
    GetMainchainTip {
        respond_to: oneshot::Sender<bitcoin::BlockHash>,
    },
    AttemptBmm {
        amount: u64,
        header: Header,
        body: Body,
    },
    ConfirmBmm {
        respond_to: oneshot::Sender<Option<(Header, Body)>>,
    },
    VerifyBmm {
        header: Header,
        respond_to: oneshot::Sender<bool>,
    },
}

pub async fn spawn_drivechain() -> Result<DrivechainHandle> {
    let (sender, receiver) = mpsc::channel(1024);
    let drivechain = DrivechainActor::new(receiver)?;
    tokio::task::spawn(run_drivechain_actor(drivechain));
    Ok(DrivechainHandle { sender })
}

async fn run_drivechain_actor(mut actor: DrivechainActor) {
    while let Some(message) = actor.receiver.recv().await {
        actor.handle_message(message).await;
    }
}

impl DrivechainActor {
    async fn handle_message(&mut self, message: DrivechainMessage) {
        match message {
            DrivechainMessage::BroadcastWithdrawalBundle { transaction } => {
                let rawtx = transaction.serialize();
                let rawtx = hex::encode(&rawtx);
                self.client
                    .receivewithdrawalbundle(THIS_SIDECHAIN, &rawtx)
                    .await
                    .unwrap();
            }
            DrivechainMessage::AttemptBmm {
                amount,
                header,
                body,
            } => {
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
                    .await
                    .unwrap();
                let txid =
                    bitcoin::Txid::from_str(value["txid"]["txid"].as_str().unwrap()).unwrap();
                assert_eq!(header.merkle_root, body.compute_merkle_root());
                self.block = Some((header, body));
            }
            DrivechainMessage::ConfirmBmm { respond_to } => {
                if let Some((header, body)) = &self.block {
                    if self.verify_bmm(header).await.is_ok() {
                        let block = self.block.clone();
                        self.block = None;
                        respond_to.send(block).unwrap();
                    }
                }
            }
            DrivechainMessage::VerifyBmm { header, respond_to } => {
                let is_valid = self.verify_bmm(&header).await.is_ok();
                respond_to.send(is_valid).unwrap();
            }
            DrivechainMessage::GetMainchainTip { respond_to } => {
                let tip = self.client.getbestblockhash().await.unwrap();
                respond_to.send(tip).unwrap();
            }
            DrivechainMessage::GetTwoWayPegData {
                end,
                start,
                respond_to,
            } => {
                let (deposits, deposit_block_hash) =
                    self.get_deposit_outputs(end, start).await.unwrap();
                let bundle_statuses = self.get_withdrawal_bundle_statuses().await.unwrap();
                let two_way_peg_data = TwoWayPegData {
                    deposits,
                    deposit_block_hash,
                    bundle_statuses,
                };
                respond_to.send(two_way_peg_data).unwrap();
            }
        }
    }
    async fn verify_bmm(&self, header: &Header) -> Result<()> {
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
    fn new(receiver: mpsc::Receiver<DrivechainMessage>) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let auth = format!("{}:{}", "user", "password");
        let header_value = format!("Basic {}", base64::encode(auth)).parse()?;
        headers.insert("authorization", header_value);
        let client = HttpClientBuilder::default()
            .set_headers(headers.clone())
            .build("http://127.0.0.1:18443")?;
        Ok(DrivechainActor {
            client,
            block: None,
            receiver,
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
