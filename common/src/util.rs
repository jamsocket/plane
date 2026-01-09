use rand::{distributions::Uniform, prelude::Distribution, Rng};

const ALLOWED_CHARS: &str = "abcdefghijklmnopqrstuvwxyz0123456789";

pub fn random_string() -> String {
    let range = Uniform::new(0, ALLOWED_CHARS.len());
    let mut rng = rand::thread_rng();

    range
        .sample_iter(&mut rng)
        .take(14)
        .map(|i| ALLOWED_CHARS.chars().nth(i).expect("Index is always valid"))
        .collect()
}

pub fn random_token() -> String {
    let mut token = [0u8; 32];
    rand::thread_rng().fill(&mut token);
    data_encoding::BASE64URL_NOPAD.encode(&token)
}

pub fn random_prefixed_string(prefix: &str) -> String {
    format!("{}-{}", prefix, random_string())
}

/// Returns a duration of approximately 1 hour, with random jitter of +/- 10 minutes.
pub fn random_reconnect_interval() -> std::time::Duration {
    const BASE_SECONDS: u64 = 60 * 60; // 1 hour
    const JITTER_SECONDS: u64 = 10 * 60; // 10 minutes

    let jitter: i64 =
        rand::thread_rng().gen_range(-(JITTER_SECONDS as i64)..=(JITTER_SECONDS as i64));
    let total_seconds = (BASE_SECONDS as i64 + jitter) as u64;
    std::time::Duration::from_secs(total_seconds)
}
