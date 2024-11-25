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
