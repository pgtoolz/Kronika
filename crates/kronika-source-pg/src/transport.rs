//! Verified TLS for monitoring sessions and their cancellation connections.

use std::sync::Arc;

use anyhow::Result;
use rustls::{ClientConfig, RootCertStore};
use tokio_postgres::{CancelToken, Client, Config};
use tokio_postgres_rustls::MakeRustlsConnect;

/// A configured CA bundle could not be read or decoded; contains no supplied data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaConfigError;

impl std::fmt::Display for CaConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KRONIKA_PG_SSL_ROOT_CERT must name a readable, valid PEM CA bundle")
    }
}

impl std::error::Error for CaConfigError {}

/// Shared certificate policy for `PostgreSQL` metrics and log discovery.
#[derive(Clone)]
pub struct Transport {
    connector: MakeRustlsConnect,
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport").finish_non_exhaustive()
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::from_roots(public_roots())
    }
}

impl Transport {
    /// Read an optional PEM CA bundle from `KRONIKA_PG_SSL_ROOT_CERT`.
    ///
    /// Without it, use the compiled Mozilla public CA roots. A supplied bundle
    /// replaces that root set. Certificate and hostname verification stay enabled.
    ///
    /// # Errors
    /// Returns an error for an unreadable or invalid configured CA bundle.
    pub fn from_env() -> Result<Self> {
        let Some(path) = std::env::var_os("KRONIKA_PG_SSL_ROOT_CERT") else {
            return Ok(Self::default());
        };
        let pem = std::fs::read(path).map_err(|_error| CaConfigError)?;
        Self::from_pem(&pem)
    }

    /// Build a verified transport using only the supplied PEM CA certificates.
    ///
    /// # Errors
    /// Returns an error for an empty bundle or an invalid certificate.
    pub fn from_pem(pem: &[u8]) -> Result<Self> {
        let mut roots = RootCertStore::empty();
        let mut input = pem;
        for certificate in rustls_pemfile::certs(&mut input) {
            roots
                .add(certificate.map_err(|_error| CaConfigError)?)
                .map_err(|_error| CaConfigError)?;
        }
        if roots.is_empty() {
            return Err(CaConfigError.into());
        }
        Ok(Self::from_roots(roots))
    }

    fn from_roots(roots: RootCertStore) -> Self {
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("the ring provider supports the enabled TLS1.2 and TLS1.3 defaults")
                .with_root_certificates(roots)
                .with_no_client_auth();
        Self {
            connector: MakeRustlsConnect::new(config),
        }
    }

    /// Connect using the DSN SSL mode and verified TLS when it is negotiated.
    ///
    /// `sslmode=disable` remains plaintext. `require` refuses a non-TLS server;
    /// `prefer` follows `PostgreSQL` negotiation and may use plaintext.
    ///
    /// # Errors
    /// Returns a `PostgreSQL` connection, certificate, hostname or protocol error.
    pub async fn connect(
        &self,
        config: &Config,
    ) -> Result<
        (
            Client,
            impl Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static + use<>,
        ),
        tokio_postgres::Error,
    > {
        config.connect(self.connector.clone()).await
    }

    /// Send cancellation with the same certificate policy as the original session.
    ///
    /// # Errors
    /// Returns a cancellation connection or TLS validation error.
    pub async fn cancel(&self, token: CancelToken) -> Result<(), tokio_postgres::Error> {
        token.cancel_query(self.connector.clone()).await
    }
}

fn public_roots() -> RootCertStore {
    webpki_roots::TLS_SERVER_ROOTS.iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::Transport;

    #[test]
    fn empty_or_invalid_custom_ca_never_disables_verification() {
        assert!(Transport::from_pem(b"").is_err());
        let error = Transport::from_pem(b"not a certificate").expect_err("invalid CA fails closed");
        assert!(error.is::<super::CaConfigError>());
        assert!(error.to_string().contains("KRONIKA_PG_SSL_ROOT_CERT"));
        assert!(
            Transport::from_pem(
                b"-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----\n"
            )
            .is_err()
        );
    }
}
