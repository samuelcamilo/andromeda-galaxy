use reqwest::{Client, Error};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::time::Duration;

#[derive(Serialize, Deserialize)]
struct RetryOptions {
    retry: bool,
    retry_quantities: usize,
    time_to_wait_to_next_retry: u64,
}

#[derive(Clone)]
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    pub fn new() -> Self {
        let client = Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        HttpClient { client }
    }

    pub async fn retry<T, Fut, F>(&self, operation: F) -> Result<T, Error>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        let max_retries = 3;
        let wait_time = Duration::from_millis(1000);

        for attempt in 0..max_retries {
            match operation().await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    println!("Tentativa {} falhou: {:?}", attempt + 1, e);
                    if attempt < max_retries - 1 {
                        tokio::time::sleep(wait_time).await;
                    }
                }
            }
        }

        let err = reqwest::Client::new()
            .get("error")
            .send()
            .await
            .unwrap_err();
        Err(err)
    }

    pub fn get_client(&self) -> &Client {
        &self.client
    }
}
