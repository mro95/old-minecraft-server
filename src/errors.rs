use thiserror::Error;

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("World error: {0}")]
    World(#[from] crate::world::WorldError),

    #[error("Timeout error: {0}")]
    Timeout(#[from] tokio::time::error::Elapsed),

    #[error("Connection closed")]
    ConnectionClosed,

    #[error("Player not found")]
    PlayerNotFound,

    #[error("Protocol error: {0}")]
    Protocol(String),
}

impl From<&str> for ServerError {
    fn from(s: &str) -> Self {
        ServerError::Protocol(s.to_string())
    }
}

pub type Result<T> = std::result::Result<T, ServerError>;
