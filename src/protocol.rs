use nom::bytes::streaming::take;
use nom::IResult;

use crate::packet_ids;
use crate::packets::ClientPacket;

/// Parse a 32-bit big-endian integer
pub fn parse_i32(input: &[u8]) -> IResult<&[u8], i32> {
    let (input, bytes) = take(4usize)(input)?;
    Ok((
        input,
        i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    ))
}

/// Parse a 64-bit big-endian integer
pub fn parse_i64(input: &[u8]) -> IResult<&[u8], i64> {
    let (input, bytes) = take(8usize)(input)?;
    Ok((
        input,
        i64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]),
    ))
}

/// Parse a 32-bit big-endian float
pub fn parse_f32(input: &[u8]) -> IResult<&[u8], f32> {
    let (input, bytes) = take(4usize)(input)?;
    Ok((
        input,
        f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
    ))
}

/// Parse a 64-bit big-endian float
pub fn parse_f64(input: &[u8]) -> IResult<&[u8], f64> {
    let (input, bytes) = take(8usize)(input)?;
    Ok((
        input,
        f64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]),
    ))
}

/// Parse a UTF-16 encoded string (Minecraft protocol format)
pub fn parse_utf16_string(input: &[u8]) -> IResult<&[u8], String> {
    let (input, len) = take(2usize)(input)?;
    let string_len = u16::from_be_bytes([len[0], len[1]]) as usize;
    let (input, string_bytes) = take(string_len * 2)(input)?;

    let utf16_chars: Vec<u16> = string_bytes
        .chunks(2)
        .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
        .collect();

    Ok((input, String::from_utf16_lossy(&utf16_chars)))
}

/// Parse a boolean value
pub fn parse_bool(input: &[u8]) -> IResult<&[u8], bool> {
    let (input, byte) = take(1usize)(input)?;
    Ok((input, byte[0] != 0))
}

/// Parse a client packet from raw bytes
pub fn parse_packet(input: &[u8]) -> IResult<&[u8], ClientPacket> {
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
            
            // Modern protocol (Beta 1.8+) may have additional fields
            // Try to consume them if present: game_mode (i32), dimension (i8), difficulty (i8), world_height (u8), max_players (u8)
            // For compatibility, consume remaining bytes if available (some clients send extra data)
            let input = if input.len() >= 7 {
                let (input, _extra) = take(7usize)(input)?;
                input
            } else {
                input
            };

            Ok((
                input,
                ClientPacket::LoginRequest {
                    protocol_version,
                    username,
                    map_seed,
                    dimension: dimension[0] as i8,
                },
            ))
        }
        packet_ids::CHAT_MESSAGE => {
            let (input, message) = parse_utf16_string(input)?;
            Ok((input, ClientPacket::ChatMessage(message)))
        }
        packet_ids::PLAYER => {
            let (input, on_ground) = parse_bool(input)?;
            Ok((input, ClientPacket::Player { on_ground }))
        }
        packet_ids::PLAYER_POSITION => {
            let (input, x) = parse_f64(input)?;
            let (input, y) = parse_f64(input)?;
            let (input, stance) = parse_f64(input)?;
            let (input, z) = parse_f64(input)?;
            let (input, on_ground) = parse_bool(input)?;

            Ok((
                input,
                ClientPacket::PlayerPosition {
                    x,
                    y,
                    stance,
                    z,
                    on_ground,
                },
            ))
        }
        packet_ids::PLAYER_LOOK => {
            let (input, yaw) = parse_f32(input)?;
            let (input, pitch) = parse_f32(input)?;
            let (input, on_ground) = parse_bool(input)?;

            Ok((
                input,
                ClientPacket::PlayerLook {
                    yaw,
                    pitch,
                    on_ground,
                },
            ))
        }
        packet_ids::PLAYER_POSITION_AND_LOOK => {
            let (input, x) = parse_f64(input)?;
            let (input, y) = parse_f64(input)?;
            let (input, stance) = parse_f64(input)?;
            let (input, z) = parse_f64(input)?;
            let (input, yaw) = parse_f32(input)?;
            let (input, pitch) = parse_f32(input)?;
            let (input, on_ground) = parse_bool(input)?;

            Ok((
                input,
                ClientPacket::PlayerPositionAndLook {
                    x,
                    y,
                    stance,
                    z,
                    yaw,
                    pitch,
                    on_ground,
                },
            ))
        }
        packet_ids::PLAYER_DIGGING => {
            let (input, status) = take(1usize)(input)?;
            let (input, x) = parse_i32(input)?;
            let (input, y) = take(1usize)(input)?;
            let (input, z) = parse_i32(input)?;
            let (input, face) = take(1usize)(input)?;

            Ok((
                input,
                ClientPacket::PlayerDigging {
                    status: status[0] as i8,
                    x,
                    y: y[0] as i8,
                    z,
                    face: face[0] as i8,
                },
            ))
        }
        packet_ids::PLAYER_BLOCK_PLACEMENT => {
            let (input, x) = parse_i32(input)?;
            let (input, y) = take(1usize)(input)?;
            let (input, z) = parse_i32(input)?;
            let (input, direction) = take(1usize)(input)?;
            let (input, held_item_bytes) = take(2usize)(input)?;
            let held_item = i16::from_be_bytes([held_item_bytes[0], held_item_bytes[1]]);

            // Note: Full implementation would need to handle item stack data if held_item != -1
            // For now, we'll assume no additional data

            Ok((
                input,
                ClientPacket::PlayerBlockPlacement {
                    x,
                    y: y[0] as i8,
                    z,
                    direction: direction[0] as i8,
                    held_item,
                },
            ))
        }
        packet_ids::HOLDING_CHANGE => {
            let (input, slot_bytes) = take(2usize)(input)?;
            let slot = i16::from_be_bytes([slot_bytes[0], slot_bytes[1]]);
            Ok((input, ClientPacket::HoldingChange { slot }))
        }
        packet_ids::ANIMATION => {
            let (input, entity_id) = parse_i32(input)?;
            let (input, animation) = take(1usize)(input)?;
            Ok((
                input,
                ClientPacket::Animation {
                    entity_id,
                    animation: animation[0] as i8,
                },
            ))
        }
        packet_ids::ENTITY_ACTION => {
            let (input, entity_id) = parse_i32(input)?;
            let (input, action) = take(1usize)(input)?;
            Ok((
                input,
                ClientPacket::EntityAction {
                    entity_id,
                    action: action[0] as i8,
                },
            ))
        }
        packet_ids::DISCONNECT => {
            let (input, reason) = parse_utf16_string(input)?;
            Ok((input, ClientPacket::Disconnect(reason)))
        }
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        ))),
    }
}
