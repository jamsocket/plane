use std::{path::PathBuf, fs};
use anyhow::Result;
use rcgen::generate_simple_self_signed;
use super::tempdir::TemporaryDirectory;

pub struct Certificates {
    key_path: PathBuf,
    cert_path: PathBuf,
    tmpdir: TemporaryDirectory,
}

impl Certificates {
    pub fn new(subject_alt_names: Vec<String>) -> Result<Certificates> {
        let tmpdir = TemporaryDirectory::new()?;

        let key_path = tmpdir.path.join("selfsigned.key");
        let cert_path = tmpdir.path.join("selfsigned.pem");

        let cert = generate_simple_self_signed(subject_alt_names)?;

        fs::write(&cert_path, cert.serialize_pem()?)?;
        fs::write(&key_path, cert.serialize_private_key_pem())?;

        Ok(Certificates {
            key_path, cert_path, tmpdir
        })
    }

    pub fn path(&self) -> String {
        self.tmpdir.path.to_str().unwrap().to_owned()
    }
}