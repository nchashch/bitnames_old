use bitnames_state::*;
use std::collections::HashMap;
use tokio::sync::{mpsc, oneshot};

const BLOCK_SIZE_LIMIT: usize = 100 * 1024;

#[derive(Debug)]
pub struct MemPoolActor {
    // Relevant things are fee and size.
    transactions: HashMap<Txid, AuthorizedTransaction>,
    receiver: mpsc::Receiver<MemPoolMessage>,
}

#[derive(Clone)]
pub struct MemPoolHandle {
    sender: mpsc::Sender<MemPoolMessage>,
}

impl MemPoolHandle {
    pub async fn add_transaction(&self, transaction: AuthorizedTransaction) {
        self.sender
            .send(MemPoolMessage::AddTransaction { transaction })
            .await
            .unwrap();
    }

    pub async fn remove_transactions(&self, txids: Vec<Txid>) {
        self.sender
            .send(MemPoolMessage::RemoveTransactions { txids })
            .await
            .unwrap();
    }

    pub async fn get_transactions(&self) -> Vec<AuthorizedTransaction> {
        let (respond_to, receiver) = oneshot::channel();
        self.sender
            .send(MemPoolMessage::GetTransactions { respond_to })
            .await
            .unwrap();
        receiver.await.unwrap()
    }
}

#[derive(Debug)]
pub enum MemPoolMessage {
    AddTransaction {
        transaction: AuthorizedTransaction,
    },
    RemoveTransactions {
        txids: Vec<Txid>,
    },
    GetTransactions {
        respond_to: oneshot::Sender<Vec<AuthorizedTransaction>>,
    },
}

pub async fn spawn_mem_pool() -> MemPoolHandle {
    let (sender, receiver) = mpsc::channel(1024);
    let mempool = MemPoolActor::new(receiver);
    tokio::task::spawn(run_mem_pool_actor(mempool));
    MemPoolHandle { sender }
}

async fn run_mem_pool_actor(mut actor: MemPoolActor) {
    while let Some(message) = actor.receiver.recv().await {
        actor.handle_message(message);
    }
}

impl MemPoolActor {
    fn handle_message(&mut self, message: MemPoolMessage) {
        match message {
            MemPoolMessage::AddTransaction { transaction } => {
                self.transactions
                    .insert(transaction.transaction.txid(), transaction);
            }
            MemPoolMessage::RemoveTransactions { txids } => {
                for txid in &txids {
                    self.transactions.remove(txid);
                }
            }
            MemPoolMessage::GetTransactions { respond_to } => {
                let transactions = self.get_transactions();
                // TODO: Get rid of this unwrap
                respond_to.send(transactions).unwrap();
            }
        }
    }
    fn new(receiver: mpsc::Receiver<MemPoolMessage>) -> Self {
        MemPoolActor {
            transactions: HashMap::new(),
            receiver,
        }
    }

    fn get_transactions(&self) -> Vec<AuthorizedTransaction> {
        self.transactions.values().cloned().collect()
    }
}
