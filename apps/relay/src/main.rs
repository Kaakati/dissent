use anyhow::{Context, Result, bail};
use clap::Parser;
use dissent_relay::{Server, Tls, client_url, dev_config, tls_config};
use std::net::SocketAddr;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

/// Run an open relay for Dissent. Pick one mode: `--dev`, `--acme-email`, or `--cert` with `--key`.
#[derive(Debug, Parser)]
#[command(name = "dissent-relay", version, about)]
struct Cli {
    /// HTTP address to bind. With TLS, it serves only the captive-portal probe.
    #[arg(long, default_value = "0.0.0.0:3340")]
    bind: SocketAddr,

    /// HTTPS address to bind. TLS modes only.
    #[arg(long, default_value = "0.0.0.0:443")]
    https_bind: SocketAddr,

    /// Serve plain HTTP with no TLS. For local testing only.
    #[arg(long, conflicts_with_all = ["acme_email", "cert", "key"])]
    dev: bool,

    /// Public DNS name of this relay. Required with --acme-email. Also used in the printed URL.
    #[arg(long)]
    hostname: Option<String>,

    /// Email for the Let's Encrypt account. Gets a certificate for --hostname.
    #[arg(long, requires = "hostname", conflicts_with_all = ["cert", "key"])]
    acme_email: Option<String>,

    /// Use the Let's Encrypt staging directory. Its certificates are not trusted by browsers.
    #[arg(long, requires = "acme_email")]
    acme_staging: bool,

    /// Directory caching the ACME account and issued certificates.
    #[arg(long, default_value = "acme-cache")]
    acme_cache: PathBuf,

    /// PEM certificate chain. Use with --key.
    #[arg(long, requires = "key", conflicts_with = "acme_email")]
    cert: Option<PathBuf>,

    /// PEM private key. Use with --cert.
    #[arg(long, requires = "cert")]
    key: Option<PathBuf>,
}

/// The mode the flags select.
#[derive(Debug, PartialEq)]
enum Mode {
    Dev,
    Tls(Tls),
}

impl Cli {
    fn mode(&self) -> Result<Mode> {
        if self.dev {
            return Ok(Mode::Dev);
        }
        if let Some(email) = &self.acme_email {
            let Some(hostname) = &self.hostname else {
                bail!("--acme-email needs --hostname");
            };
            return Ok(Mode::Tls(Tls::Acme {
                hostname: hostname.clone(),
                email: email.clone(),
                staging: self.acme_staging,
                cache_dir: self.acme_cache.clone(),
            }));
        }
        if let (Some(cert), Some(key)) = (&self.cert, &self.key) {
            return Ok(Mode::Tls(Tls::Files {
                cert: cert.clone(),
                key: key.clone(),
            }));
        }
        bail!("choose a mode: --dev, --acme-email with --hostname, or --cert with --key");
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    // Both TLS providers may be linked, so pick ring explicitly, as the iroh relay binary does.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = Cli::parse();
    let (config, tls) = match cli.mode()? {
        Mode::Dev => (dev_config(cli.bind), false),
        Mode::Tls(tls) => (
            tls_config(cli.bind, cli.https_bind, tls)
                .await
                .context("configure TLS")?,
            true,
        ),
    };
    let mut server = Server::spawn(config).await.context("start relay")?;
    println!(
        "Relay listening. Put this URL in a Dissent client: {}",
        client_url(&server, cli.hostname.as_deref(), cli.https_bind, tls)
    );

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = server.join() => {}
    }
    server.shutdown().await.context("shut down relay")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("dissent-relay").chain(args.iter().copied()))
            .expect("valid arguments")
    }

    #[test]
    fn defaults_bind_3340() {
        let cli = parse(&["--dev"]);
        assert_eq!(cli.bind, "0.0.0.0:3340".parse().unwrap());
        assert_eq!(cli.mode().unwrap(), Mode::Dev);
    }

    #[test]
    fn acme_needs_hostname() {
        assert!(Cli::try_parse_from(["dissent-relay", "--acme-email", "a@b.c"]).is_err());
        let cli = parse(&["--acme-email", "a@b.c", "--hostname", "relay.example.org"]);
        assert_eq!(
            cli.mode().unwrap(),
            Mode::Tls(Tls::Acme {
                hostname: "relay.example.org".into(),
                email: "a@b.c".into(),
                staging: false,
                cache_dir: PathBuf::from("acme-cache"),
            })
        );
    }

    #[test]
    fn cert_needs_key_and_pairs_with_files_mode() {
        assert!(Cli::try_parse_from(["dissent-relay", "--cert", "c.pem"]).is_err());
        let cli = parse(&["--cert", "c.pem", "--key", "k.pem"]);
        assert_eq!(
            cli.mode().unwrap(),
            Mode::Tls(Tls::Files {
                cert: "c.pem".into(),
                key: "k.pem".into(),
            })
        );
    }

    #[test]
    fn dev_conflicts_with_tls_flags() {
        assert!(
            Cli::try_parse_from(["dissent-relay", "--dev", "--cert", "c.pem", "--key", "k.pem"])
                .is_err()
        );
    }

    #[test]
    fn no_mode_is_an_error() {
        let cli = parse(&[]);
        assert!(cli.mode().is_err());
    }
}
