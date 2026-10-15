//! Cliente TLS (rustls + raíces de Mozilla). Con `allow_self_signed` acepta
//! certificados autofirmados, para servidores propios sin Let's Encrypt.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use std::sync::{Arc, OnceLock};
use tokio_rustls::TlsConnector;

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn connector(allow_self_signed: bool) -> TlsConnector {
    TlsConnector::from(client_config(allow_self_signed))
}

/// Configuración rustls compartida (IMAP y clientes HTTP como reqwest).
pub fn client_config(allow_self_signed: bool) -> Arc<ClientConfig> {
    static STRICT: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    static LAX: OnceLock<Arc<ClientConfig>> = OnceLock::new();
    let cell = if allow_self_signed { &LAX } else { &STRICT };
    cell.get_or_init(|| {
        let builder = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .expect("versiones TLS por defecto");
        let config = if allow_self_signed {
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAny))
                .with_no_client_auth()
        } else {
            let roots = RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            builder.with_root_certificates(roots).with_no_client_auth()
        };
        Arc::new(config)
    })
    .clone()
}

/// Cliente HTTP (reqwest) con rustls/ring y raíces de Mozilla.
pub fn http_client(allow_self_signed: bool) -> reqwest::Client {
    reqwest::Client::builder()
        .use_preconfigured_tls((*client_config(allow_self_signed)).clone())
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(concat!("rvmail/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("cliente HTTP")
}

pub fn server_name(host: &str) -> anyhow::Result<ServerName<'static>> {
    ServerName::try_from(host.to_string())
        .map_err(|_| anyhow::anyhow!("Nombre de servidor no válido: {host}"))
}

#[derive(Debug)]
struct AcceptAny;

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
