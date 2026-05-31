use progenitor::generate_api;
use std::sync::LazyLock;

pub mod collectors;
pub mod config;
pub mod remote_write;

generate_api!(spec = "./api-spec.json", interface = Builder);

pub static API_CLIENT: LazyLock<Client> = LazyLock::new(|| {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-api-key",
        HeaderValue::try_from(std::env::var("BITPING_API_KEY").expect("Couldn't get API key"))
            .unwrap(),
    );

    let req_client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    Client::new_with_client("https://api.bitping.com/v2", req_client)
});
