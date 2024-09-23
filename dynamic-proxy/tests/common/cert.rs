use rcgen::generate_simple_self_signed;
use rustls::crypto::ring::sign::any_supported_type;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::{pki_types::PrivateKeyDer, sign::CertifiedKey};
use std::sync::Arc;

const CERTIFICATE_SUBJECT_ALT_NAME: &str = "plane.test";

#[derive(Debug)]
pub struct StaticCertificateResolver {
    certified_key: Arc<CertifiedKey>,
}

#[allow(unused)]
impl StaticCertificateResolver {
    pub fn new() -> Self {
        let subject_alt_names = vec![CERTIFICATE_SUBJECT_ALT_NAME.to_string()];

        let rcgen::CertifiedKey { cert, key_pair } =
            generate_simple_self_signed(subject_alt_names).unwrap();

        let key = PrivateKeyDer::try_from(key_pair.serialized_der())
            .expect("Could not convert key pair to der");

        let key = any_supported_type(&key).expect("Could not convert key to supported type");

        let cert = cert.der().clone();

        let certified_key = Arc::new(CertifiedKey::new(vec![cert], key));

        Self { certified_key }
    }

    pub fn certificate(&self) -> reqwest::Certificate {
        let der = &self.certified_key.cert[0];
        reqwest::Certificate::from_der(der).unwrap()
    }

    pub fn hostname(&self) -> String {
        CERTIFICATE_SUBJECT_ALT_NAME.to_string()
    }
}

impl ResolvesServerCert for StaticCertificateResolver {
    fn resolve(&self, _client_hello: ClientHello) -> Option<Arc<CertifiedKey>> {
        Some(self.certified_key.clone())
    }
}
