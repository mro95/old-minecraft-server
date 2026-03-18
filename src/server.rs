use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument, warn};

use crate::packets::{ClientPacket, ServerPacket};
use crate::protocol;
use crate::world;

const CHUNK_SIZE_X: u8 = 16;
const CHUNK_SIZE_Y: u8 = 128;
const CHUNK_SIZE_Z: u8 = 16;

/// Handle an incoming client connection
#[instrument(skip(socket))]
pub async fn handle_connection(socket: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    socket.set_nodelay(true)?; // Disable Nagle's algorithm for lower latency
    info!("Connection established");

    let mut buffer = BytesMut::with_capacity(1024);
    let mut read_buf = [0u8; 1024];

    let socket = Arc::new(Mutex::new(socket)); // Wrap socket in Arc<Mutex> for shared access

    // Spawn keep-alive task
    let keepalive_socket = Arc::clone(&socket);
    let keep_alive_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let mut sock = keepalive_socket.lock().await;
            if let Err(e) = send_packet(&mut sock, ServerPacket::KeepAlive(0)).await {
                debug!(error = %e, "Failed to send keep-alive, connection likely closed");
                break; // Exit task on error (e.g. connection closed)
            }
        }
    });

    loop {
        let n = {
            let mut sock = socket.lock().await;
            tokio::time::timeout(Duration::from_secs(30), sock.read(&mut read_buf)).await??
        };

        if n == 0 {
            keep_alive_task.abort(); // Stop keep-alive task if connection is closed
            info!("Connection closed by client");
            return Ok(()); // Connection closed
        }

        buffer.extend_from_slice(&read_buf[..n]);

        while !buffer.is_empty() {
            match protocol::parse_packet(&buffer) {
                Ok((remaining, packet)) => {
                    debug!(?packet, "Parsed packet");

                    let consumed = buffer.len() - remaining.len();
                    buffer.advance(consumed);

                    let mut sock = socket.lock().await; // Lock socket for sending response
                    handle_packet(packet, &mut sock).await?;
                }
                Err(nom::Err::Incomplete(_)) => {
                    // Need more data
                    debug!("Incomplete packet, waiting for more data");
                    break;
                }
                Err(e) => {
                    warn!(error = ?e, "Parse error, clearing buffer");
                    buffer.clear();
                    break;
                }
            }
        }
    }
}

/// Handle a parsed client packet
#[instrument(skip(socket))]
async fn handle_packet(
    packet: ClientPacket,
    socket: &mut TcpStream,
) -> Result<(), Box<dyn std::error::Error>> {
    match packet {
        ClientPacket::KeepAlive(value) => {
            debug!("Keep alive received");
            send_packet(socket, ServerPacket::KeepAlive(value)).await?;
        }
        ClientPacket::Handshake(username) => {
            info!(username = %username, "Handshake received");
            send_packet(socket, ServerPacket::Handshake("-".to_string())).await?;
        }
        ClientPacket::LoginRequest {
            protocol_version,
            username,
            map_seed,
            dimension,
        } => {
            info!(
                username = %username,
                protocol_version,
                map_seed,
                dimension,
                "Login request received"
            );
            handle_login(socket).await?;
        }
        ClientPacket::PlayerPosition {
            x,
            y,
            stance,
            z,
            on_ground,
        } => {
            debug!(x, y, stance, z, on_ground, "Player position update");
        }
        ClientPacket::PlayerPositionAndLook {
            x,
            y,
            stance,
            z,
            yaw,
            pitch,
            on_ground,
        } => {
            debug!(
                x, y, stance, z, yaw, pitch, on_ground,
                "Player position and look update"
            );
        }
    }
    Ok(())
}

/// Send a packet to the client
#[instrument(skip(socket, packet))]
async fn send_packet(
    socket: &mut TcpStream,
    packet: ServerPacket,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = packet.to_bytes();
    socket.write_all(&bytes).await?;
    debug!(size = bytes.len(), "Sent packet");
    Ok(())
}

/// Handle login sequence for a new player
#[instrument(skip(socket))]
async fn handle_login(socket: &mut TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting login sequence");

    // Send login response
    send_packet(
        socket,
        ServerPacket::LoginResponse {
            entity_id: 1,
            level_type: "".to_string(),
            map_seed: 1234,
            game_mode: 0,
            dimension: 0,
            difficulty: 1,
            world_height: 127,
            max_players: 20,
        },
    )
    .await?;

    // Send spawn position first
    send_packet(
        socket,
        ServerPacket::SpawnPosition {
            x: 0,
            y: 64,
            z: 0,
        },
    )
    .await?;

    // IMPORTANT: Send chunks BEFORE player position
    // Send a small 3x3 plain of chunks around spawn
    info!("Sending chunks");
    for cz in -1..=1 {
        for cx in -1..=1 {
            send_grass_chunk(socket, cx, cz).await?;
        }
    }
    info!("All chunks sent");

    // Now send player position after chunks are loaded
    send_packet(
        socket,
        ServerPacket::PlayerPositionAndLook {
            x: 0.5,
            y: 65.0,
            stance: 66.62, // Y + 1.62 (eye height)
            z: 0.5,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        },
    )
    .await?;

    info!("Login sequence complete");
    Ok(())
}

/// Send a grass plain chunk to the client
#[instrument(skip(socket))]
async fn send_grass_chunk(
    socket: &mut TcpStream,
    chunk_x: i32,
    chunk_z: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    // Calculate expected size per spec: (Size_X+1) * (Size_Y+1) * (Size_Z+1) * 2.5
    let blocks = (CHUNK_SIZE_X as usize) * (CHUNK_SIZE_Y as usize) * (CHUNK_SIZE_Z as usize);
    let expected_total = (blocks as f32 * 2.5) as usize;

    // Generate terrain data
    let data = world::generate_grass_plain_chunk(CHUNK_SIZE_X, CHUNK_SIZE_Y, CHUNK_SIZE_Z);

    debug!(
        chunk_x,
        chunk_z,
        generated_bytes = data.len(),
        expected_bytes = expected_total,
        "Generated chunk data"
    );

    // Compress using zlib format
    let compressed_data = world::compress_chunk_data(&data)?;

    // Verify zlib format
    if world::verify_zlib_format(&compressed_data) {
        debug!(
            header = format!("{:02X} {:02X}", compressed_data[0], compressed_data[1]),
            "Valid zlib compression format"
        );
    } else {
        warn!("Invalid compression format - Minecraft may reject this chunk");
    }

    // Send pre-chunk packet (0x32)
    send_packet(
        socket,
        ServerPacket::PreChunk {
            x: chunk_x,
            z: chunk_z,
            mode: true,
        },
    )
    .await?;

    // Send chunk data (0x33)
    // Per spec: X, Y, Z are BLOCK coordinates (not chunk)
    // Sizes are actual size - 1
    send_packet(
        socket,
        ServerPacket::MapChunk {
            x: chunk_x * 16,      // Convert chunk coord to block coord
            y: 0,                 // Start from bedrock
            z: chunk_z * 16,      // Convert chunk coord to block coord
            size_x: CHUNK_SIZE_X - 1, // 15 (actual size 16)
            size_y: CHUNK_SIZE_Y - 1, // 127 (actual size 128)
            size_z: CHUNK_SIZE_Z - 1, // 15 (actual size 16)
            compressed_data,
        },
    )
    .await?;

    Ok(())
}
