mod args;
mod authorization;

use anyhow::Result;
use args::{Bmm, Cli, Command};
use bitnames_api::bit_names_client::BitNamesClient;
use bitnames_api::*;
use bitnames_types::bitcoin;
use clap::Parser;
use ureq_jsonrpc::json;

#[tokio::main]
async fn main() -> Result<()> {
    let mut client = BitNamesClient::connect("http://[::1]:50051").await?;

    let args = Cli::parse();
    match args.command {
        Command::Bmm(command) => bmm(command, &mut client).await?,
    }
    Ok(())
}

async fn bmm(
    command: Bmm,
    client: &mut BitNamesClient<bitnames_api::tonic::transport::Channel>,
) -> Result<()> {
    let main = ureq_jsonrpc::Client {
        host: "localhost".to_string(),
        port: 18443,
        user: "user".into(),
        password: "password".into(),
        id: "bitnames_cli".to_string(),
    };

    match command {
        Bmm::Attempt { amount } => {
            let request = tonic::Request::new(AttemptBmmRequest {
                amount: amount.to_sat(),
            });
            let response = client.attempt_bmm(request).await?;
            println!("RESPONSE={:?}", response);
        }
        Bmm::Confirm {} => {
            let request = tonic::Request::new(ConfirmBmmRequest {});
            let response = client.confirm_bmm(request).await?;
            println!("RESPONSE={:?}", response);
        }

        Bmm::Generate { amount } => {
            let request = tonic::Request::new(AttemptBmmRequest {
                amount: amount.to_sat(),
            });
            client.attempt_bmm(request).await?;

            std::thread::sleep(std::time::Duration::from_millis(100));
            main.send_request::<Vec<bitcoin::BlockHash>>("generate", &[json!(1)])?;
            std::thread::sleep(std::time::Duration::from_millis(100));

            let request = tonic::Request::new(ConfirmBmmRequest {});
            let response = client.confirm_bmm(request).await?;
            println!("RESPONSE={:?}", response);
        }
    }
    Ok(())
}
