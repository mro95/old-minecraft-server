use bytes::BytesMut;

use crate::packet_ids;

/// Packets sent from the client to the server
#[derive(Debug)]
pub enum ClientPacket {
    KeepAlive(i32),
    Handshake(String),
    LoginRequest {
        protocol_version: i32,
        username: String,
        map_seed: i64,
        dimension: i8,
    },
    ChatMessage(String),
    Player {
        on_ground: bool,
    },
    PlayerPosition {
        x: f64,
        y: f64,
        stance: f64,
        z: f64,
        on_ground: bool,
    },
    PlayerLook {
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    PlayerPositionAndLook {
        x: f64,
        y: f64,
        stance: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    PlayerDigging {
        status: i8,
        x: i32,
        y: i8,
        z: i32,
        face: i8,
    },
    PlayerBlockPlacement {
        x: i32,
        y: i8,
        z: i32,
        direction: i8,
        held_item: i16,
    },
    HoldingChange {
        slot: i16,
    },
    Animation {
        entity_id: i32,
        animation: i8,
    },
    EntityAction {
        entity_id: i32,
        action: i8,
    },
    Disconnect(String),
}

/// Packets sent from the server to the client
#[derive(Debug, Clone)]
pub enum ServerPacket {
    KeepAlive(i32),
    Handshake(String),
    LoginResponse {
        entity_id: u32,
        level_type: String,
        map_seed: i64,
        game_mode: i32,
        dimension: u8,
        difficulty: u8,
        world_height: i8,
        max_players: i8,
    },
    SpawnPosition {
        x: i32,
        y: i32,
        z: i32,
    },
    // Force the client to update the player's position and look (e.g. after teleportation)
    PlayerPositionAndLook {
        x: f64,
        y: f64,
        stance: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    PreChunk {
        x: i32,
        z: i32,
        mode: bool,
    },
    MapChunk {
        x: i32,
        y: i16,
        z: i32,
        size_x: u8,
        size_y: u8,
        size_z: u8,
        compressed_data: Vec<u8>,
    },
    ChatMessage(String),
    PlayerListItem {
        username: String,
        online: bool,
        ping: i16,
    },
    BlockChange {
        x: i32,
        y: i8,
        z: i32,
        block_id: u8,
    },
    NamedEntitySpawn {
        entity_id: i32,
        username: String,
        x: i32,
        y: i32,
        z: i32,
        yaw: u8,
        pitch: u8,
        current_item: i16,
    },
    Entity(i32),
    EntityRelativeMove {
        entity_id: i32,
        delta_x: u8,
        delta_y: u8,
        delta_z: u8,
    },
    EntityLook {
        entity_id: i32,
        yaw: u8,
        pitch: u8,
    },
    EntityLookAndRelativeMove {
        entity_id: i32,
        delta_x: u8,
        delta_y: u8,
        delta_z: u8,
        yaw: u8,
        pitch: u8,
    },
    EntityTeleport {
        entity_id: i32,
        x: i32,
        y: i32,
        z: i32,
        yaw: u8,
        pitch: u8,
    },
}

impl ServerPacket {
    /// Serialize packet to bytes for network transmission
    pub fn to_bytes(&self) -> BytesMut {
        let mut bytes = BytesMut::new();

        match self {
            ServerPacket::KeepAlive(value) => {
                bytes.extend_from_slice(&[packet_ids::KEEP_ALIVE]);
                bytes.extend_from_slice(&value.to_be_bytes());
            }
            ServerPacket::Handshake(hash) => {
                bytes.extend_from_slice(&[packet_ids::HANDSHAKE]);
                write_utf16_string(&mut bytes, hash);
            }
            ServerPacket::LoginResponse {
                entity_id,
                level_type,
                map_seed,
                game_mode,
                dimension,
                difficulty,
                world_height,
                max_players,
            } => {
                bytes.extend_from_slice(&[packet_ids::LOGIN_REQUEST]);
                bytes.extend_from_slice(&entity_id.to_be_bytes());

                // Write level type as UTF-16 string
                write_utf16_string(&mut bytes, level_type);

                // Write map_seed (between length and string content)
                bytes.extend_from_slice(&map_seed.to_be_bytes());

                bytes.extend_from_slice(&game_mode.to_be_bytes());
                bytes.extend_from_slice(&[*dimension, *difficulty, *world_height as u8, *max_players as u8]);
            }
            ServerPacket::SpawnPosition { x, y, z } => {
                bytes.extend_from_slice(&[packet_ids::SPAWN_POSITION]);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
            }
            ServerPacket::PlayerPositionAndLook {
                x,
                y,
                stance,
                z,
                yaw,
                pitch,
                on_ground,
            } => {
                bytes.extend_from_slice(&[packet_ids::PLAYER_POSITION_AND_LOOK]);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&stance.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.extend_from_slice(&yaw.to_be_bytes());
                bytes.extend_from_slice(&pitch.to_be_bytes());
                bytes.extend_from_slice(&[if *on_ground { 1 } else { 0 }]);
            }
            ServerPacket::PreChunk { x, z, mode } => {
                bytes.extend_from_slice(&[packet_ids::PRE_CHUNK]);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.extend_from_slice(&[if *mode { 1 } else { 0 }]);
            }
            ServerPacket::MapChunk {
                x,
                y,
                z,
                size_x,
                size_y,
                size_z,
                compressed_data,
            } => {
                bytes.extend_from_slice(&[packet_ids::MAP_CHUNK]);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.extend_from_slice(&[*size_x]);
                bytes.extend_from_slice(&[*size_y]);
                bytes.extend_from_slice(&[*size_z]);
                bytes.extend_from_slice(&(compressed_data.len() as i32).to_be_bytes());
                bytes.extend_from_slice(compressed_data);
            }
            ServerPacket::ChatMessage(message) => {
                bytes.extend_from_slice(&[packet_ids::CHAT_MESSAGE]);
                write_utf16_string(&mut bytes, message);
            }
            ServerPacket::PlayerListItem {
                username,
                online,
                ping,
            } => {
                bytes.extend_from_slice(&[packet_ids::PLAYER_LIST_ITEM]);
                write_utf16_string(&mut bytes, username);
                bytes.extend_from_slice(&[if *online { 1 } else { 0 }]);
                bytes.extend_from_slice(&ping.to_be_bytes());
            }
            ServerPacket::BlockChange { x, y, z, block_id } => {
                bytes.extend_from_slice(&[packet_ids::BLOCK_CHANGE]);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&[*y as u8]);
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.extend_from_slice(&[*block_id]);
                bytes.extend_from_slice(&[0]); // metadata (nibble) - can be extended to include actual metadata if needed
            }
            ServerPacket::Entity(entity_id) => {
                bytes.extend_from_slice(&[packet_ids::ENTITY]);
                bytes.extend_from_slice(&entity_id.to_be_bytes());
            }
            ServerPacket::EntityRelativeMove {
                entity_id,
                delta_x,
                delta_y,
                delta_z,
            } => {
                bytes.extend_from_slice(&[packet_ids::ENTITY_RELATIVE_MOVE]);
                bytes.extend_from_slice(&entity_id.to_be_bytes());
                bytes.extend_from_slice(&[*delta_x]);
                bytes.extend_from_slice(&[*delta_y]);
                bytes.extend_from_slice(&[*delta_z]);
            }
            ServerPacket::EntityLook {
                entity_id,
                yaw,
                pitch,
            } => {
                bytes.extend_from_slice(&[packet_ids::ENTITY_LOOK]);
                bytes.extend_from_slice(&entity_id.to_be_bytes());
                bytes.extend_from_slice(&[*yaw]);
                bytes.extend_from_slice(&[*pitch]);
            }
            ServerPacket::EntityLookAndRelativeMove {
                entity_id,
                delta_x,
                delta_y,
                delta_z,
                yaw,
                pitch,
            } => {
                bytes.extend_from_slice(&[packet_ids::ENTITY_LOOK_AND_RELATIVE_MOVE]);
                bytes.extend_from_slice(&entity_id.to_be_bytes());
                bytes.extend_from_slice(&[*delta_x]);
                bytes.extend_from_slice(&[*delta_y]);
                bytes.extend_from_slice(&[*delta_z]);
                bytes.extend_from_slice(&[*yaw]);
                bytes.extend_from_slice(&[*pitch]);
            }
            ServerPacket::EntityTeleport {
                entity_id,
                x,
                y,
                z,
                yaw,
                pitch,
            } => {
                bytes.extend_from_slice(&[packet_ids::ENTITY_TELEPORT]);
                bytes.extend_from_slice(&entity_id.to_be_bytes());
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.extend_from_slice(&[*yaw]);
                bytes.extend_from_slice(&[*pitch]);
            }
            ServerPacket::NamedEntitySpawn {
                entity_id,
                username,
                x,
                y,
                z,
                yaw,
                pitch,
                current_item,
            } => {
                bytes.extend_from_slice(&[packet_ids::NAMED_ENTITY_SPAWN]);
                bytes.extend_from_slice(&entity_id.to_be_bytes());
                write_utf16_string(&mut bytes, username);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.extend_from_slice(&[*yaw]);
                bytes.extend_from_slice(&[*pitch]);
                bytes.extend_from_slice(&current_item.to_be_bytes());
            }
        }

        bytes
    }
}

/// Helper function to write UTF-16 strings in Minecraft protocol format
fn write_utf16_string(buffer: &mut BytesMut, s: &str) {
    let bytes = s.as_bytes();
    buffer.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    for byte in bytes {
        buffer.extend_from_slice(&[0x00, *byte]);
    }
}
