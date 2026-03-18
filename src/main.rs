mod packet_ids;
mod packets;
mod protocol;
mod server;
mod world;

use tokio::net::TcpListener;
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing subscriber
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mincraft_server=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let listener = TcpListener::bind("127.0.0.1:25565").await?;
    info!("Server listening on 127.0.0.1:25565");

    loop {
        let (socket, addr) = listener.accept().await?;
        info!(address = %addr, "New connection");

        tokio::spawn(async move {
            if let Err(e) = server::handle_connection(socket).await {
                error!(error = %e, "Connection error");
            }
        });
    }
}
