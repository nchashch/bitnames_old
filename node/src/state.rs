use anyhow::Result;
use bitnames_state::*;
use std::collections::HashSet;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum BitNamesStateMessage {
    GetLastDepositBlockHash {
        respond_to: oneshot::Sender<Result<Option<bitcoin::BlockHash>, bitnames_state::Error>>,
    },
    GetBestHeader {
        respond_to: oneshot::Sender<Result<(u32, Header), bitnames_state::Error>>,
    },
    ConnectBlock {
        header: Header,
        body: Body,
        two_way_peg_data: TwoWayPegData,
    },
    ValidateTransaction {
        transaction: AuthorizedTransaction,
        respond_to: oneshot::Sender<Result<u64, bitnames_state::Error>>,
    },
    GetUtxosByAddresses {
        addresses: Vec<Address>,
        respond_to: oneshot::Sender<Result<Vec<(OutPoint, Output)>, bitnames_state::Error>>,
    },
    GetPendingWithdrawalBundle {
        respond_to: oneshot::Sender<Result<Option<WithdrawalBundle>, bitnames_state::Error>>,
    },
}

pub struct BitNamesStateActor {
    env: heed::Env,
    state: BitNamesState,
    receiver: mpsc::Receiver<BitNamesStateMessage>,
}

impl BitNamesStateActor {
    fn new(env: heed::Env, receiver: mpsc::Receiver<BitNamesStateMessage>) -> Result<Self> {
        let state = BitNamesState::new(&env)?;
        Ok(Self {
            env,
            state,
            receiver,
        })
    }

    async fn handle_message(&self, message: BitNamesStateMessage) {
        match message {
            BitNamesStateMessage::ConnectBlock {
                header,
                body,
                two_way_peg_data,
            } => {
                let mut wtxn = self.env.write_txn().unwrap();
                self.state
                    .validate_block(&wtxn, &header, &body, &two_way_peg_data)
                    .unwrap();
                self.state
                    .connect_block(&mut wtxn, &header, &body, &two_way_peg_data)
                    .unwrap();
                wtxn.commit().unwrap();
            }
            BitNamesStateMessage::ValidateTransaction {
                transaction,
                respond_to,
            } => {
                let rtxn = self.env.read_txn().unwrap();
                let result = self
                    .state
                    .validate_transaction(&rtxn, &transaction.transaction);
                respond_to.send(result).unwrap();
            }
            BitNamesStateMessage::GetBestHeader { respond_to } => {
                let rtxn = self.env.read_txn().unwrap();
                let result = self.state.get_best_header(&rtxn);
                respond_to.send(result).unwrap();
            }
            BitNamesStateMessage::GetUtxosByAddresses {
                addresses,
                respond_to,
            } => {
                let rtxn = self.env.read_txn().unwrap();
                let addresses: HashSet<_> = addresses.into_iter().collect();
                let result = self.state.get_utxos_by_addresses(&rtxn, &addresses);
                respond_to.send(result).unwrap();
            }
            BitNamesStateMessage::GetLastDepositBlockHash { respond_to } => {
                let rtxn = self.env.read_txn().unwrap();
                let result = self.state.get_last_deposit_block_hash(&rtxn);
                respond_to.send(result).unwrap();
            }
            BitNamesStateMessage::GetPendingWithdrawalBundle { respond_to } => {
                let rtxn = self.env.read_txn().unwrap();
                let result = self.state.get_pending_withdrawal_bundle(&rtxn);
                respond_to.send(result).unwrap();
            }
        }
    }
}

#[derive(Clone)]
pub struct BitNamesStateHandle {
    sender: mpsc::Sender<BitNamesStateMessage>,
}

pub async fn spawn_bitnames_state(env: heed::Env) -> Result<BitNamesStateHandle> {
    let (sender, receiver) = mpsc::channel(1024);
    let state = BitNamesStateActor::new(env, receiver)?;
    tokio::task::spawn(run_bitnames_state_actor(state));
    Ok(BitNamesStateHandle { sender })
}

pub async fn run_bitnames_state_actor(mut actor: BitNamesStateActor) {
    while let Some(message) = actor.receiver.recv().await {
        actor.handle_message(message).await;
    }
}

impl BitNamesStateHandle {
    pub async fn connect_block(&self, header: Header, body: Body, two_way_peg_data: TwoWayPegData) {
        println!("--- connecting block {} ---", header.block_hash());
        self.sender
            .send(BitNamesStateMessage::ConnectBlock {
                header,
                body,
                two_way_peg_data,
            })
            .await
            .unwrap();
    }

    pub async fn validate_transaction(
        &self,
        transaction: AuthorizedTransaction,
    ) -> Result<u64, bitnames_state::Error> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(BitNamesStateMessage::ValidateTransaction {
                transaction,
                respond_to,
            })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn get_best_header(&self) -> Result<(u32, Header), bitnames_state::Error> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(BitNamesStateMessage::GetBestHeader { respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn get_last_deposit_block_hash(
        &self,
    ) -> Result<Option<bitcoin::BlockHash>, bitnames_state::Error> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(BitNamesStateMessage::GetLastDepositBlockHash { respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn get_pending_withdrawal_bundle(
        &self,
    ) -> Result<Option<WithdrawalBundle>, bitnames_state::Error> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(BitNamesStateMessage::GetPendingWithdrawalBundle { respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }

    pub async fn get_utxos_by_addresses(
        &self,
        addresses: Vec<Address>,
    ) -> Result<Vec<(OutPoint, Output)>, bitnames_state::Error> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(BitNamesStateMessage::GetUtxosByAddresses {
                addresses,
                respond_to,
            })
            .await
            .unwrap();
        receiver.await.unwrap()
    }
}
