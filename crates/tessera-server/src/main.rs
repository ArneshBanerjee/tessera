//! The `tessera` command: run the server and manage users from the command line.

use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use tessera_server::mail::Sender;
use tessera_server::store::users::{self, NewUser};
use tessera_server::{AppState, app, auth, db};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "tessera",
    version,
    about = "An open-source clone of the Atlassian suite"
)]
struct Cli {
    /// SQLite database URL.
    #[arg(
        long,
        env = "TESSERA_DATABASE",
        default_value = "sqlite://tessera.db",
        global = true
    )]
    database: String,
    /// Directory for attachments and other files.
    #[arg(
        long,
        env = "TESSERA_DATA_DIR",
        default_value = "tessera-data",
        global = true
    )]
    data_dir: std::path::PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server (the default).
    Serve {
        /// Address to listen on.
        #[arg(long, env = "TESSERA_ADDR", default_value = "127.0.0.1:8080")]
        addr: SocketAddr,
        /// Public URL of the server. Defaults to http://<addr>.
        #[arg(long, env = "TESSERA_BASE_URL")]
        base_url: Option<String>,
        /// SMTP server for notifications, such as smtp://user:password@mail.example.com:587.
        #[arg(long, env = "TESSERA_SMTP_URL")]
        smtp_url: Option<String>,
        /// The From address of notifications.
        #[arg(
            long,
            env = "TESSERA_MAIL_FROM",
            default_value = "Tessera <tessera@localhost>"
        )]
        mail_from: String,
    },
    /// Create a user.
    CreateUser {
        #[arg(long)]
        email: String,
        #[arg(long)]
        name: String,
        #[arg(long, env = "TESSERA_PASSWORD")]
        password: String,
        /// Make the user a site administrator.
        #[arg(long)]
        admin: bool,
    },
    /// Create an API token for a user and print it.
    CreateToken {
        #[arg(long)]
        email: String,
        #[arg(long, default_value = "cli")]
        label: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cli = Cli::parse();
    let pool = db::connect(&cli.database).await?;

    match cli.command.unwrap_or(Command::Serve {
        addr: std::env::var("TESSERA_ADDR")
            .ok()
            .and_then(|a| a.parse().ok())
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 8080))),
        base_url: std::env::var("TESSERA_BASE_URL").ok(),
        smtp_url: std::env::var("TESSERA_SMTP_URL").ok(),
        mail_from: std::env::var("TESSERA_MAIL_FROM")
            .unwrap_or_else(|_| "Tessera <tessera@localhost>".into()),
    }) {
        Command::Serve {
            addr,
            base_url,
            smtp_url,
            mail_from,
        } => {
            bootstrap_admin(&pool).await?;
            let mail_enabled = match smtp_url {
                Some(url) => {
                    let sender = Sender::new(&url, &mail_from)?;
                    tokio::spawn(sender.run(pool.clone()));
                    tracing::info!("Sending notifications as {mail_from}");
                    true
                }
                None => {
                    tracing::info!("No SMTP server set; email notifications are off");
                    false
                }
            };
            let base_url = base_url
                .unwrap_or_else(|| format!("http://{addr}"))
                .trim_end_matches('/')
                .to_string();
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!("Tessera listening on {base_url}");
            let data_dir = cli.data_dir;
            axum::serve(
                listener,
                app(AppState {
                    db: pool,
                    base_url,
                    data_dir,
                    mail_enabled,
                }),
            )
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
        }
        Command::CreateUser {
            email,
            name,
            password,
            admin,
        } => {
            let user = users::create(
                &pool,
                NewUser {
                    email: &email,
                    display_name: &name,
                    password_hash: Some(auth::hash_password(&password)),
                    is_admin: admin,
                },
            )
            .await?;
            println!("Created {} ({})", user.email, user.account_id);
        }
        Command::CreateToken { email, label } => {
            let user = users::by_email(&pool, &email)
                .await?
                .ok_or_else(|| format!("No user with email {email}"))?;
            println!("{}", auth::create_api_token(&pool, user.id, &label).await?);
        }
    }
    Ok(())
}

/// On a fresh database, creates the first administrator from `TESSERA_ADMIN_EMAIL` and
/// `TESSERA_ADMIN_PASSWORD`, if both are set.
async fn bootstrap_admin(pool: &sqlx::SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    if users::count(pool).await? > 0 {
        return Ok(());
    }
    let (Ok(email), Ok(password)) = (
        std::env::var("TESSERA_ADMIN_EMAIL"),
        std::env::var("TESSERA_ADMIN_PASSWORD"),
    ) else {
        tracing::warn!(
            "No users yet. Run `tessera create-user --admin`, or set TESSERA_ADMIN_EMAIL and TESSERA_ADMIN_PASSWORD."
        );
        return Ok(());
    };
    users::create(
        pool,
        NewUser {
            email: &email,
            display_name: "Administrator",
            password_hash: Some(auth::hash_password(&password)),
            is_admin: true,
        },
    )
    .await?;
    tracing::info!("Created administrator {email}");
    Ok(())
}
