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

/// Generates a chunk using 3D fractal Perlin noise for more natural terrain.
///
/// # Arguments
/// * `size_x`, `size_y`, `size_z` – Chunk dimensions (max 255 each).
/// * `seed` – Base seed for the noise generators.
///
/// # Returns
/// A `Vec<u8>` containing block IDs followed by metadata, block light,
/// and sky light sections. Blocks are stored in YZX order (Y fastest).
///
/// # Panics
/// Panics if any dimension is zero or greater than 256 (arbitrary safety limit).
pub fn generate_perlin_noise_chunk(size_x: u8, size_y: u8, size_z: u8, seed: u32) -> Vec<u8> {
    use noise::{Fbm, NoiseFn, Perlin, Seedable};

    // Validate dimensions (optional but recommended)
    assert!(
        size_x > 0 && size_y > 0 && size_z > 0,
        "Chunk dimensions must be positive"
    );
    //assert!(size_x <= 256 && size_y <= 256 && size_z <= 256, "Dimensions exceed safety limit");

    let (sx, sy, sz) = (size_x as usize, size_y as usize, size_z as usize);
    let block_count = sx * sy * sz;
    let meta_size = (block_count + 1) / 2; // 4 bits per block → half the block count, rounded up

    // Pre‑allocate exact capacity: blocks + metadata + block_light + sky_light
    let total_capacity = block_count + 3 * meta_size;
    let mut data = Vec::with_capacity(total_capacity);

    // --- Noise setup ---
    // Heightmap uses 2D fractal noise (FBM) for more natural terrain with detail.
    let mut height_fbm = Fbm::<Perlin>::new(seed);
    height_fbm.octaves = 4; // Number of noise layers
    height_fbm.frequency = 0.03; // Lower frequency = larger features (was 1/10 = 0.1)
    height_fbm.persistence = 0.5; // Amplitude decay per octave
    height_fbm.lacunarity = 2.0; // Frequency multiplier per octave

    // Cave noise uses 3D fractal noise with a shifted seed for independence.
    let mut cave_fbm = Fbm::<Perlin>::new(seed.wrapping_add(12345));
    cave_fbm.octaves = 3;
    cave_fbm.frequency = 0.06; // Lower = fewer, larger caves
    cave_fbm.persistence = 0.5;
    cave_fbm.lacunarity = 2.0;

    // Height range control – avoids extreme mountains and pits.
    const BASE_HEIGHT: f64 = 64.0; // Average ground level
    const HEIGHT_AMP: f64 = 24.0; // Max deviation from base (so terrain 40–88 typically)
    // Noise range for FBM with default settings is roughly [-1, 1], but can exceed.
    // We'll clamp final height to safe bounds.

    // Cave threshold: noise values below -threshold become air.
    const CAVE_THRESHOLD: f64 = 0.15; // Lower = more caves

    // --- Block generation (YZX order: Y fastest, then Z, then X) ---
    for x in 0..sx {
        for z in 0..sz {
            // Height sample (2D) – determines the ground surface
            let height_val = height_fbm.get([x as f64, z as f64]);
            let ground_level = (BASE_HEIGHT + HEIGHT_AMP * height_val) as isize;
            // Clamp to valid range [0, sy-1]
            let ground_level = ground_level.clamp(0, sy as isize - 1) as usize;

            for y in 0..sy {
                let block = if y < ground_level {
                    // Underground layers
                    if y < ground_level.saturating_sub(4) {
                        STONE
                    } else if y < ground_level.saturating_sub(1) {
                        DIRT
                    } else {
                        GRASS
                    }
                } else {
                    AIR
                };

                // Apply 3D cave noise – if below threshold, carve air
                if block != AIR {
                    let cave_val = cave_fbm.get([x as f64, y as f64, z as f64]);
                    if cave_val < -CAVE_THRESHOLD {
                        data.push(AIR);
                    } else {
                        data.push(block);
                    }
                } else {
                    data.push(AIR);
                }
            }
        }
    }

    // Append metadata, block light, and sky light sections (simplified)
    data.extend(vec![0u8; meta_size]); // Metadata (all zero)
    data.extend(vec![0u8; meta_size]); // Block light (all zero)
    data.extend(vec![0xFFu8; meta_size]); // Sky light (full brightness)

    debug_assert_eq!(data.len(), total_capacity, "Incorrect final data size");
    debug!(
        data_size = data.len(),
        expected_size = total_capacity,
        "Generated fractal noise chunk with caves"
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
    let valid_header = data[0] == 0x78 && (data[1] == 0x01 || data[1] == 0x9C || data[1] == 0xDA);

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
    let (compressed_slice, status) = zlib_rs::compress_slice(
        &mut compress_buffer,
        data,
        zlib_rs::DeflateConfig::default(),
    );

    // Check compression status
    if status != zlib_rs::ReturnCode::Ok {
        return Err(format!("Compression failed with status: {:?}", status));
    }

    // Copy the compressed data
    let compressed_data = compressed_slice.to_vec();

    debug!(
        original_size = data.len(),
        compressed_size = compressed_data.len(),
        ratio = format!(
            "{:.2}%",
            (compressed_data.len() as f64 / data.len() as f64) * 100.0
        ),
        "Compressed chunk data"
    );

    Ok(compressed_data)
}
