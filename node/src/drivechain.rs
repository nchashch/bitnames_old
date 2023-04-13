use bitnames_state::{bitcoin, OutPoint};
use jsonrpsee::http_client::{HeaderMap, HttpClient, HttpClientBuilder};

struct Drivechain {
    client: HttpClient,
}

impl Drivechain {
    fn new() -> Self {
        let mut headers = HeaderMap::new();
        let auth = format!("{}:{}", "user", "password");
        let header_value = format!("Basic {}", base64::encode(auth)).parse().unwrap();
        headers.insert("authorization", header_value);
        let client = HttpClientBuilder::default()
            .set_headers(headers.clone())
            .build("http://127.0.0.1:18443")
            .unwrap();
        Self { client }
    }

    fn verify_bmm() -> bool {
        todo!();
    }

    fn submit_bmm() -> Result<(), ()> {
        todo!();
    }

    fn get_update(prev: &bitcoin::BlockHash, curr: &bitcoin::BlockHash) -> Update {
        todo!();
    }
}

struct Update {
    deposits: Vec<Deposit>,
    spent_withdrawals: Vec<OutPoint>,
    locked_withdrawals: Vec<OutPoint>,
    unlocked_withdrawals: Vec<OutPoint>,
}

struct Deposit {
    address: [u8; 32],
    value: u64,
}
