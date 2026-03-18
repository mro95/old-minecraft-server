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
        _ => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Tag,
        ))),
    }
}
