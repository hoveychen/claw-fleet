use rcgen::{
    Certificate, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair,
};
use sha2::{Digest, Sha256};

pub struct RunnerIdentityIssuer {
    ca: Certificate,
    ca_key: KeyPair,
    ca_pem: String,
}

pub struct IssuedIdentity {
    pub certificate_pem: String,
    pub private_key_pem: String,
    pub ca_certificate_pem: String,
    pub fingerprint: Vec<u8>,
}

impl RunnerIdentityIssuer {
    pub fn from_pem(ca_pem: &str, ca_key_pem: &str) -> anyhow::Result<Self> {
        let params = CertificateParams::from_ca_cert_pem(ca_pem)?;
        let ca_key = KeyPair::from_pem(ca_key_pem)?;
        let ca = params.self_signed(&ca_key)?;
        Ok(Self {
            ca,
            ca_key,
            ca_pem: ca_pem.to_owned(),
        })
    }

    pub fn issue(&self, runner_id: &str) -> anyhow::Result<IssuedIdentity> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(Vec::<String>::new())?;
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, runner_id);
        params.distinguished_name = name;
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let certificate = params.signed_by(&key, &self.ca, &self.ca_key)?;
        Ok(IssuedIdentity {
            certificate_pem: certificate.pem(),
            private_key_pem: key.serialize_pem(),
            ca_certificate_pem: self.ca_pem.clone(),
            fingerprint: Sha256::digest(certificate.der().as_ref()).to_vec(),
        })
    }
}
