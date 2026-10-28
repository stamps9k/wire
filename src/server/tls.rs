//! TLS setup: loads the server's certificate chain and private key from PEM
//! files and builds the [`TlsAcceptor`] that runs the handshake on each TLS
//! connection, using rustls with the *ring* crypto provider.

use std::io;
use std::path::Path;
use std::sync::Arc;

use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::rustls::pki_types::PrivateKeyDer;
use tokio_rustls::rustls::pki_types::pem::PemObject;

/// Builds the [`TlsAcceptor`] for the TLS listener from the PEM files at
/// `cert_path` and `key_path`.
///
/// `cert_path` holds the certificate chain, leaf first and then the
/// intermediates, as in certbot's `fullchain.pem`. Without the intermediates
/// browsers cannot verify the leaf. `key_path` holds the private key in any
/// of the PEM formats certbot may write (PKCS#8, SEC1 or PKCS#1); the first
/// key in the file is used.
///
/// The configuration uses rustls's safe default protocol versions (TLS 1.2
/// and 1.3), asks clients for no certificate, and offers only `http/1.1`
/// through ALPN, since the server does not speak HTTP/2. The files are read
/// once, so a renewed certificate takes effect when the server restarts.
///
/// # Errors
///
/// Returns an error naming the file at fault if either file cannot be read
/// or parsed, or if `cert_path` holds no certificate, and an error naming
/// both files if rustls rejects the pair, for example when the key does not
/// belong to the certificate.
pub fn acceptor(cert_path: &Path, key_path: &Path) -> io::Result<TlsAcceptor> {
  let certs: Vec<CertificateDer<'static>> =
    CertificateDer::pem_file_iter(cert_path)
      .map_err(|e| io::Error::other(format!("{}: {e}", cert_path.display())))?
      .collect::<Result<Vec<_>, _>>()
      .map_err(|e| io::Error::other(format!("{}: {e}", cert_path.display())))?;

  if certs.is_empty() {
    return Err(io::Error::other(format!(
      "{}: no certificate found",
      cert_path.display()
    )));
  }

  let key = PrivateKeyDer::from_pem_file(key_path)
    .map_err(|e| io::Error::other(format!("{}: {e}", key_path.display())))?;

  let mut config = ServerConfig::builder_with_provider(Arc::new(
    tokio_rustls::rustls::crypto::ring::default_provider(),
  ))
  .with_safe_default_protocol_versions()
  .map_err(|e| io::Error::other(format!("tls config: {e}")))?
  .with_no_client_auth()
  .with_single_cert(certs, key)
  .map_err(|e| {
    io::Error::other(format!(
      "{} and {}: {e}",
      cert_path.display(),
      key_path.display()
    ))
  })?;

  config.alpn_protocols = vec![b"http/1.1".to_vec()];

  Ok(TlsAcceptor::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
  use super::*;

  use std::error::Error;
  use std::fs;
  use std::path::PathBuf;
  use std::process;
  use std::sync::atomic::AtomicU64;
  use std::sync::atomic::Ordering;

  use rcgen::BasicConstraints;
  use rcgen::CertificateParams;
  use rcgen::IsCa;
  use rcgen::KeyPair;

  type TestResult = Result<(), Box<dyn Error>>;

  /// A directory of its own under the system's temporary directory, removed
  /// with everything in it when dropped. The name holds the process id and a
  /// counter, so tests running in parallel never share one.
  struct TempDir(PathBuf);

  impl TempDir {
    fn new() -> io::Result<Self> {
      static NEXT: AtomicU64 = AtomicU64::new(0);
      let n = NEXT.fetch_add(1, Ordering::Relaxed);
      let path =
        std::env::temp_dir().join(format!("wire-tls-{}-{n}", process::id()));
      fs::create_dir_all(&path)?;
      Ok(Self(path))
    }

    /// Writes `contents` to `name` in the directory and returns its path.
    fn file(&self, name: &str, contents: &str) -> io::Result<PathBuf> {
      let path = self.0.join(name);
      fs::write(&path, contents)?;
      Ok(path)
    }
  }

  impl Drop for TempDir {
    fn drop(&mut self) {
      let _ = fs::remove_dir_all(&self.0);
    }
  }

  /// A self-signed certificate for `wire.test` and its key, both as PEM.
  fn self_signed() -> Result<(String, String), Box<dyn Error>> {
    let key = KeyPair::generate()?;
    let cert =
      CertificateParams::new(vec!["wire.test".into()])?.self_signed(&key)?;
    Ok((cert.pem(), key.serialize_pem()))
  }

  /// Asserts that `result` failed with a message containing `path`.
  fn assert_names(result: io::Result<TlsAcceptor>, path: &Path) -> TestResult {
    let Err(e) = result else {
      return Err("expected an error".into());
    };
    let shown = path.display().to_string();
    assert!(e.to_string().contains(&shown), "{e} does not name {shown}");
    Ok(())
  }

  #[test]
  fn builds_an_acceptor_from_a_matching_certificate_and_key() -> TestResult {
    let dir = TempDir::new()?;
    let (cert, key) = self_signed()?;
    let cert_path = dir.file("cert.pem", &cert)?;
    let key_path = dir.file("key.pem", &key)?;

    acceptor(&cert_path, &key_path)?;
    Ok(())
  }

  #[test]
  fn accepts_a_chain_with_an_intermediate_after_the_leaf() -> TestResult {
    let dir = TempDir::new()?;
    let ca_key = KeyPair::generate()?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key)?;
    let leaf_key = KeyPair::generate()?;
    let leaf = CertificateParams::new(vec!["wire.test".into()])?
      .signed_by(&leaf_key, &ca, &ca_key)?;
    // Leaf first, then the issuer, as in certbot's fullchain.pem.
    let chain = format!("{}{}", leaf.pem(), ca.pem());
    let cert_path = dir.file("fullchain.pem", &chain)?;
    let key_path = dir.file("key.pem", &leaf_key.serialize_pem())?;

    acceptor(&cert_path, &key_path)?;
    Ok(())
  }

  #[test]
  fn offers_only_http_1_1_through_alpn() -> TestResult {
    let dir = TempDir::new()?;
    let (cert, key) = self_signed()?;
    let cert_path = dir.file("cert.pem", &cert)?;
    let key_path = dir.file("key.pem", &key)?;

    let acceptor = acceptor(&cert_path, &key_path)?;

    assert_eq!(acceptor.config().alpn_protocols, vec![b"http/1.1".to_vec()]);
    Ok(())
  }

  #[test]
  fn a_missing_certificate_file_is_named_in_the_error() -> TestResult {
    let dir = TempDir::new()?;
    let (_, key) = self_signed()?;
    let cert_path = dir.0.join("missing.pem");
    let key_path = dir.file("key.pem", &key)?;

    assert_names(acceptor(&cert_path, &key_path), &cert_path)
  }

  #[test]
  fn a_missing_key_file_is_named_in_the_error() -> TestResult {
    let dir = TempDir::new()?;
    let (cert, _) = self_signed()?;
    let cert_path = dir.file("cert.pem", &cert)?;
    let key_path = dir.0.join("missing.pem");

    assert_names(acceptor(&cert_path, &key_path), &key_path)
  }

  #[test]
  fn a_certificate_file_with_no_certificate_is_refused() -> TestResult {
    let dir = TempDir::new()?;
    let (_, key) = self_signed()?;
    let cert_path = dir.file("cert.pem", "not a certificate\n")?;
    let key_path = dir.file("key.pem", &key)?;

    let result = acceptor(&cert_path, &key_path);

    let Err(e) = &result else {
      return Err("expected an error".into());
    };
    assert!(e.to_string().contains("no certificate found"), "{e}");
    assert_names(result, &cert_path)
  }

  #[test]
  fn a_key_file_with_no_key_is_named_in_the_error() -> TestResult {
    let dir = TempDir::new()?;
    let (cert, _) = self_signed()?;
    let cert_path = dir.file("cert.pem", &cert)?;
    // A certificate where the key should be: valid PEM, but no private key.
    let key_path = dir.file("key.pem", &cert)?;

    assert_names(acceptor(&cert_path, &key_path), &key_path)
  }

  #[test]
  fn a_garbled_certificate_is_named_in_the_error() -> TestResult {
    let dir = TempDir::new()?;
    let (_, key) = self_signed()?;
    let cert_path = dir.file(
      "cert.pem",
      "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n",
    )?;
    let key_path = dir.file("key.pem", &key)?;

    assert_names(acceptor(&cert_path, &key_path), &cert_path)
  }

  #[test]
  fn a_key_from_another_certificate_is_refused_naming_both_files() -> TestResult
  {
    let dir = TempDir::new()?;
    let (cert, _) = self_signed()?;
    let (_, other_key) = self_signed()?;
    let cert_path = dir.file("cert.pem", &cert)?;
    let key_path = dir.file("key.pem", &other_key)?;

    let result = acceptor(&cert_path, &key_path);

    let Err(e) = &result else {
      return Err("a mismatched key was accepted".into());
    };
    assert!(
      e.to_string().contains(&key_path.display().to_string()),
      "{e}"
    );
    assert_names(result, &cert_path)
  }
}
