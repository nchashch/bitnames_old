use crate::bmm::Bmm;
use anyhow::Result;
use bitcoin::hashes::Hash as _;
use bitnames_state::*;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};

const BLOCK_SIZE_LIMIT: usize = 100 * 1024;

pub struct BmmActor {
    bmm: Bmm,
    receiver: mpsc::Receiver<BmmMessage>,
}

#[derive(Clone)]
pub struct BmmHandle {
    sender: mpsc::Sender<BmmMessage>,
}

impl BmmHandle {
    pub async fn attempt_bmm(&self, amount: u64, header: Header, body: Body) {
        self.sender
            .send(BmmMessage::AttemptBmm {
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
            .send(BmmMessage::ConfirmBmm { respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn verify_bmm(&self, header: Header) -> bool {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(BmmMessage::VerifyBmm { header, respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn get_mainchain_tip(&self) -> bitcoin::BlockHash {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(BmmMessage::GetMainchainTip { respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn get_two_way_peg_data(
        &self,
        end: bitcoin::BlockHash,
        start: bitcoin::BlockHash,
    ) -> TwoWayPegData {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(BmmMessage::GetTwoWayPegData {
                end,
                start,
                respond_to,
            })
            .await
            .unwrap();
        receiver.await.unwrap()
    }
}

#[derive(Debug)]
pub enum BmmMessage {
    GetTwoWayPegData {
        end: bitcoin::BlockHash,
        start: bitcoin::BlockHash,
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

pub async fn spawn_bmm(env: heed::Env) -> Result<BmmHandle> {
    let (sender, receiver) = mpsc::channel(1024);
    let bmm = BmmActor::new(env, receiver)?;
    tokio::task::spawn(run_bmm_actor(bmm));
    Ok(BmmHandle { sender })
}

async fn run_bmm_actor(mut actor: BmmActor) {
    while let Some(message) = actor.receiver.recv().await {
        actor.handle_message(message).await;
    }
}

impl BmmActor {
    async fn handle_message(&mut self, message: BmmMessage) {
        match message {
            BmmMessage::AttemptBmm {
                amount,
                header,
                body,
            } => {
                self.bmm.attempt_bmm(amount, header, body).await.unwrap();
            }
            BmmMessage::ConfirmBmm { respond_to } => {
                let block = self.bmm.confirm_bmm().await;
                respond_to.send(block).unwrap();
            }
            BmmMessage::VerifyBmm { header, respond_to } => {
                let is_valid = self.bmm.verify_bmm(&header).await.is_ok();
                respond_to.send(is_valid).unwrap();
            }
            BmmMessage::GetMainchainTip { respond_to } => {
                let tip = self.bmm.get_mainchain_tip().await.unwrap();
                respond_to.send(tip).unwrap();
            }
            BmmMessage::GetTwoWayPegData {
                end,
                start,
                respond_to,
            } => {
                let deposits = self.bmm.get_deposit_outputs(end).await.unwrap();
                let two_way_peg_data = TwoWayPegData {
                    deposits,
                    ..TwoWayPegData::default()
                };
                respond_to.send(two_way_peg_data).unwrap();
            }
        }
    }
    fn new(env: heed::Env, receiver: mpsc::Receiver<BmmMessage>) -> Result<Self> {
        Ok(BmmActor {
            bmm: Bmm::new(env)?,
            receiver,
        })
    }
}
