//! Delivering the outbox over SMTP.
//!
//! The server is given an SMTP URL such as `smtp://user:password@mail.example.com:587`
//! (STARTTLS) or `smtps://...:465` (TLS from the start). Without one, mail stays in the outbox.

use std::time::Duration;

use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use sqlx::SqlitePool;

use crate::store::outbox;
use crate::time::now_ms;

/// How often the sender looks for new mail.
const INTERVAL: Duration = Duration::from_secs(5);
/// Sent mail is kept this long, in milliseconds (one week).
const KEEP_SENT_MS: i64 = 7 * 24 * 3_600_000;

pub struct Sender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Sender {
    /// Reads an SMTP URL and the sender's address.
    pub fn new(smtp_url: &str, from: &str) -> Result<Sender, String> {
        let url = url_parts(smtp_url)?;
        let mut builder = match url.scheme.as_str() {
            "smtps" => AsyncSmtpTransport::<Tokio1Executor>::relay(&url.host),
            "smtp" => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&url.host),
            // Plain SMTP without TLS, for a local relay or a test server.
            "smtp+plain" => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &url.host,
            )),
            other => {
                return Err(format!(
                    "Unsupported mail scheme {other}; use smtp, smtps, or smtp+plain"
                ));
            }
        }
        .map_err(|e| e.to_string())?;
        if let Some(port) = url.port {
            builder = builder.port(port);
        }
        if let Some((user, password)) = url.credentials {
            builder = builder.credentials(Credentials::new(user, password));
        }
        let from = from
            .parse()
            .map_err(|e| format!("Invalid sender address {from}: {e}"))?;
        Ok(Sender {
            transport: builder.build(),
            from,
        })
    }

    async fn send(&self, mail: &outbox::Mail) -> Result<(), String> {
        let to: Mailbox = mail
            .to_email
            .parse()
            .map_err(|e| format!("Invalid address: {e}"))?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(&mail.subject)
            .body(mail.body.clone())
            .map_err(|e| e.to_string())?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Sends pending mail forever, retrying failures a few times.
    pub async fn run(self, db: SqlitePool) {
        loop {
            match outbox::pending(&db, 50).await {
                Ok(batch) => {
                    for mail in &batch {
                        let result = match self.send(mail).await {
                            Ok(()) => outbox::mark_sent(&db, mail.id).await,
                            Err(error) => {
                                tracing::warn!(to = %mail.to_email, %error, "could not send mail");
                                outbox::mark_failed(&db, mail.id, &error).await
                            }
                        };
                        if let Err(error) = result {
                            tracing::error!(%error, "could not update the outbox");
                        }
                    }
                }
                Err(error) => tracing::error!(%error, "could not read the outbox"),
            }
            let _ = outbox::prune(&db, now_ms() - KEEP_SENT_MS).await;
            tokio::time::sleep(INTERVAL).await;
        }
    }
}

struct UrlParts {
    scheme: String,
    host: String,
    port: Option<u16>,
    credentials: Option<(String, String)>,
}

/// Splits `scheme://user:password@host:port`. User and password may be percent-encoded.
fn url_parts(text: &str) -> Result<UrlParts, String> {
    let (scheme, rest) = text
        .split_once("://")
        .ok_or_else(|| format!("Invalid SMTP URL: {text}"))?;
    let rest = rest.trim_end_matches('/');
    let (credentials, address) = match rest.rsplit_once('@') {
        Some((auth, address)) => {
            let (user, password) = auth.split_once(':').unwrap_or((auth, ""));
            let decode = |s: &str| {
                percent_encoding::percent_decode_str(s)
                    .decode_utf8_lossy()
                    .to_string()
            };
            (Some((decode(user), decode(password))), address)
        }
        None => (None, rest),
    };
    let (host, port) = match address.rsplit_once(':') {
        Some((host, port)) => (
            host,
            Some(
                port.parse()
                    .map_err(|_| format!("Invalid SMTP port in {text}"))?,
            ),
        ),
        None => (address, None),
    };
    if host.is_empty() {
        return Err(format!("Invalid SMTP URL: {text}"));
    }
    Ok(UrlParts {
        scheme: scheme.to_lowercase(),
        host: host.to_string(),
        port,
        credentials,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_smtp_urls() {
        let url = url_parts("smtp://me%40x.com:p%3Ass@mail.x.com:587").unwrap();
        assert_eq!(url.host, "mail.x.com");
        assert_eq!(url.port, Some(587));
        assert_eq!(url.credentials, Some(("me@x.com".into(), "p:ss".into())));
        let plain = url_parts("smtp+plain://localhost:1025").unwrap();
        assert_eq!(
            (plain.scheme.as_str(), plain.credentials),
            ("smtp+plain", None)
        );
        assert!(url_parts("nonsense").is_err());
    }
}
