use convert_base::Convert;
use sha2::{Digest, Sha256};

const ALPHANUM: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Convert a string key into a sha256 hash. Keys are serialized
/// using a full lowercase alphabet rather than hex, allowing
/// them to fit in the 63 character limit for a Kubernetes pod name.
pub fn hash_key(key: &str) -> String {
    let hash = Sha256::digest(key.as_bytes());

    let mut base = Convert::new(256, 36);
    let result = base.convert::<u8, u8>(&hash);
    let ch = result.into_iter().map(|ix| ALPHANUM[ix as usize] as char);

    ch.collect()
}
