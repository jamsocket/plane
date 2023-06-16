use anyhow::{anyhow, Context, Result};
use openssl::pkey::{PKey, Private};
use rustls::{
    sign::{any_supported_type, CertifiedKey},
    Certificate, PrivateKey,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct KeyCertPaths {
    pub key_path: PathBuf,
    pub cert_path: PathBuf,
    pub account_key_path: Option<PathBuf>,
}

impl KeyCertPaths {
    pub fn load_certified_key(&self) -> Result<CertifiedKey> {
        let certificate = load_certs(&self.cert_path).context("Loading certificates")?;
        let private_key = load_private_key(&self.key_path).context("Loading private key")?;
        let private_key = any_supported_type(&private_key).context("Parsing private key")?;

        Ok(CertifiedKey::new(certificate, private_key))
    }

    pub fn load_account_key(&self) -> Result<PKey<Private>> {
        let Some(account_key_path) = &self.account_key_path else {
            tracing::warn!("account_key_path not set; we will always generate a new account key.");
            return acme2_eab::gen_rsa_private_key(4096).context("Generating private key");
        };
        let private_key: PKey<Private> = if account_key_path.exists() {
            tracing::info!(?account_key_path, "Loading account key");
            let pem = std::fs::read(account_key_path).context("Reading account key")?;
            PKey::private_key_from_pem(&pem).context("Parsing account key")?
        } else {
            tracing::info!(?account_key_path, "Generating new account key.");
            let private_key =
                acme2_eab::gen_rsa_private_key(4096).context("Generating private key")?;
            std::fs::write(account_key_path, private_key.private_key_to_pem_pkcs8()?)?;
            private_key
        };

        Ok(private_key)
    }

    /// Return a vector of directories that we should listen for filesystem events on.
    ///
    /// If the key and certificate are in the same directory, this will be a one-element vector with just that
    /// directory. If the key and certificate are in different directories, this will be a two-element vector
    /// with both directories.
    ///
    /// We listen to the parent directories rather than the files themselves because the files themselves
    /// may not exist when the server is started.
    pub fn parent_paths(&self) -> Vec<PathBuf> {
        let key_parent_dir = self
            .key_path
            .parent()
            .expect("Key file should never be filesystem root.");
        let certificate_parent_dir = self
            .key_path
            .parent()
            .expect("Key file should never be filesystem root.");

        if key_parent_dir == certificate_parent_dir {
            vec![key_parent_dir.to_path_buf()]
        } else {
            vec![
                key_parent_dir.to_path_buf(),
                certificate_parent_dir.to_path_buf(),
            ]
        }
    }
}

// Load public certificate from file.
// Source: https://github.com/rustls/hyper-rustls/blob/main/examples/server.rs
pub fn load_certs(filename: &Path) -> Result<Vec<Certificate>> {
    // Open certificate file.
    let certfile = File::open(filename).context("Opening certificate file")?;
    let mut reader = BufReader::new(certfile);

    // Load and return certificate.
    let certs = rustls_pemfile::certs(&mut reader).context("Reading certificates")?;
    Ok(certs.into_iter().map(rustls::Certificate).collect())
}

// Load private key from file.
// Source: https://github.com/rustls/hyper-rustls/blob/main/examples/server.rs
pub fn load_private_key(filename: &Path) -> Result<PrivateKey> {
    // Open keyfile.
    let keyfile = File::open(filename).context("Reading keyfile")?;
    let mut reader = BufReader::new(keyfile);

    // Load and return a single private key.
    let keys = rustls_pemfile::pkcs8_private_keys(&mut reader).context("Loading private keys")?;
    if keys.len() != 1 {
        return Err(anyhow!("expected a single private key"));
    }

    Ok(rustls::PrivateKey(keys[0].clone()))
}
