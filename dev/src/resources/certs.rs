use anyhow::Result;
use dis_spawner_drone::keys::KeyCertPathPair;
use rcgen::generate_simple_self_signed;
use std::{
    fs::{self, create_dir_all},
    path::PathBuf,
};

use crate::scratch_dir;

pub struct Certificates {
    pub key_pem: String,
    pub cert_pem: String,
    path: PathBuf,
    pub path_pair: KeyCertPathPair,
}

impl Certificates {
    pub fn new(name: &str, subject_alt_names: Vec<String>) -> Result<Certificates> {
        let path = scratch_dir(name);
        create_dir_all(&path)?;

        let key_path = path.join("selfsigned.key");
        let cert_path = path.join("selfsigned.pem");

        let cert = generate_simple_self_signed(subject_alt_names)?;

        let key_pem = cert.serialize_private_key_pem();
        let cert_pem = cert.serialize_pem()?;

        fs::write(&cert_path, &cert_pem)?;
        fs::write(&key_path, &key_pem)?;

        let path_pair = KeyCertPathPair {
            certificate_path: cert_path,
            private_key_path: key_path,
        };

        Ok(Certificates {
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
