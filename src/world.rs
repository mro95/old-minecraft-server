use tracing::{debug, warn};

// Block IDs
const AIR: u8 = 0;
const STONE: u8 = 1;
const GRASS: u8 = 2;
const DIRT: u8 = 3;

const GROUND_LEVEL: u8 = 63; // Top grass block
const DIRT_LAYERS: u8 = 3;

/// Generate chunk data for a flat grass plain
pub fn generate_grass_plain_chunk(size_x: u8, size_y: u8, size_z: u8) -> Vec<u8> {
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
                let nibble_low = 0u8; // metadata for y
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

    debug!(
        data_size = data.len(),
        expected_size = total_size,
        "Generated chunk data"
    );

    data
}

/// Verify that compressed data is in valid zlib format
pub fn verify_zlib_format(data: &[u8]) -> bool {
    if data.len() < 6 {
        warn!("Compressed data too small for valid zlib format");
        return false; // Too small for valid zlib (2 byte header + data + 4 byte checksum)
    }

    // Check zlib header magic bytes
    // 0x78 = deflate compression method
    // Second byte varies based on compression level and window size
    let valid_header =
        data[0] == 0x78 && (data[1] == 0x01 || data[1] == 0x9C || data[1] == 0xDA);

    if !valid_header {
        warn!(
            header_byte1 = format!("{:02X}", data[0]),
            header_byte2 = format!("{:02X}", data[1]),
            "Invalid zlib header, expected 0x78 followed by 0x01/0x9C/0xDA"
        );
    }

    valid_header
}

/// Compress chunk data using zlib format
pub fn compress_chunk_data(data: &[u8]) -> Result<Vec<u8>, String> {
    // Allocate enough space for compressed data
    let mut compress_buffer = vec![0u8; data.len() * 2];
    let (compressed_slice, status) =
        zlib_rs::compress_slice(&mut compress_buffer, data, zlib_rs::DeflateConfig::default());

    // Check compression status
    if status != zlib_rs::ReturnCode::Ok {
        return Err(format!("Compression failed with status: {:?}", status));
    }

    // Copy the compressed data
    let compressed_data = compressed_slice.to_vec();

    debug!(
        original_size = data.len(),
        compressed_size = compressed_data.len(),
        ratio = format!("{:.2}%", (compressed_data.len() as f64 / data.len() as f64) * 100.0),
        "Compressed chunk data"
    );

    Ok(compressed_data)
}
