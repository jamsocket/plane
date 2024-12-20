use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use pem::Pem;
use plane_common::log_types::LoggableTime;
use plane_dynamic_proxy::rustls::{
    crypto::aws_lc_rs::sign::any_supported_type, pki_types::PrivateKeyDer,
};
use plane_dynamic_proxy::tokio_rustls::rustls::sign::CertifiedKey;
use rustls_pki_types::CertificateDer;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use std::{fs::Permissions, io, os::unix::fs::PermissionsExt, path::Path};
use time::OffsetDateTime;
use x509_parser::{certificate::X509Certificate, oid_registry::asn1_rs::FromDer};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializedCertificatePair {
    cert: String,
    key: String,
}

fn load_cert_ders(pem: &str) -> Result<Vec<CertificateDer>> {
    let mut bytes = pem.as_bytes();
    let results: Result<Vec<_>, _> = rustls_pemfile::certs(&mut bytes).collect();
    let results = results?;

    let results: Vec<_> = results.into_iter().collect();

    Ok(results)
}

fn load_key_der(pem: &str) -> Result<PrivateKeyDer> {
    let mut bytes = pem.as_bytes();
    let results: Result<_, io::Error> = rustls_pemfile::private_key(&mut bytes);

    let result = results?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("No private key found."))?;

    Ok(result)
}

#[derive(Clone)]
pub struct CertificatePair {
    pub certified_key: CertifiedKey,
    pub private_key_der: Vec<u8>,
    pub common_name: String,
    pub validity_start: LoggableTime,
    pub validity_end: LoggableTime,
}

fn convert_time(time: OffsetDateTime) -> LoggableTime {
    let t: SystemTime = time.into();
    let t: DateTime<Utc> = t.into();
    LoggableTime(t)
}

impl CertificatePair {
    fn new(key: &PrivateKeyDer, certs: Vec<CertificateDer>) -> Result<Self> {
        let private_key_der = key.secret_der().to_vec();

        let (_, parsed_cert) = X509Certificate::from_der(
            certs
                .first()
                .ok_or_else(|| anyhow!("Error getting first cert."))?,
        )?;
        let common_name = parsed_cert
            .subject()
            .iter_common_name()
            .next()
            .ok_or(anyhow!("No common name found"))?
            .as_str()?
            .to_string();

        let validity = parsed_cert.validity();
        let validity_start = validity.not_before.to_datetime();
        let validity_end = validity.not_after.to_datetime();

        // Convert the Vec<CertificateDer<'_>> to a Vec<CertificateDer<'static>>
        // by copying the certificate data.
        let certs = certs
            .into_iter()
            .map(|cert| CertificateDer::from(cert.to_vec()))
            .collect();

        let key = any_supported_type(key)?;

        let certified_key = CertifiedKey::new(certs, key);

        Ok(Self {
            certified_key,
            common_name,
            private_key_der,
            validity_start: convert_time(validity_start),
            validity_end: convert_time(validity_end),
        })
    }

    pub fn from_raw_ders(pkey_der: &[u8], cert_ders: &[Vec<u8>]) -> Result<Self> {
        let certs = cert_ders
            .iter()
            .map(|cert_der| CertificateDer::from(cert_der.to_vec()))
            .collect();

        let key = PrivateKeyDer::try_from(pkey_der.to_vec())
            .map_err(|e| anyhow!("Error converting private key to der: {}", e))?;

        Self::new(&key, certs)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let cert_pair: SerializedCertificatePair = serde_json::from_str(&contents)?;
        let certs = load_cert_ders(&cert_pair.cert)?;
        let key = load_key_der(&cert_pair.key)?;

        Self::new(&key, certs)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let cert_ders: Vec<&[u8]> = self
            .certified_key
            .cert
            .iter()
            .map(|cert| cert.as_ref())
            .collect();
        let cert = pem::encode_many(
            cert_ders
                .into_iter()
                .map(|cert_der| Pem::new("CERTIFICATE", cert_der))
                .collect::<Vec<_>>()
                .as_slice(),
        );

        let key = pem::encode(&Pem::new("RSA PRIVATE KEY", self.private_key_der.clone()));

        let cert_pair = SerializedCertificatePair { cert, key };

        let contents = serde_json::to_string_pretty(&cert_pair)?;

        // If the file does not exist, we want to make sure it is created with specific
        // permissions instead of the default system umask.
        if !path.exists() {
            std::fs::File::create(path)?;
            let permissions = Permissions::from_mode(0o600);
            std::fs::set_permissions(path, permissions)?;
        }

        std::fs::write(path, contents)?;

        Ok(())
    }
}
