mod chunk_manager;
mod config;
mod errors;
mod packet_ids;
mod packets;
mod player;
mod protocol;
mod server;
mod world;

pub use chunk_manager::{ChunkManager, SharedChunkManager};
pub use config::WorldConfig;
pub use errors::{Result, ServerError};

use std::{collections::HashMap, sync::Arc};

use tokio::{net::TcpListener, sync::RwLock};
use tracing::{error, info};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::player::Player;

type PlayerRegistry = Arc<RwLock<HashMap<std::net::SocketAddr, Arc<RwLock<Player>>>>>;

#[tokio::main(flavor = "multi_thread", worker_threads = 10)]
async fn main() -> crate::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "mincraft_server=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let world_config = WorldConfig::load_or_default("world_config.toml");
    info!(
        seed = world_config.world.seed,
        "Loaded world configuration"
    );

    let listener = TcpListener::bind("127.0.0.1:25565").await?;
    info!("Server listening on 127.0.0.1:25565");

    let players: PlayerRegistry = Arc::new(RwLock::new(HashMap::new()));

    let chunk_manager: SharedChunkManager = Arc::new(ChunkManager::new("world", world_config));
    chunk_manager.clone().start_auto_save().await;
    info!("Auto-save task started (every 60 seconds)");

    loop {
        let (socket, addr) = listener.accept().await?;
        info!(address = %addr, "New connection");

        let players = players.clone();
        let chunk_manager = chunk_manager.clone();
        tokio::spawn(async move {
            if let Err(e) = server::handle_connection(socket, players, chunk_manager).await {
                error!(error = %e, "Connection error");
            }
        });
    }
}
