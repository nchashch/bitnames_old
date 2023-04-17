mod amount;
mod drivechain;
mod mainchain_client;
mod node;

use anyhow::Result;
use bitnames_api::bit_names_server::{BitNames, BitNamesServer};
use bitnames_api::*;
use bitnames_state::Body;
use bitnames_state::*;
use core::str::FromStr;
use futures::executor::block_on;
use std::sync::Mutex;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

struct BitNamesNode {
    node: Mutex<node::Node>,
    // libp2p -- event source
    // libp2p -- event sink
}

impl BitNamesNode {
    async fn new() -> Result<BitNamesNode> {
        let node = Mutex::new(node::Node::new()?);
        Ok(BitNamesNode { node })
    }
}

#[tonic::async_trait]
impl BitNames for BitNamesNode {
    async fn get_utxos_by_addresses(
        &self,
        request: Request<GetUtxosByAddressesRequest>,
    ) -> Result<Response<GetUtxosByAddressesResponse>, Status> {
        let addresses: Vec<_> = request
            .into_inner()
            .addresses
            .into_iter()
            .map(|address| {
                let address: [u8; 32] = address.try_into().unwrap();
                Address::from(address)
            })
            .collect();
        let utxos = self
            .node
            .lock()
            .unwrap()
            .get_utxos_by_addresses(&addresses)
            .unwrap()
            .iter()
            .map(|utxo| bincode::serialize(utxo).unwrap())
            .collect();
        Ok(Response::new(GetUtxosByAddressesResponse { utxos }))
    }

    async fn submit_transaction(
        &self,
        request: Request<SubmitTransactionRequest>,
    ) -> Result<Response<SubmitTransactionResponse>, Status> {
        let transaction = request.into_inner().transaction;
        let transaction: AuthorizedTransaction = bincode::deserialize(&transaction).unwrap();
        let (valid, fee) = self
            .node
            .lock()
            .unwrap()
            .submit_transaction(transaction)
            .unwrap();
        Ok(Response::new(SubmitTransactionResponse { valid, fee }))
    }

    // TODO: Reconsider this RPC.
    async fn attempt_bmm(
        &self,
        request: Request<AttemptBmmRequest>,
    ) -> Result<Response<AttemptBmmResponse>, Status> {
        let amount = request.into_inner().amount;
        block_on(self.node.lock().unwrap().attempt_bmm(amount)).unwrap();
        Ok(Response::new(AttemptBmmResponse {}))
    }

    async fn confirm_bmm(
        &self,
        request: Request<ConfirmBmmRequest>,
    ) -> Result<Response<ConfirmBmmResponse>, Status> {
        let connected = block_on(self.node.lock().unwrap().confirm_bmm()).unwrap();
        return Ok(Response::new(ConfirmBmmResponse { connected }));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let addr = "[::1]:50051".parse().unwrap();
    let node = BitNamesNode::new().await?;
    println!("BitNames server is listening on {}", addr);
    Server::builder()
        .add_service(BitNamesServer::new(node))
        .serve(addr)
        .await?;
    Ok(())
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
