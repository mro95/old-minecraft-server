use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, instrument, warn};

use crate::chunk_manager::{
    CHUNK_SIZE_X_U8, CHUNK_SIZE_Y_U8, CHUNK_SIZE_Z_U8, ChunkPos, SharedChunkManager,
    SharedChunkPos, VIEW_DISTANCE,
};
use crate::packets::{ClientPacket, ServerPacket};
use crate::player::Player;
use crate::{PlayerRegistry, Result, ServerError, protocol};

/// Handle an incoming client connection
#[instrument(skip(socket, players, chunk_manager))]
pub async fn handle_connection(
    socket: TcpStream,
    players: PlayerRegistry,
    chunk_manager: SharedChunkManager,
) -> Result<()> {
    socket.set_nodelay(true)?; // Disable Nagle's algorithm for lower latency
    info!("Connection established");

    let (mut read_half, write_half) = socket.into_split();
    let write_half = Arc::new(Mutex::new(write_half));
    let peer_addr = read_half.peer_addr()?;

    let player = Player::new(write_half.clone(), peer_addr);
    let player = Arc::new(RwLock::new(player));

    // Spawn keep-alive task before registration
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

    let mut buffer = BytesMut::with_capacity(1024);
    let mut read_buf = [0u8; 1024];

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

                    handle_packet(
                        packet,
                        player.clone(),
                        players.clone(),
                        chunk_manager.clone(),
                    )
                    .await?;
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
#[instrument(skip(player, players, chunk_manager))]
async fn handle_packet(
    packet: ClientPacket,
    player: Arc<RwLock<Player>>,
    players: PlayerRegistry,
    chunk_manager: SharedChunkManager,
) -> Result<()> {
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
            handle_login(player.clone(), players.clone(), chunk_manager.clone()).await?;
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

            {
                let mut player = player.write().await;
                player.rotation = (yaw, pitch);
            }

            // Broadcast the player's new look to all other players
            player
                .read()
                .await
                .broadcast_packet(
                    &players,
                    ServerPacket::EntityLook {
                        entity_id: player.read().await.entity_id.unwrap_or(0),
                        yaw: (yaw * 256.0 / 360.0) as u8,
                        pitch: (pitch * 256.0 / 360.0) as u8,
                    },
                    false, // Don't include self in broadcast
                )
                .await?;
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

            let (old_position, old_rotation, old_chunk) = {
                let player = player.read().await;
                (player.position, player.rotation, player.current_chunk_pos)
            };

            let new_chunk = ChunkPos::from_world_pos(x as i32, z as i32);

            if new_chunk != old_chunk {
                debug!(
                    ?old_chunk,
                    ?new_chunk,
                    "Player crossed chunk boundary, streaming chunks"
                );
                handle_chunk_transition(
                    player.clone(),
                    old_chunk,
                    new_chunk,
                    chunk_manager.clone(),
                )
                .await?;
            }

            // If position changed less then a threshold, we can send a relative move/look instead of teleport for better client interpolation
            const POSITION_THRESHOLD: u8 = 4; // Less than 4 blocks it expected by the client.
            let delta_x = ((x - old_position.0).abs() * 32.0) as u8;
            let delta_y = ((y - old_position.1).abs() * 32.0) as u8;
            let delta_z = ((z - old_position.2).abs() * 32.0) as u8;
            let small_position_change = delta_x < POSITION_THRESHOLD
                && delta_y < POSITION_THRESHOLD
                && delta_z < POSITION_THRESHOLD;

            let delta_yaw = ((yaw - old_rotation.0).abs() * 256.0 / 360.0) as u8;
            let delta_pitch = ((pitch - old_rotation.1).abs() * 256.0 / 360.0) as u8;
            let rotation_changed = delta_yaw > 0 || delta_pitch > 0;

            {
                let mut player = player.write().await;
                player.position = (x, y, z);
                player.rotation = (yaw, pitch);
                player.current_chunk_pos = new_chunk;
            }

            if small_position_change || rotation_changed {
                debug!(
                    small_position_change,
                    rotation_changed,
                    old_position = ?old_position,
                    new_position = ?(x, y, z),
                    old_rotation = ?old_rotation,
                    new_rotation = ?(yaw, pitch),
                    "Determined packet type to send based on changes"
                );

                // Send relative move/look because it's a small change, which allows client to interpolate smoothly
                player
                    .read()
                    .await
                    .broadcast_packet(
                        &players,
                        ServerPacket::EntityLookAndRelativeMove {
                            entity_id: player.read().await.entity_id.unwrap_or(0),
                            delta_x,
                            delta_y,
                            delta_z,
                            yaw: (yaw * 256.0 / 360.0) as u8,
                            pitch: (pitch * 256.0 / 360.0) as u8,
                        },
                        false, // Don't include self in broadcast
                    )
                    .await?;
            } else {
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

            return Err(ServerError::ConnectionClosed);
        }
    }
    Ok(())
}

/// Send a packet to the client
#[instrument(skip(socket, packet))]
async fn send_packet(socket: Arc<Mutex<OwnedWriteHalf>>, packet: ServerPacket) -> Result<()> {
    let bytes = packet.to_bytes();
    socket.lock().await.write_all(&bytes).await?;
    debug!(size = bytes.len(), "Sent packet");
    Ok(())
}

/// Handle login sequence for a new player
#[instrument(skip(player, players, chunk_manager))]
async fn handle_login(
    player: Arc<RwLock<Player>>,
    players: PlayerRegistry,
    chunk_manager: SharedChunkManager,
) -> Result<()> {
    info!("Starting login sequence");

    const SPAWN_X: f64 = 0.5;
    const SPAWN_Y: f64 = 65.0;
    const SPAWN_Z: f64 = 0.5;

    let spawn_chunk = ChunkPos::from_world_pos(SPAWN_X as i32, SPAWN_Z as i32);
    let chunk_positions = ChunkPos::chunks_in_radius(spawn_chunk, VIEW_DISTANCE);
    info!(chunk_count = chunk_positions.len(), view_distance = VIEW_DISTANCE, "Chunks to send for initial view distance");

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

    let socket = player.read().await.get_socket();

    // Send PreChunk and MapChunk packets for all chunks in view distance
    info!("Sending initial chunks");
    for pos in &chunk_positions {
        send_packet(
            socket.clone(),
            ServerPacket::PreChunk {
                x: pos.x,
                z: pos.z,
                mode: true,
            },
        )
        .await?;

        send_chunk(socket.clone(), pos.x, pos.z, chunk_manager.clone()).await?;
    }
    info!(count = chunk_positions.len(), "All initial chunks sent");

    // Create shared chunk position for prefetch task
    let player_chunk_pos: SharedChunkPos = Arc::new(RwLock::new(spawn_chunk));

    // Update player state
    {
        let mut player_write = player.write().await;
        player_write.position = (SPAWN_X, SPAWN_Y, SPAWN_Z);
        player_write.current_chunk_pos = spawn_chunk;
    }

    // Start background prefetch task
    chunk_manager.clone().start_prefetch(player_chunk_pos.clone());
    info!("Started chunk prefetch task");

    // Now send player position after chunks are loaded
    player
        .write()
        .await
        .send_packet(ServerPacket::PlayerPositionAndLook {
            x: SPAWN_X,
            y: SPAWN_Y,
            stance: 66.62,
            z: SPAWN_Z,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        })
        .await?;

    let (entity_id, username, peer_addr) = {
        let mut player_write = player.write().await;
        let entity_id = rand::random::<i32>();
        player_write.entity_id = Some(entity_id);
        (
            entity_id,
            player_write.get_username().unwrap_or_default(),
            player_write.peer_addr,
        )
    };

    // Create entity spawn packets for existing players to show them to the new player
    let player_list = crate::player::get_player_list(&players).await;
    info!(
        player_count = player_list.len(),
        "Existing players to spawn for new player"
    );
    for (addr, other_username, other_entity_id) in player_list {
        if addr == peer_addr {
            info!("Skipping self");
            continue;
        }

        let (x, y, z) = {
            let players_lock = players.read().await;
            if let Some(other_player) = players_lock.get(&addr) {
                let p = other_player.read().await;
                info!(addr = %addr, username = %other_username, entity_id = ?other_entity_id, pos = ?p.position, "Existing player data");
                (
                    p.position.0 as i32,
                    p.position.1 as i32,
                    p.position.2 as i32,
                )
            } else {
                info!("Player not found in registry");
                (0, 64, 0)
            }
        };

        info!(username = %other_username, entity_id = ?other_entity_id, x, y, z, "Sending NamedEntitySpawn to new player");
        player
            .write()
            .await
            .send_packet(ServerPacket::NamedEntitySpawn {
                entity_id: other_entity_id.unwrap_or(0),
                username: other_username,
                x,
                y,
                z,
                yaw: 0,
                pitch: 0,
                current_item: 0,
            })
            .await?;
    }

    // Add player to registry AFTER all data is set (entity_id, username, position)
    // This ensures other players see fully initialized player data
    info!("Adding player to registry");
    players.write().await.insert(peer_addr, Arc::clone(&player));

    // Now broadcast this player's spawn to all existing players
    info!(username = %username, entity_id, x = SPAWN_X as i32, y = SPAWN_Y as i32, z = SPAWN_Z as i32, "Broadcasting player spawn");
    let (entity_id, username) = {
        let p = player.read().await;
        (
            p.entity_id.unwrap_or(0),
            p.get_username().unwrap_or_default(),
        )
    };
    player
        .read()
        .await
        .broadcast_packet(
            &players,
            ServerPacket::NamedEntitySpawn {
                entity_id,
                username,
                x: SPAWN_X as i32,
                y: SPAWN_Y as i32,
                z: SPAWN_Z as i32,
                yaw: 0,
                pitch: 0,
                current_item: 0,
            },
            false,
        )
        .await?;

    Ok(())
}

/// Handle chunk transition when player moves to a new chunk
async fn handle_chunk_transition(
    player: Arc<RwLock<Player>>,
    old_chunk: ChunkPos,
    new_chunk: ChunkPos,
    chunk_manager: SharedChunkManager,
) -> Result<()> {
    let socket = player.read().await.get_socket();

    // Send unload packets for chunks that left view distance
    let chunks_to_unload = ChunkPos::chunks_to_unload(old_chunk, new_chunk, VIEW_DISTANCE);
    for pos in &chunks_to_unload {
        send_packet(
            socket.clone(),
            ServerPacket::PreChunk {
                x: pos.x,
                z: pos.z,
                mode: false, // Unload
            },
        )
        .await?;
    }

    // Send load packets for new chunks
    let chunks_to_load = ChunkPos::chunks_to_load(old_chunk, new_chunk, VIEW_DISTANCE);
    for pos in &chunks_to_load {
        send_packet(
            socket.clone(),
            ServerPacket::PreChunk {
                x: pos.x,
                z: pos.z,
                mode: true, // Load
            },
        )
        .await?;

        send_chunk(socket.clone(), pos.x, pos.z, chunk_manager.clone()).await?;
    }

    info!(
        loaded = chunks_to_load.len(),
        unloaded = chunks_to_unload.len(),
        "Chunk transition complete"
    );

    Ok(())
}

/// Send a chunk to the client (only MapChunk, PreChunk sent separately)
#[instrument(skip(socket, chunk_manager))]
async fn send_chunk(
    socket: Arc<Mutex<OwnedWriteHalf>>,
    chunk_x: i32,
    chunk_z: i32,
    chunk_manager: SharedChunkManager,
) -> Result<()> {
    let pos = ChunkPos::new(chunk_x, chunk_z);
    let compressed_data = chunk_manager.get_compressed_chunk_data(pos).await;

    match compressed_data {
        Some(data) => {
            debug!(chunk_x, chunk_z, bytes = data.len(), "Sending chunk data");
            send_packet(
                socket.clone(),
                ServerPacket::MapChunk {
                    x: chunk_x * 16,
                    y: 0,
                    z: chunk_z * 16,
                    size_x: CHUNK_SIZE_X_U8 - 1,
                    size_y: CHUNK_SIZE_Y_U8 - 1,
                    size_z: CHUNK_SIZE_Z_U8 - 1,
                    compressed_data: data,
                },
            )
            .await?;
        }
        None => {
            warn!(chunk_x, chunk_z, "Failed to get chunk data");
        }
    }

    Ok(())
}
