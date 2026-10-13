//! The `tessera` command: run the server and manage users from the command line.

use std::net::SocketAddr;

use clap::{Parser, Subcommand};
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
    }) {
        Command::Serve { addr, base_url } => {
            bootstrap_admin(&pool).await?;
            let base_url = base_url
                .unwrap_or_else(|| format!("http://{addr}"))
                .trim_end_matches('/')
                .to_string();
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!("Tessera listening on {base_url}");
            axum::serve(listener, app(AppState { db: pool, base_url }))
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
