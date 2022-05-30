use std::{path::{PathBuf, Path}, fs::File, io::BufReader};
use anyhow::{anyhow, Result};
use rustls::{PrivateKey, Certificate};

pub struct KeyCertPathPair {
    pub private_key_path: PathBuf,
    pub certificate_path: PathBuf,
}

#[derive(Clone, PartialEq, Debug)]
pub struct CertifiedKey {
    pub private_key: PrivateKey,
    pub certificate: Vec<Certificate>,
}

impl CertifiedKey {
    pub fn new(path_pair: &KeyCertPathPair) -> Result<Self> {
        let certificate = load_certs(&path_pair.certificate_path)?;
        let private_key = load_private_key(&path_pair.private_key_path)?;

        Ok(CertifiedKey {
            certificate, private_key
        })
    }
}

// Load public certificate from file.
// Source: https://github.com/rustls/hyper-rustls/blob/main/examples/server.rs
fn load_certs(filename: &Path) -> Result<Vec<Certificate>> {
    // Open certificate file.
    let certfile = File::open(filename.clone())?;
    let mut reader = BufReader::new(certfile);

    // Load and return certificate.
    let certs = rustls_pemfile::certs(&mut reader)?;
    Ok(certs.into_iter().map(rustls::Certificate).collect())
}

// Load private key from file.
// Source: https://github.com/rustls/hyper-rustls/blob/main/examples/server.rs
fn load_private_key(filename: &Path) -> Result<PrivateKey> {
    // Open keyfile.
    let keyfile = File::open(filename.clone())?;
    let mut reader = BufReader::new(keyfile);

    // Load and return a single private key.
    let keys = rustls_pemfile::pkcs8_private_keys(&mut reader)?;
    if keys.len() != 1 {
        return Err(anyhow!("expected a single private key"));
    }

    Ok(rustls::PrivateKey(keys[0].clone()))
}