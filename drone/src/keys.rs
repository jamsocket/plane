use anyhow::{anyhow, Result};
use rustls::{
    sign::{any_supported_type, CertifiedKey},
    Certificate, PrivateKey,
};
use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCertPathPair {
    pub private_key_path: PathBuf,
    pub certificate_path: PathBuf,
}

impl KeyCertPathPair {
    pub fn load_certified_key(&self) -> Result<CertifiedKey> {
        let certificate = load_certs(&self.certificate_path)?;
        let private_key = load_private_key(&self.private_key_path)?;
        let private_key = any_supported_type(&private_key)?;

        Ok(CertifiedKey::new(certificate, private_key))
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
            .private_key_path
            .parent()
            .expect("Key file should never be filesystem root.");
        let certificate_parent_dir = self
            .private_key_path
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
    let certfile = File::open(filename)?;
    let mut reader = BufReader::new(certfile);

    // Load and return certificate.
    let certs = rustls_pemfile::certs(&mut reader)?;
    Ok(certs.into_iter().map(rustls::Certificate).collect())
}

// Load private key from file.
// Source: https://github.com/rustls/hyper-rustls/blob/main/examples/server.rs
pub fn load_private_key(filename: &Path) -> Result<PrivateKey> {
    // Open keyfile.
    let keyfile = File::open(filename)?;
    let mut reader = BufReader::new(keyfile);

    // Load and return a single private key.
    let keys = rustls_pemfile::pkcs8_private_keys(&mut reader)?;
    if keys.len() != 1 {
        return Err(anyhow!("expected a single private key"));
    }

    Ok(rustls::PrivateKey(keys[0].clone()))
}
