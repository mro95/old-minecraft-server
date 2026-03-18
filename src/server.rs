use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, instrument, warn};

use crate::packets::{ClientPacket, ServerPacket};
use crate::player::Player;
use crate::world;
use crate::{PlayerRegistry, protocol};

const CHUNK_SIZE_X: u8 = 16;
const CHUNK_SIZE_Y: u8 = 128;
const CHUNK_SIZE_Z: u8 = 16;

/// Handle an incoming client connection
#[instrument(skip(socket, players))]
pub async fn handle_connection(
    socket: TcpStream,
    players: PlayerRegistry,
) -> Result<(), Box<dyn std::error::Error>> {
    socket.set_nodelay(true)?; // Disable Nagle's algorithm for lower latency
    info!("Connection established");

    let (mut read_half, write_half) = socket.into_split();
    let write_half = Arc::new(Mutex::new(write_half));

    let player = Player::new(write_half.clone(), read_half.peer_addr()?);
    let player = Arc::new(RwLock::new(player));
    players
        .write()
        .await
        .insert(read_half.peer_addr()?, Arc::clone(&player));

    let mut buffer = BytesMut::with_capacity(1024);
    let mut read_buf = [0u8; 1024];

    // Spawn keep-alive task
    let players_tick = Arc::clone(&players);
    let player_tick = Arc::clone(&player);
    let keep_alive_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            let tick = interval.tick().await;

            {
                let mut player_tick = player_tick.write().await;
                let keepalive_id = player_tick.start_latency_measurement();

                if let Err(e) = player_tick
                    .send_packet(ServerPacket::KeepAlive(keepalive_id))
                    .await
                {
                    debug!(error = %e, "Failed to send keep-alive, connection likely closed");
                    player_tick.pending_keepalive.remove(&keepalive_id); // Clean up if send failed
                    break; // Exit task on error (e.g. connection closed)
                }
            }

            // Send player list update every 5 seconds
            if tick.elapsed().as_secs() % 5 == 0 {
                let player_list = crate::player::get_player_list(&players_tick).await;
                info!(player_count = player_list.len(), "Current player list");
                crate::player::send_player_list_update(&players_tick)
                    .await
                    .unwrap_or_else(|e| {
                        debug!(error = %e, "Failed to send player list update");
                    });
            }
        }
    });

    loop {
        let n = {
            tokio::time::timeout(Duration::from_secs(30), read_half.read(&mut read_buf)).await?? // Add timeout to prevent hanging
        };

        if n == 0 {
            keep_alive_task.abort(); // Stop keep-alive task if connection is closed
            info!("Connection closed by client");
            return Ok(()); // Connection closed
        }

        buffer.extend_from_slice(&read_buf[..n]);

        // Log what we just read AND the total buffer state
        if n > 0 {
            debug!(
                bytes_read = n,
                read_hex = hex::encode(&read_buf[..n.min(50)]),
                buffer_total = buffer.len(),
                buffer_start = hex::encode(&buffer.chunk()[..buffer.len().min(10)]),
                "Read from socket"
            );
        }

        while !buffer.is_empty() {
            match protocol::parse_packet(&buffer) {
                Ok((remaining, packet)) => {
                    let consumed = buffer.len() - remaining.len();
                    debug!(
                        ?packet,
                        consumed_bytes = consumed,
                        packet_id = format!("0x{:02X}", buffer.chunk()[0]),
                        "Parsed packet"
                    );

                    buffer.advance(consumed);

                    handle_packet(packet, player.clone(), players.clone()).await?;
                }
                Err(nom::Err::Incomplete(_)) => {
                    // Need more data
                    debug!("Incomplete packet, waiting for more data");
                    break;
                }
                Err(e) => {
                    // Log details about the error to help debug
                    let buffer_preview = if buffer.len() > 0 {
                        let preview_len = buffer.len().min(10);
                        format!("{:02X?}", &buffer[..preview_len])
                    } else {
                        "empty".to_string()
                    };

                    warn!(
                        error = ?e,
                        buffer_len = buffer.len(),
                        buffer_preview = %buffer_preview,
                        "Parse error, clearing buffer"
                    );
                    buffer.clear();
                    break;
                }
            }
        }
    }
}

/// Handle a parsed client packet
#[instrument(skip(player, players))]
async fn handle_packet(
    packet: ClientPacket,
    player: Arc<RwLock<Player>>,
    players: PlayerRegistry,
) -> Result<(), Box<dyn std::error::Error>> {
    match packet {
        ClientPacket::KeepAlive(value) => {
            info!(value, "Keep alive received from client");

            let mut player = player.write().await;
            if let Some(pending) = player.pending_keepalive.remove(&value) {
                let latency = pending.elapsed().as_millis() as i16;
                player.last_latency = Some(Duration::from_millis(latency as u64));
                info!(
                    latency_ms = latency,
                    "Keep-alive response received, latency updated"
                );
            } else {
                info!("Received unexpected keep-alive value: {}, ignoring", value);
            }
        }
        ClientPacket::Handshake(username) => {
            info!(username = %username, "Handshake received");
            player
                .write()
                .await
                .send_packet(ServerPacket::Handshake("-".to_string()))
                .await?;
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
            player.write().await.set_username(username.clone());
            handle_login(player.clone(), players.clone()).await?;
        }
        ClientPacket::ChatMessage(message) => {
            info!(message = %message, "Chat message received");
            {
                let player = player.read().await;
                player
                    .broadcast_packet(
                        &players,
                        ServerPacket::ChatMessage(format!(
                            "{}: {}",
                            player.get_username().unwrap_or_default(),
                            message
                        )),
                        true,
                    )
                    .await?;
            }
        }
        ClientPacket::Player { on_ground } => {
            debug!(on_ground, "Player on-ground status update");
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
        ClientPacket::PlayerLook {
            yaw,
            pitch,
            on_ground,
        } => {
            debug!(yaw, pitch, on_ground, "Player look update");
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
                x,
                y, stance, z, yaw, pitch, on_ground, "Player position and look update"
            );

            {
                let mut player = player.write().await;
                player.position = (x, y, z);
                player.rotation = (yaw, pitch);
            }

            // Broadcast the player's new position and look to all other players
            player
                .read()
                .await
                .broadcast_packet(
                    &players,
                    ServerPacket::EntityTeleport {
                        entity_id: player.read().await.entity_id.unwrap_or(0),
                        x: (x * 32.0) as i32,
                        y: (y * 32.0) as i32,
                        z: (z * 32.0) as i32,
                        yaw: (yaw * 256.0 / 360.0) as u8,
                        pitch: (pitch * 256.0 / 360.0) as u8,
                    },
                    false, // Don't include self in broadcast
                )
                .await?;
        }
        ClientPacket::PlayerDigging {
            status,
            x,
            y,
            z,
            face,
        } => {
            debug!(status, x, y, z, face, "Player digging");

            // Status: 0=start digging, 1=cancel digging, 2=finish digging, 4=drop item, 5=shoot arrow

            if status == 0 {
                // For simplicity, just send a block change to air immediately
                let socket = player.read().await.get_socket();
                send_packet(
                    socket,
                    ServerPacket::BlockChange {
                        x,
                        y,
                        z,
                        block_id: 0, // Air
                    },
                )
                .await?;
            }
        }
        ClientPacket::PlayerBlockPlacement {
            x,
            y,
            z,
            direction,
            held_item,
        } => {
            debug!(x, y, z, direction, held_item, "Player block placement");
            // TODO: Handle block placement
        }
        ClientPacket::HoldingChange { slot } => {
            debug!(slot, "Player holding change");
        }
        ClientPacket::Animation {
            entity_id,
            animation,
        } => {
            debug!(entity_id, animation, "Player animation");
        }
        ClientPacket::EntityAction { entity_id, action } => {
            debug!(entity_id, action, "Entity action");
            // Actions: 1=crouch, 2=uncrouch, 3=leave bed, 4=start sprinting, 5=stop sprinting
        }
        ClientPacket::Disconnect(reason) => {
            info!(reason = %reason, "Client disconnect");
            // Clean up player from registry
            let mut players_lock = players.write().await;
            if let Some(addr) = player
                .read()
                .await
                .get_socket()
                .lock()
                .await
                .peer_addr()
                .ok()
            {
                players_lock.remove(&addr);
                info!(address = %addr, "Player removed from registry");
            } else {
                warn!("Could not get player address for cleanup");
            }

            // Close socket by dropping write half
            player
                .write()
                .await
                .get_socket()
                .lock()
                .await
                .shutdown()
                .await?;

            return Err("Client disconnected".into());
        }
    }
    Ok(())
}

/// Send a packet to the client
#[instrument(skip(socket, packet))]
async fn send_packet(
    socket: Arc<Mutex<OwnedWriteHalf>>,
    packet: ServerPacket,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = packet.to_bytes();
    socket.lock().await.write_all(&bytes).await?;
    debug!(size = bytes.len(), "Sent packet");
    Ok(())
}

/// Handle login sequence for a new player
#[instrument(skip(player, players))]
async fn handle_login(
    player: Arc<RwLock<Player>>,
    players: PlayerRegistry,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting login sequence");

    // Send login response
    player
        .write()
        .await
        .send_packet(ServerPacket::LoginResponse {
            entity_id: 1,
            level_type: "".to_string(),
            map_seed: 1234,
            game_mode: 0,
            dimension: 0,
            difficulty: 1,
            world_height: 127,
            max_players: 20,
        })
        .await?;

    // Send spawn position first
    player
        .write()
        .await
        .send_packet(ServerPacket::SpawnPosition { x: 0, y: 64, z: 0 })
        .await?;

    // IMPORTANT: Send chunks BEFORE player position
    // Send a small 3x3 plain of chunks around spawn
    info!("Sending chunks");
    for cz in -1..=1 {
        for cx in -1..=1 {
            let socket = player.read().await.get_socket();
            send_grass_chunk(socket, cx, cz).await?;
        }
    }
    info!("All chunks sent");

    // Now send player position after chunks are loaded
    player
        .write()
        .await
        .send_packet(ServerPacket::PlayerPositionAndLook {
            x: 0.5,
            y: 65.0,
            stance: 66.62, // Y + 1.62 (eye height)
            z: 0.5,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        })
        .await?;

    player.write().await.entity_id = Some(rand::random::<i32>());

    player
        .read()
        .await
        .broadcast_packet(
            &players,
            ServerPacket::NamedEntitySpawn {
                entity_id: player.read().await.entity_id.unwrap_or(0),
                username: player.read().await.get_username().unwrap_or_default(),
                x: 0,
                y: 64,
                z: 0,
                yaw: 0,
                pitch: 0,
                current_item: 0,
            },
            false, // Don't include self in broadcast
        )
        .await?;

    info!("Login sequence complete");
    Ok(())
}

/// Send a grass plain chunk to the client
#[instrument(skip(socket))]
async fn send_grass_chunk(
    socket: Arc<Mutex<OwnedWriteHalf>>,
    chunk_x: i32,
    chunk_z: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    // Calculate expected size per spec: (Size_X+1) * (Size_Y+1) * (Size_Z+1) * 2.5
    let blocks = (CHUNK_SIZE_X as usize) * (CHUNK_SIZE_Y as usize) * (CHUNK_SIZE_Z as usize);
    let expected_total = (blocks as f32 * 2.5) as usize;

    // Generate terrain data
    let data = world::generate_perlin_noise_chunk(
        CHUNK_SIZE_X,
        CHUNK_SIZE_Y,
        CHUNK_SIZE_Z,
        781378172 + chunk_x as u32 * 348712 + chunk_z as u32 * 7987541,
    );

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
        socket.clone(),
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
        socket.clone(),
        ServerPacket::MapChunk {
            x: chunk_x * 16,          // Convert chunk coord to block coord
            y: 0,                     // Start from bedrock
            z: chunk_z * 16,          // Convert chunk coord to block coord
            size_x: CHUNK_SIZE_X - 1, // 15 (actual size 16)
            size_y: CHUNK_SIZE_Y - 1, // 127 (actual size 128)
            size_z: CHUNK_SIZE_Z - 1, // 15 (actual size 16)
            compressed_data,
        },
    )
    .await?;

    Ok(())
}
