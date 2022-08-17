use anyhow::Result;
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

        Ok(Certificates {
            key_pem,
            cert_pem,
            path,
        })
    }

    pub fn path(&self) -> String {
        self.path.to_str().unwrap().to_owned()
    }
}
