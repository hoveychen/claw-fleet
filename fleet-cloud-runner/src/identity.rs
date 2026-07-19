use std::io::BufReader;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};

pub fn client_config(
    ca_pem: &[u8],
    certificate_pem: &[u8],
    private_key_pem: &[u8],
) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut roots = rustls::RootCertStore::empty();
    let ca = rustls_pemfile::certs(&mut BufReader::new(ca_pem)).collect::<Result<Vec<_>, _>>()?;
    let (added, ignored) = roots.add_parsable_certificates(ca);
    anyhow::ensure!(added > 0 && ignored == 0, "invalid Runner CA certificate");
    let chain: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut BufReader::new(certificate_pem)).collect::<Result<_, _>>()?;
    anyhow::ensure!(!chain.is_empty(), "Runner client certificate is empty");
    let key: PrivateKeyDer<'static> =
        rustls_pemfile::private_key(&mut BufReader::new(private_key_pem))?
            .ok_or_else(|| anyhow::anyhow!("Runner private key is empty"))?;
    Ok(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(chain, key)?,
    ))
}
