use std::time::Duration;
use anyhow::{anyhow, Result};
use rand::{distributions::Alphanumeric, Rng};
use std::time::SystemTime;

pub fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

pub async fn wait_for_url(url: &str, timeout_ms: u128) -> Result<()> {
    let initial_time = SystemTime::now();
    let client = reqwest::ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .build()?;
    loop {
        let result = client.get(url.clone()).send().await;

        match result {
            Ok(_) => return Ok(()),
            Err(e) => {
                if SystemTime::now()
                    .duration_since(initial_time)
                    .unwrap()
                    .as_millis()
                    > timeout_ms
                {
                    return Err(anyhow!(
                        "Failed to load URL {} after {}ms. Last error was {:?}",
                        url,
                        timeout_ms,
                        e
                    ));
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
