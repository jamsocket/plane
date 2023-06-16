use anyhow::Result;
use plane_drone::keys::KeyCertPaths;
use rcgen::generate_simple_self_signed;
use std::{
    fs::{self, create_dir_all},
    path::PathBuf,
};

use crate::scratch_dir;

pub struct SelfSignedCert {
    pub key_pem: String,
    pub cert_pem: String,
    path: PathBuf,
    pub path_pair: KeyCertPaths,
}

impl SelfSignedCert {
    pub fn new(name: &str, subject_alt_names: Vec<String>) -> Result<SelfSignedCert> {
        let path = scratch_dir(name);
        create_dir_all(&path)?;

        let key_path = path.join("selfsigned.key");
        let cert_path = path.join("selfsigned.pem");
        let account_path = path.join("account.key");

        let cert = generate_simple_self_signed(subject_alt_names)?;

        let key_pem = cert.serialize_private_key_pem();
        let cert_pem = cert.serialize_pem()?;

        fs::write(&cert_path, &cert_pem)?;
        fs::write(&key_path, &key_pem)?;

        let path_pair = KeyCertPaths {
            cert_path,
            key_path,
            account_key_path: Some(account_path),
        };

        Ok(SelfSignedCert {
            key_pem,
            cert_pem,
            path,
            path_pair,
        })
    }

    pub fn path(&self) -> String {
        self.path.to_str().unwrap().to_owned()
    }
}
