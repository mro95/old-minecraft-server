use bytes::BytesMut;
use bytes::Buf;
use nom::bytes::streaming::take;
use tokio::net::{TcpListener, TcpStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use nom::{bytes::streaming::tag, sequence::preceded, IResult};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:25565").await?;
    println!("Server draait op 127.0.0.1:25565");

    loop {
        let (mut socket, _) = listener.accept().await?;

        tokio::spawn(async move {
            handle_connection(socket).await;
        });
    }
}

async fn handle_connection(mut socket: TcpStream) {
    let mut buffer = BytesMut::with_capacity(1024);
    let mut read_buf = [0u8; 1024];

    loop {
        let n = match socket.read(&mut read_buf).await {
            Ok(0) => return, // Verbinding gesloten
            Ok(n) => n,
            Err(_) => return,
        };

        buffer.extend_from_slice(&read_buf[..n]);

        while !buffer.is_empty() {
            match parse_message(&buffer) {
                Ok((remaining, output)) => {
                    println!("Geparsed: {:?}", output);
                    
                    let consumed = buffer.len() - remaining.len();
                    buffer.advance(consumed); 

                    handle_packet(output, &mut socket).await;
                }
                Err(nom::Err::Incomplete(_)) => {
                    break;
                }
                Err(_) => {
                    eprintln!("Parse fout, buffer legen, buffer: {:#04x?}", read_buf[..n].to_vec());
                    buffer.clear();
                    break;
                }
            }
        }
    }
}

// Example
//   0x02, Packet ID
//   0x00, 
//   0x05,
//   0x00,
//   0x4d,
//   0x00,
//   0x72,
//   0x00,
//   0x6f,
//   0x00,
//   0x39,
//   0x00,
//   0x35,

fn parse_message(input: &[u8]) -> IResult<&[u8], ClientServerPackets> {
    // Parse the packet ID (1 byte)
    let (input, packet_id) = take(1usize)(input)?;
    match packet_id[0] {
        // 0x00 is Keep Alive
        0x00 => {
            // parse the Keep Alive packet (4 bytes of random data)
            let (input, keep_alive_data) = take(4usize)(input)?;
            let keep_alive_value = i32::from_be_bytes([keep_alive_data[0], keep_alive_data[1], keep_alive_data[2], keep_alive_data[3]]);
            Ok((input, ClientServerPackets::KeepAlive(keep_alive_value)))
        }
        0x02 => {
            // Handshake packet
            let (input, len) = take(2usize)(input)?; // Length of the username (2 bytes)
            let username_len = u16::from_be_bytes([len[0], len[1]]) as usize;
            let (input, username) = take(username_len * 2)(input)?; // Username (UTF-16, so 2 bytes per character)


            Ok((input, ClientServerPackets::Handshake(String::from_utf16_lossy(
                &username
                    .chunks(2)
                    .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                    .collect::<Vec<u16>>(),
            ))))
        }
        0x01 => {
            // Login Start packet
            let (input, protocol_version) = take(4usize)(input)?; // Protocol version (4 byte)
            let protocol_version = i32::from_be_bytes([protocol_version[0], protocol_version[1], protocol_version[2], protocol_version[3]]);
            let (input, username_len) = take(2usize)(input)?; // Length of the username (2 bytes)
            let username_len = u16::from_be_bytes([username_len[0], username_len[1]]) as usize;
            let (input, username) = take(username_len * 2)(input)?; // Username (UTF-16, so 2 bytes per character)
            let (input, map_seed) = take(8usize)(input)?; // Map seed (8 bytes)
            let map_seed = i64::from_be_bytes([
                map_seed[0], map_seed[1], map_seed[2], map_seed[3],
                map_seed[4], map_seed[5], map_seed[6], map_seed[7],
            ]);
            let (input, dimension) = take(1usize)(input)?; // Dimension (1 byte)
            let dimension = dimension[0] as i8;
            Ok((input, ClientServerPackets::LoginStart {
                protocol_version: protocol_version,
                username: String::from_utf16_lossy(
                    &username
                        .chunks(2)
                        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                        .collect::<Vec<u16>>(),
                ),
                map_seed,
                dimension,
            }))
        }
        // Player Position and Look packet (0x0b)
        0x0b => {
            let (input, x) = take(8usize)(input)?; // X coordinate (8 bytes)
            let x = f64::from_be_bytes([
                x[0], x[1], x[2], x[3],
                x[4], x[5], x[6], x[7],
            ]);
            let (input, y) = take(8usize)(input)?; // Y coordinate (8 bytes)
            let y = f64::from_be_bytes([
                y[0], y[1], y[2], y[3],
                y[4], y[5], y[6], y[7],
            ]);
            let (input, stance) = take(8usize)(input)?; // Stance
            let stance = f64::from_be_bytes([
                stance[0], stance[1], stance[2], stance[3],
                stance[4], stance[5], stance[6], stance[7],
            ]);
            let (input, z) = take(8usize)(input)?; // Z coordinate
            let z = f64::from_be_bytes([
                z[0], z[1], z[2], z[3],
                z[4], z[5], z[6], z[7],
            ]);
            let (input, on_ground) = take(1usize)(input)?; // On ground (1 byte)
            let on_ground = on_ground[0] != 0; // Convert to bool
            Ok((input, ClientServerPackets::PlayerPosition {
                x,
                y,
                stance,
                z,
                on_ground,
            }))
        }
        // Player Position and Look packet (0x0d)
        0x0d => {
            let (input, x) = take(8usize)(input)?; // X coordinate (8 bytes)
            let x = f64::from_be_bytes([
                x[0], x[1], x[2], x[3],
                x[4], x[5], x[6], x[7],
            ]);
            let (input, y) = take(8usize)(input)?; // Y coordinate (8 bytes)
            let y = f64::from_be_bytes([
                y[0], y[1], y[2], y[3],
                y[4], y[5], y[6], y[7],
            ]);
            let (input, stance) = take(8usize)(input)?; // Stance
            let stance = f64::from_be_bytes([
                stance[0], stance[1], stance[2], stance[3],
                stance[4], stance[5], stance[6], stance[7],
            ]);
            let (input, z) = take(8usize)(input)?; // Z coordinate
            let z = f64::from_be_bytes([
                z[0], z[1], z[2], z[3],
                z[4], z[5], z[6], z[7],
            ]);
            let (input, yaw) = take(4usize)(input)?; // Yaw
            let yaw = f32::from_be_bytes([yaw[0], yaw[1], yaw[2], yaw[3]]);
            let (input, pitch) = take(4usize)(input)?; // Pitch
            let pitch = f32::from_be_bytes([pitch[0], pitch[1], pitch[2], pitch[3]]);
            let (input, on_ground) = take(1usize)(input)?; // On ground (1 byte)
            let on_ground = on_ground[0] != 0; // Convert to bool
            Ok((input, ClientServerPackets::PlayerPositionAndLook {
                x,
                y,
                stance,
                z,
                yaw,
                pitch,
                on_ground,
            }))
        }
        _ => {
            // Onbekend packet ID
            Err(nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Tag)))
        }
    }
}

#[derive(Debug)]
enum ClientServerPackets {
    // 0x00 is Keep Alive
    KeepAlive(i32), // Random integer sent by the client to keep the connection alive.

    // 0x02
    Handshake(String), // Username
    //0x01
    LoginStart {
        protocol_version: i32,
        username: String,
        map_seed: i64,
        dimension: i8,
    },
    // 0x0b
    PlayerPosition {
        x: f64,
        y: f64,
        stance: f64, // Used to modify the player's hitbox height
        z: f64,
        on_ground: bool,
    },
    // 0x0d Player Position and Look
    PlayerPositionAndLook {
        x: f64,
        y: f64,
        stance: f64, // Used to modify the player's hitbox height
        z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    }
}

enum ServerClientPackets {
    // 0x00
    KeepAlive(i32), // Random integer sent by the client to keep the connection alive, used for latency checks

    // 0x02
    Handshake(String), // Hash

    // 0x01
    LoginSuccess {
        entity_id: i32,
        level_type: String,
        game_mode: u8,
        dimension: i8,
        difficulty: u8,
        max_players: u8,
    }


}

fn handle_packet(packet: ClientServerPackets, socket: &mut TcpStream) -> impl std::future::Future<Output = ()> {
    async move {
        match packet {
            ClientServerPackets::KeepAlive(_) => {
                println!("Keep Alive ontvangen");
                
                let response = [0x00, 0x00, 0x00, 0x00]; // Keep Alive response (4 bytes of random data)
                if let Err(e) = socket.write_all(&response).await {
                    eprintln!("Fout bij het verzenden van keep alive response: {}", e);
                }
            }
            ClientServerPackets::Handshake(username) => {
                println!("Handshake ontvangen van gebruiker: {}", username);
                
                // Send back a handshake response (for demonstration purposes, we just send a simple message)
                // Send "-" some hash back to the client (for demonstration purposes, we use a fixed hash)
                let response = [
                    0x02, // Packet ID for Handshake response
                    0x00, 0x01, // Length of the hash (1 byte)
                    0x00, '-' as u8, // The hash character '-'
                ];
                if let Err(e) = socket.write_all(&response).await {
                    eprintln!("Fout bij het verzenden van handshake response: {}", e);
                }

                println!("Handshake response verzonden");
            }
            ClientServerPackets::LoginStart { protocol_version, username, map_seed, dimension } => {
                println!("Login Start ontvangen");
                
                let entity_id: u32 = 1;
                let level_type = "".to_string();
                let map_seed: i64 = 1234;
                let game_mode: i32 = 0;
                let dimension: u8 = 0;
                let difficulty: u8 = 1;
                let world_height: i8 = 127;
                let max_players: i8 = 20;

                // Serialize the LoginSuccess response                
                let mut response_bytes = Vec::new();
                response_bytes.push(0x01); // Packet ID for LoginSuccess
                response_bytes.extend_from_slice(&entity_id.to_be_bytes());

                let level_type_bytes = level_type.as_bytes();
                response_bytes.extend_from_slice(&(level_type_bytes.len() as u16).to_be_bytes());
                
                let map_seed_bytes = map_seed.to_be_bytes();
                response_bytes.extend_from_slice(&map_seed_bytes);

                // send every character as UTF-16 (2 bytes per character)
                for byte in level_type_bytes {
                    response_bytes.extend_from_slice(&[0x00, *byte]);
                }
                response_bytes.extend_from_slice(&game_mode.to_be_bytes());
                response_bytes.push(dimension as u8);
                response_bytes.push(difficulty);
                response_bytes.push(world_height as u8);
                response_bytes.push(max_players as u8);

                if let Err(e) = socket.write_all(&response_bytes).await {
                    eprintln!("Fout bij het verzenden van login success response: {}", e);
                }

                println!("Login success response verzonden");

                // Send Spawn Position packet (0x06) as an example of sending another packet after login success
                let spawn_x: i32 = 0;
                let spawn_y: i32 = 64;
                let spawn_z: i32 = 0;
                let mut spawn_response = Vec::new();
                spawn_response.push(0x06); // Packet ID for Spawn Position
                spawn_response.extend_from_slice(&spawn_x.to_be_bytes());
                spawn_response.extend_from_slice(&spawn_y.to_be_bytes());
                spawn_response.extend_from_slice(&spawn_z.to_be_bytes());

                if let Err(e) = socket.write_all(&spawn_response).await {
                    eprintln!("Fout bij het verzenden van spawn position response: {}", e);
                }

                // Sen Player Position & Look (0x0D)
                let x: f64 = 0.0;
                let stance: f64 = 0.0;
                let y: f64 = 64.0;
                let z: f64 = 0.0;
                let yaw: f32 = 0.0;
                let pitch: f32 = 0.0;
                let on_ground: bool = true;

                let mut position_response = Vec::new();
                position_response.push(0x0D); // Packet ID for Player Position & Look
                position_response.extend_from_slice(&x.to_be_bytes());
                position_response.extend_from_slice(&stance.to_be_bytes());
                position_response.extend_from_slice(&y.to_be_bytes());
                position_response.extend_from_slice(&z.to_be_bytes());
                position_response.extend_from_slice(&yaw.to_be_bytes());
                position_response.extend_from_slice(&pitch.to_be_bytes());
                position_response.push(if on_ground { 1 } else { 0 });

                // Send pre-chunk data (0x32) 
                let chunk_x: i32 = 0;
                let chunk_z: i32 = 0;
                let mode: bool = true; // True for load chunk, false for unload chunk
                let mut chunk_response = Vec::new();
                chunk_response.push(0x32); // Packet ID for Pre-Chunk Data
                chunk_response.extend_from_slice(&chunk_x.to_be_bytes());
                chunk_response.extend_from_slice(&chunk_z.to_be_bytes());
                chunk_response.push(if mode { 1 } else { 0 });

                if let Err(e) = socket.write_all(&position_response).await {
                    eprintln!("Fout bij het verzenden van player position response: {}", e);
                }

                // Send map chunk data (0x33)
                let chunk_x: i32 = 0;
                let chunk_y: i16 = 0;
                let chunk_z: i32 = 0;
                let size_x: u8 = 16;
                let size_y: u8 = 128;
                let size_z: u8 = 16;
                
                let data = vec![0u8; (size_x as usize) * (size_y as usize) * (size_z as usize) / 2]; // Example chunk data (half the size of the chunk, since it's compressed)
                let mut compressed_data = Vec::with_capacity(data.len());
                zlib_rs::compress_slice(&mut compressed_data, &data, zlib_rs::DeflateConfig::best_compression());

                let mut chunk_data_response = Vec::new();
                chunk_data_response.push(0x33); // Packet ID for Map Chunk Data
                chunk_data_response.extend_from_slice(&chunk_x.to_be_bytes());
                chunk_data_response.extend_from_slice(&chunk_y.to_be_bytes());
                chunk_data_response.extend_from_slice(&chunk_z.to_be_bytes());
                chunk_data_response.push(size_x);
                chunk_data_response.push(size_y);
                chunk_data_response.push(size_z);
                chunk_data_response.extend_from_slice(&(compressed_data.len() as i32).to_be_bytes());
                chunk_data_response.extend_from_slice(&compressed_data);

                if let Err(e) = socket.write_all(&chunk_data_response).await {
                    eprintln!("Fout bij het verzenden van chunk data response: {}", e);
                }
            },
            ClientServerPackets::PlayerPosition { x, y, stance, z, on_ground } => {
                println!("Player Position ontvangen: x={}, y={}, stance={}, z={}, on_ground={}", x, y, stance, z, on_ground);
            }
            ClientServerPackets::PlayerPositionAndLook { x, y, stance, z, yaw, pitch, on_ground } => {
                println!("Player Position and Look ontvangen: x={}, y={}, stance={}, z={}, yaw={}, pitch={}, on_ground={}", x, y, stance, z, yaw, pitch, on_ground);
            }
        }
    }
}