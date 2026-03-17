use std::sync::Arc;
use std::time::Duration;

use bytes::BytesMut;
use bytes::Buf;
use nom::bytes::streaming::take;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use nom::IResult;
use tokio::sync::Mutex;

// Packet ID constants
mod packet_ids {
    pub const KEEP_ALIVE: u8 = 0x00;
    pub const LOGIN_REQUEST: u8 = 0x01;
    pub const HANDSHAKE: u8 = 0x02;
    pub const SPAWN_POSITION: u8 = 0x06;
    pub const PLAYER_POSITION: u8 = 0x0B;
    pub const PLAYER_POSITION_AND_LOOK: u8 = 0x0D;
    pub const PRE_CHUNK: u8 = 0x32;
    pub const MAP_CHUNK: u8 = 0x33;
}

// Helper functions for parsing
fn parse_i32(input: &[u8]) -> IResult<&[u8], i32> {
    let (input, bytes) = take(4usize)(input)?;
    Ok((input, i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
}

fn parse_i64(input: &[u8]) -> IResult<&[u8], i64> {
    let (input, bytes) = take(8usize)(input)?;
    Ok((input, i64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ])))
}

fn parse_f32(input: &[u8]) -> IResult<&[u8], f32> {
    let (input, bytes) = take(4usize)(input)?;
    Ok((input, f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])))
}

fn parse_f64(input: &[u8]) -> IResult<&[u8], f64> {
    let (input, bytes) = take(8usize)(input)?;
    Ok((input, f64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5], bytes[6], bytes[7],
    ])))
}

fn parse_utf16_string(input: &[u8]) -> IResult<&[u8], String> {
    let (input, len) = take(2usize)(input)?;
    let string_len = u16::from_be_bytes([len[0], len[1]]) as usize;
    let (input, string_bytes) = take(string_len * 2)(input)?;
    
    let utf16_chars: Vec<u16> = string_bytes
        .chunks(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();
    
    Ok((input, String::from_utf16_lossy(&utf16_chars)))
}

fn parse_bool(input: &[u8]) -> IResult<&[u8], bool> {
    let (input, byte) = take(1usize)(input)?;
    Ok((input, byte[0] != 0))
}

// Helper functions for building packets
fn write_utf16_string(buffer: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    buffer.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    for byte in bytes {
        buffer.extend_from_slice(&[0x00, *byte]);
    }
}

fn verify_zlib_format(data: &[u8]) -> bool {
    if data.len() < 6 {
        return false; // Too small for valid zlib (2 byte header + data + 4 byte checksum)
    }
    
    // Check zlib header magic bytes
    // 0x78 = deflate compression method
    // Second byte varies based on compression level and window size
    let valid_header = data[0] == 0x78 && (data[1] == 0x01 || data[1] == 0x9C || data[1] == 0xDA);
    
    if !valid_header {
        eprintln!("WARNING: Invalid zlib header: {:02X} {:02X}", data[0], data[1]);
        eprintln!("  Expected 0x78 followed by 0x01/0x9C/0xDA");
    }
    
    valid_header
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:25565").await?;
    println!("Server listening on 127.0.0.1:25565");

    loop {
        let (socket, addr) = listener.accept().await?;
        println!("New connection from: {}", addr);

        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket).await {
                eprintln!("Connection error: {}", e);
            }
        });
    }
}

async fn handle_connection(socket: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    socket.set_nodelay(true)?; // Disable Nagle's algorithm for lower latency

    let mut buffer = BytesMut::with_capacity(1024);
    let mut read_buf = [0u8; 1024];

    let socket = Arc::new(Mutex::new(socket)); // Wrap socket in Arc<Mutex> for shared access

    let keepalive_socket = Arc::clone(&socket);
    let keep_alive_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            interval.tick().await;
            let mut sock = keepalive_socket.lock().await;
            if let Err(e) = send_packet(&mut sock, ServerPacket::KeepAlive(0)).await {
                eprintln!("Failed to send keep-alive: {}", e);
                break; // Exit task on error (e.g. connection closed)
            }
        }
    });

    loop {
        let n = {
            let mut sock = socket.lock().await;
            tokio::time::timeout(
                Duration::from_secs(30),
                sock.read(&mut read_buf)
            ).await??
        };

        if n == 0 {
            keep_alive_task.abort(); // Stop keep-alive task if connection is closed
            return Ok(()); // Connection closed
        }

        buffer.extend_from_slice(&read_buf[..n]);

        while !buffer.is_empty() {
            match parse_packet(&buffer) {
                Ok((remaining, packet)) => {
                    println!("Parsed packet: {:?}", packet);
                    
                    let consumed = buffer.len() - remaining.len();
                    buffer.advance(consumed); 

                    let mut sock = socket.lock().await; // Lock socket for sending response
                    handle_packet(packet, &mut sock).await?;
                }
                Err(nom::Err::Incomplete(_)) => {
                    // Need more data
                    break;
                }
                Err(e) => {
                    eprintln!("Parse error: {:?}, clearing buffer", e);
                    buffer.clear();
                    break;
                }
            }
        }
    }
}

fn parse_packet(input: &[u8]) -> IResult<&[u8], ClientPacket> {
    let (input, packet_id) = take(1usize)(input)?;
    
    match packet_id[0] {
        packet_ids::KEEP_ALIVE => {
            let (input, keep_alive_value) = parse_i32(input)?;
            Ok((input, ClientPacket::KeepAlive(keep_alive_value)))
        }
        packet_ids::HANDSHAKE => {
            let (input, username) = parse_utf16_string(input)?;
            Ok((input, ClientPacket::Handshake(username)))
        }
        packet_ids::LOGIN_REQUEST => {
            let (input, protocol_version) = parse_i32(input)?;
            let (input, username) = parse_utf16_string(input)?;
            let (input, map_seed) = parse_i64(input)?;
            let (input, dimension) = take(1usize)(input)?;
            
            Ok((input, ClientPacket::LoginRequest {
                protocol_version,
                username,
                map_seed,
                dimension: dimension[0] as i8,
            }))
        }
        packet_ids::PLAYER_POSITION => {
            let (input, x) = parse_f64(input)?;
            let (input, y) = parse_f64(input)?;
            let (input, stance) = parse_f64(input)?;
            let (input, z) = parse_f64(input)?;
            let (input, on_ground) = parse_bool(input)?;
            
            Ok((input, ClientPacket::PlayerPosition { x, y, stance, z, on_ground }))
        }
        packet_ids::PLAYER_POSITION_AND_LOOK => {
            let (input, x) = parse_f64(input)?;
            let (input, y) = parse_f64(input)?;
            let (input, stance) = parse_f64(input)?;
            let (input, z) = parse_f64(input)?;
            let (input, yaw) = parse_f32(input)?;
            let (input, pitch) = parse_f32(input)?;
            let (input, on_ground) = parse_bool(input)?;
            
            Ok((input, ClientPacket::PlayerPositionAndLook {
                x, y, stance, z, yaw, pitch, on_ground,
            }))
        }
        _ => {
            Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
        }
    }
}

#[derive(Debug)]
enum ClientPacket {
    KeepAlive(i32),
    Handshake(String),
    LoginRequest {
        protocol_version: i32,
        username: String,
        map_seed: i64,
        dimension: i8,
    },
    PlayerPosition {
        x: f64,
        y: f64,
        stance: f64,
        z: f64,
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
    }
}

#[derive(Debug)]
enum ServerPacket {
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
}

impl ServerPacket {
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        
        match self {
            ServerPacket::KeepAlive(value) => {
                bytes.push(packet_ids::KEEP_ALIVE);
                bytes.extend_from_slice(&value.to_be_bytes());
            }
            ServerPacket::Handshake(hash) => {
                bytes.push(packet_ids::HANDSHAKE);
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
                bytes.push(packet_ids::LOGIN_REQUEST);
                bytes.extend_from_slice(&entity_id.to_be_bytes());
                
                // Write string length
                let level_type_bytes = level_type.as_bytes();
                bytes.extend_from_slice(&(level_type_bytes.len() as u16).to_be_bytes());
                
                // Write UTF-16 string content
                for byte in level_type_bytes {
                    bytes.extend_from_slice(&[0x00, *byte]);
                }
                
                // Write map_seed (between length and string content)
                bytes.extend_from_slice(&map_seed.to_be_bytes());
                
                bytes.extend_from_slice(&game_mode.to_be_bytes());
                bytes.push(*dimension);
                bytes.push(*difficulty);
                bytes.push(*world_height as u8);
                bytes.push(*max_players as u8);
            }
            ServerPacket::SpawnPosition { x, y, z } => {
                bytes.push(packet_ids::SPAWN_POSITION);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
            }
            ServerPacket::PlayerPositionAndLook { x, y, stance, z, yaw, pitch, on_ground } => {
                bytes.push(packet_ids::PLAYER_POSITION_AND_LOOK);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&stance.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.extend_from_slice(&yaw.to_be_bytes());
                bytes.extend_from_slice(&pitch.to_be_bytes());
                bytes.push(if *on_ground { 1 } else { 0 });
            }
            ServerPacket::PreChunk { x, z, mode } => {
                bytes.push(packet_ids::PRE_CHUNK);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.push(if *mode { 1 } else { 0 });
            }
            ServerPacket::MapChunk { x, y, z, size_x, size_y, size_z, compressed_data } => {
                bytes.push(packet_ids::MAP_CHUNK);
                bytes.extend_from_slice(&x.to_be_bytes());
                bytes.extend_from_slice(&y.to_be_bytes());
                bytes.extend_from_slice(&z.to_be_bytes());
                bytes.push(*size_x);
                bytes.push(*size_y);
                bytes.push(*size_z);
                bytes.extend_from_slice(&(compressed_data.len() as i32).to_be_bytes());
                bytes.extend_from_slice(compressed_data);
            }
        }
        
        bytes
    }
}

async fn handle_packet(packet: ClientPacket, socket: &mut TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    match packet {
        ClientPacket::KeepAlive(value) => {
            println!("Keep alive received");
            send_packet(socket, ServerPacket::KeepAlive(value)).await?;
        }
        ClientPacket::Handshake(username) => {
            println!("Handshake received from: {}", username);
            send_packet(socket, ServerPacket::Handshake("-".to_string())).await?;
        }
        ClientPacket::LoginRequest { protocol_version: _, username, map_seed: _, dimension: _ } => {
            println!("Login request received from: {}", username);
            handle_login(socket).await?;
        }
        ClientPacket::PlayerPosition { x, y, stance, z, on_ground } => {
            println!("Player position: x={}, y={}, stance={}, z={}, on_ground={}", x, y, stance, z, on_ground);
        }
        ClientPacket::PlayerPositionAndLook { x, y, stance, z, yaw, pitch, on_ground } => {
            println!("Player position and look: x={}, y={}, stance={}, z={}, yaw={}, pitch={}, on_ground={}", 
                x, y, stance, z, yaw, pitch, on_ground);
        }
    }
    Ok(())
}

async fn send_packet(socket: &mut TcpStream, packet: ServerPacket) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = packet.to_bytes();
    socket.write_all(&bytes).await?;
    Ok(())
}

async fn handle_login(socket: &mut TcpStream) -> Result<(), Box<dyn std::error::Error>> {
    // Send login response
    send_packet(socket, ServerPacket::LoginResponse {
        entity_id: 1,
        level_type: "".to_string(),
        map_seed: 1234,
        game_mode: 0,
        dimension: 0,
        difficulty: 1,
        world_height: 127,
        max_players: 20,
    }).await?;

    // Send spawn position first
    send_packet(socket, ServerPacket::SpawnPosition {
        x: 0,
        y: 64,
        z: 0,
    }).await?;

    // IMPORTANT: Send chunks BEFORE player position
    // Send a small 3x3 plain of chunks around spawn
    println!("Sending chunks...");
    for cz in -1..=1 {
        for cx in -1..=1 {
            send_grass_chunk(socket, cx, cz).await?;
        }
    }
    println!("All chunks sent!");

    // Now send player position after chunks are loaded
    println!("Sending player position...");
    send_packet(socket, ServerPacket::PlayerPositionAndLook {
        x: 0.5,
        y: 65.0,
        stance: 66.62,  // Y + 1.62 (eye height)
        z: 0.5,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
    }).await?;
    println!("Login sequence complete!");

    Ok(())
}

fn generate_grass_plain_chunk(size_x: u8, size_y: u8, size_z: u8) -> Vec<u8> {
    // Block IDs
    const AIR: u8 = 0;
    const STONE: u8 = 1;
    const GRASS: u8 = 2;
    const DIRT: u8 = 3;
    
    const GROUND_LEVEL: u8 = 63;  // Top grass block
    const DIRT_LAYERS: u8 = 3;
    
    let blocks_size = (size_x as usize) * (size_y as usize) * (size_z as usize);
    
    // Data size as per spec: (Size_X+1) * (Size_Y+1) * (Size_Z+1) * 2.5 bytes
    let total_size = blocks_size + (blocks_size / 2) * 3; // blocks + metadata + block_light + sky_light
    
    let mut data = Vec::with_capacity(total_size);
    
    // Block type array - index = y + (z * Size_Y) + (x * Size_Y * Size_Z)
    // This means: for x, for z, for y (X outer, Z middle, Y inner)
    for _x in 0..size_x {
        for _z in 0..size_z {
            for y in 0..size_y {
                let block = if y < GROUND_LEVEL - DIRT_LAYERS {
                    STONE
                } else if y < GROUND_LEVEL {
                    DIRT
                } else if y == GROUND_LEVEL {
                    GRASS
                } else {
                    AIR
                };
                data.push(block);
            }
        }
    }
    
    // Metadata array (nibbles) - same iteration order, pack 2 per byte
    // Low 4 bits = lower Y, high 4 bits = higher Y
    for _x in 0..size_x {
        for _z in 0..size_z {
            for _y in (0..size_y).step_by(2) {
                // Pack two nibbles: y and y+1
                let nibble_low = 0u8;  // metadata for y
                let nibble_high = 0u8; // metadata for y+1
                data.push((nibble_high << 4) | nibble_low);
            }
        }
    }
    
    // Block light array (nibbles) - same pattern
    for _x in 0..size_x {
        for _z in 0..size_z {
            for _y in (0..size_y).step_by(2) {
                let nibble_low = 0u8;
                let nibble_high = 0u8;
                data.push((nibble_high << 4) | nibble_low);
            }
        }
    }
    
    // Sky light array (nibbles) - full brightness above ground
    for _x in 0..size_x {
        for _z in 0..size_z {
            for y in (0..size_y).step_by(2) {
                // Each nibble is 0xF (full brightness) above ground, 0x0 below
                let nibble_low = if y > GROUND_LEVEL { 0xF } else { 0x0 };
                let nibble_high = if y + 1 > GROUND_LEVEL { 0xF } else { 0x0 };
                data.push((nibble_high << 4) | nibble_low);
            }
        }
    }
    
    data
}

async fn send_grass_chunk(socket: &mut TcpStream, chunk_x: i32, chunk_z: i32) -> Result<(), Box<dyn std::error::Error>> {
    const CHUNK_SIZE_X: u8 = 16;
    const CHUNK_SIZE_Y: u8 = 128;
    const CHUNK_SIZE_Z: u8 = 16;
    
    // Calculate expected size per spec: (Size_X+1) * (Size_Y+1) * (Size_Z+1) * 2.5
    let blocks = (CHUNK_SIZE_X as usize) * (CHUNK_SIZE_Y as usize) * (CHUNK_SIZE_Z as usize);
    let expected_total = (blocks as f32 * 2.5) as usize;
    
    // Generate terrain data
    let data = generate_grass_plain_chunk(CHUNK_SIZE_X, CHUNK_SIZE_Y, CHUNK_SIZE_Z);
    
    println!("Chunk ({}, {}): generated {} bytes (expected {})", 
        chunk_x, chunk_z, data.len(), expected_total);
    println!("  First 32 bytes of uncompressed: {:02X?}", &data[..32.min(data.len())]);
    
    // Compress using zlib format (deflate with zlib wrapper)
    // Minecraft expects standard zlib format with header and checksum
    // compress_slice needs a pre-allocated buffer
    let mut compress_buffer = vec![0u8; data.len() * 2]; // Allocate enough space
    let (compressed_slice, status) = zlib_rs::compress_slice(
        &mut compress_buffer, 
        &data, 
        zlib_rs::DeflateConfig::default()
    );
    
    // Check compression status
    if status != zlib_rs::ReturnCode::Ok {
        eprintln!("  ✗ Compression failed with status: {:?}", status);
        return Err("Compression failed".into());
    }
    
    // Copy the compressed data (compress_slice returns a slice of the buffer it used)
    let compressed_data = compressed_slice.to_vec();
    
    println!("  Compressed to {} bytes", compressed_data.len());
    
    // Verify zlib format
    if verify_zlib_format(&compressed_data) {
        println!("  ✓ Valid zlib compression format (header: {:02X} {:02X})", 
            compressed_data[0], compressed_data[1]);
    } else {
        println!("  ✗ Invalid compression format - Minecraft may reject this chunk!");
    }

    // Send pre-chunk packet (0x32)
    send_packet(socket, ServerPacket::PreChunk {
        x: chunk_x,
        z: chunk_z,
        mode: true,
    }).await?;

    // Send chunk data (0x33)
    // Per spec: X, Y, Z are BLOCK coordinates (not chunk)
    // Sizes are actual size - 1
    send_packet(socket, ServerPacket::MapChunk {
        x: chunk_x * 16,  // Convert chunk coord to block coord
        y: 0,             // Start from bedrock
        z: chunk_z * 16,  // Convert chunk coord to block coord
        size_x: CHUNK_SIZE_X - 1,  // 15 (actual size 16)
        size_y: CHUNK_SIZE_Y - 1,  // 127 (actual size 128)
        size_z: CHUNK_SIZE_Z - 1,  // 15 (actual size 16)
        compressed_data,
    }).await?;

    Ok(())
}