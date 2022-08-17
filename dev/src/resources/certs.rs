use std::{path::PathBuf, fs};
use anyhow::Result;
use rcgen::generate_simple_self_signed;
use super::tempdir::TemporaryDirectory;

pub struct Certificates {
    key_path: PathBuf,
    cert_path: PathBuf,
    pub key_pem: String,
    pub cert_pem: String,
    tmpdir: TemporaryDirectory,
}

impl Certificates {
    pub fn new(subject_alt_names: Vec<String>) -> Result<Certificates> {
        let tmpdir = TemporaryDirectory::new()?;

        let key_path = tmpdir.path.join("selfsigned.key");
        let cert_path = tmpdir.path.join("selfsigned.pem");

        let cert = generate_simple_self_signed(subject_alt_names)?;

        let key_pem = cert.serialize_private_key_pem();
        let cert_pem = cert.serialize_pem()?;

        fs::write(&cert_path, &key_pem)?;
        fs::write(&key_path, &cert_pem)?;

        Ok(Certificates {
            key_path, cert_path, tmpdir, key_pem, cert_pem
        })
    }

    pub fn path(&self) -> String {
        self.tmpdir.path.to_str().unwrap().to_owned()
    }
}