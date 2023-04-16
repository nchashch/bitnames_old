mod amount;
mod bmm;
mod bmm_actor;
mod mainchain_client;
mod mempool;
mod state;

use anyhow::Result;
use bitnames_api::bit_names_server::{BitNames, BitNamesServer};
use bitnames_api::*;
use bitnames_state::Body;
use bitnames_state::*;
use core::str::FromStr;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

#[derive(Clone)]
struct BitNamesNode {
    // libp2p -- event source
    state: state::BitNamesStateHandle,
    mempool: mempool::MemPoolHandle,
    bmm: bmm_actor::BmmHandle,
    // bip300/bip301
    // libp2p -- event sink
}

impl BitNamesNode {
    async fn new() -> Result<BitNamesNode> {
        let env = new_env();
        let state = state::spawn_bitnames_state(env.clone()).await?;
        let mempool = mempool::spawn_mem_pool().await;
        let bmm = bmm_actor::spawn_bmm().await?;

        Ok(BitNamesNode {
            state,
            mempool,
            bmm,
        })
    }
}

#[tonic::async_trait]
impl BitNames for BitNamesNode {
    async fn get_utxos_by_addresses(
        &self,
        request: Request<GetUtxosByAddressesRequest>,
    ) -> Result<Response<GetUtxosByAddressesResponse>, Status> {
        let addresses = request
            .into_inner()
            .addresses
            .into_iter()
            .map(|address| {
                let address: [u8; 32] = address.try_into().unwrap();
                Address::from(address)
            })
            .collect();
        let utxos: Vec<_> = self
            .state
            .get_utxos_by_addresses(addresses)
            .await
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
        let result = self.state.validate_transaction(transaction.clone()).await;
        let (valid, fee) = match result {
            Ok(fee) => (true, fee),
            Err(_) => (false, 0),
        };
        if valid {
            self.mempool.add_transaction(transaction).await;
        }
        Ok(Response::new(SubmitTransactionResponse { valid, fee }))
    }
    async fn submit_block(
        &self,
        request: Request<SubmitBlockRequest>,
    ) -> Result<Response<SubmitBlockResponse>, Status> {
        let request = request.into_inner();
        let header = request.header;
        let header: Header = bincode::deserialize(&header).unwrap();
        let body = request.body;
        let body: Body = bincode::deserialize(&body).unwrap();
        let txids: Vec<_> = body.transactions.iter().map(Transaction::txid).collect();
        let start = self.state.get_last_deposit_block_hash().await.unwrap();
        let end = header.prev_main_block_hash;
        let two_way_peg_data = self.bmm.get_two_way_peg_data(end, start).await;
        self.bmm.verify_bmm(header.clone()).await;
        self.state
            .connect_block(header, body, two_way_peg_data)
            .await;
        self.mempool.remove_transactions(txids).await;
        if let Some(bundle) = self.state.get_pending_withdrawal_bundle().await.unwrap() {
            self.bmm
                .broadcast_withdrawal_bundle(&bundle.transaction)
                .await
                .unwrap();
        }
        Ok(Response::new(SubmitBlockResponse {}))
    }

    // TODO: Reconsider this RPC.
    async fn attempt_bmm(
        &self,
        request: Request<AttemptBmmRequest>,
    ) -> Result<Response<AttemptBmmResponse>, Status> {
        let amount = request.into_inner().amount;
        let transactions = self.mempool.get_transactions().await;
        let body = Body::new(transactions, vec![]);
        let (_, prev_header) = self.state.get_best_header().await.unwrap();
        let prev_side_block_hash = prev_header.block_hash();
        let prev_main_block_hash = self.bmm.get_mainchain_tip().await;
        let header = Header {
            merkle_root: body.compute_merkle_root(),
            prev_side_block_hash,
            prev_main_block_hash,
        };
        self.bmm.attempt_bmm(amount, header, body).await;
        Ok(Response::new(AttemptBmmResponse {}))
    }

    async fn confirm_bmm(
        &self,
        request: Request<ConfirmBmmRequest>,
    ) -> Result<Response<ConfirmBmmResponse>, Status> {
        let connected = match self.bmm.confirm_bmm().await {
            Some((header, body)) => {
                let start = self.state.get_last_deposit_block_hash().await.unwrap();
                let end = header.prev_main_block_hash;
                let two_way_peg_data = self.bmm.get_two_way_peg_data(end, start).await;
                dbg!(&header, &two_way_peg_data, &body);
                let txids = body
                    .transactions
                    .iter()
                    .map(Transaction::txid)
                    .collect::<Vec<_>>();
                self.state
                    .connect_block(header, body, two_way_peg_data)
                    .await;
                if let Some(bundle) = self.state.get_pending_withdrawal_bundle().await.unwrap() {
                    self.bmm
                        .broadcast_withdrawal_bundle(&bundle.transaction)
                        .await
                        .unwrap();
                }
                self.mempool.remove_transactions(txids).await;
                true
            }
            None => false,
        };
        return Ok(Response::new(ConfirmBmmResponse { connected }));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let addr = "[::1]:50051".parse().unwrap();
    let node = BitNamesNode::new().await?;
    println!("BitNames server is listening on {}", addr);
    Server::builder()
        .add_service(BitNamesServer::new(node.clone()))
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
