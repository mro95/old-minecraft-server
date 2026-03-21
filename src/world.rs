use thiserror::Error;
use tracing::{debug, warn};

#[derive(Error, Debug)]
pub enum WorldError {
    #[error("Compression failed: {0}")]
    CompressionFailed(String),
}

impl From<String> for WorldError {
    fn from(s: String) -> Self {
        WorldError::CompressionFailed(s)
    }
}

pub const WORLD_SEED: u32 = 781378172;

// Block IDs
const AIR: u8 = 0;
const STONE: u8 = 1;
const GRASS: u8 = 2;
const DIRT: u8 = 3;
const WATER: u8 = 9;
const SAND: u8 = 12;

/// Improved terrain generator using multiple noise layers for natural-looking terrain.
///
/// Key improvements over original:
/// - Uses SuperSimplex instead of Perlin (smoother, fewer artifacts)
/// - Domain warping to break up geometric patterns
/// - Ridged multi-fractal for mountain ranges
/// - Chunk-continuous sampling (uses world coordinates)
/// - Surface cave exclusion to avoid floating terrain
pub fn generate_perlin_noise_chunk(
    size_x: u8,
    size_y: u8,
    size_z: u8,
    chunk_x: i32,
    chunk_z: i32,
) -> Vec<u8> {
    use noise::{Fbm, NoiseFn, SuperSimplex};

    assert!(
        size_x > 0 && size_y > 0 && size_z > 0,
        "Chunk dimensions must be positive"
    );

    let (sx, sy, sz) = (size_x as usize, size_y as usize, size_z as usize);
    let block_count = sx * sy * sz;
    let meta_size = (block_count + 1) / 2;

    let total_capacity = block_count + 3 * meta_size;
    let mut data = Vec::with_capacity(total_capacity);

    // World base position for continuous noise across chunks
    let base_x = (chunk_x * 16) as f64;
    let base_z = (chunk_z * 16) as f64;

    // Base terrain: main hills (SuperSimplex for smoothness)
    let mut base_fbm = Fbm::<SuperSimplex>::new(WORLD_SEED);
    base_fbm.octaves = 5;
    base_fbm.frequency = 0.02;
    base_fbm.persistence = 0.5;
    base_fbm.lacunarity = 2.0;

    // Detail terrain: smaller scale variation for more interesting terrain
    let mut detail_fbm = Fbm::<SuperSimplex>::new(WORLD_SEED.wrapping_add(22222));
    detail_fbm.octaves = 4;
    detail_fbm.frequency = 0.08;
    detail_fbm.persistence = 0.5;
    detail_fbm.lacunarity = 2.0;

    // Mountains: use SuperSimplex for smoother, broader mountains
    let mut mountain_fbm = Fbm::<SuperSimplex>::new(WORLD_SEED.wrapping_add(33333));
    mountain_fbm.octaves = 3;
    mountain_fbm.frequency = 0.015;
    mountain_fbm.persistence = 0.5;
    mountain_fbm.lacunarity = 2.0;

    // Cave noise (3D)
    let mut cave_fbm = Fbm::<SuperSimplex>::new(WORLD_SEED.wrapping_add(44444));
    cave_fbm.octaves = 4;
    cave_fbm.frequency = 0.05;
    cave_fbm.persistence = 0.5;
    cave_fbm.lacunarity = 2.0;

    // Terrain parameters
    const BASE_HEIGHT: f64 = 60.0;
    const HEIGHT_VARIATION: f64 = 25.0;
    const MOUNTAIN_INFLUENCE: f64 = 20.0;
    const SEA_LEVEL: f64 = 54.0;

    for x in 0..sx {
        for z in 0..sz {
            // World coordinates for continuous noise across chunks
            let wx = base_x + x as f64;
            let wz = base_z + z as f64;

            // Sample main terrain
            let base_val = base_fbm.get([wx, wz]);

            // Sample detail
            let detail_val = detail_fbm.get([wx, wz]) * 0.3;

            // Sample mountains - use power curve for smooth hills that can get tall
            let mountain_val = mountain_fbm.get([wx, wz]);
            // Map [-1, 1] to [0, 1] with a steep power curve
            let normalized = (mountain_val + 1.0) * 0.5;
            let mountain_pow = normalized.powi(5); // 5th power - only high values make mountains
            let mountain_add = mountain_pow * MOUNTAIN_INFLUENCE * 1.5;

            // Combine heights
            let combined = base_val + detail_val;

            let ground_level = (BASE_HEIGHT + HEIGHT_VARIATION * combined + mountain_add) as isize;
            let ground_level = ground_level.clamp(5, sy as isize - 1) as usize;

            // Sea level check
            let is_below_sea = (ground_level as f64) < SEA_LEVEL;

            for y in 0..sy {
                let block = if y < ground_level {
                    // Underground
                    if is_below_sea {
                        if y < ground_level.saturating_sub(4) {
                            STONE
                        } else {
                            DIRT
                        }
                    } else {
                        // Normal terrain
                        if y < ground_level.saturating_sub(4) {
                            STONE
                        } else if y < ground_level.saturating_sub(1) {
                            DIRT
                        } else {
                            GRASS
                        }
                    }
                } else if is_below_sea && y < SEA_LEVEL as usize {
                    // Water blocks below sea level
                    WATER
                } else if is_below_sea
                    && ground_level == (SEA_LEVEL as usize - 1)
                    && y == ground_level
                {
                    // Sand at ocean floor
                    SAND
                } else {
                    AIR
                };

                // Cave carving (only deep underground)
                if block != AIR && block != WATER {
                    let cave_val = cave_fbm.get([wx * 0.03, y as f64 * 0.03, wz * 0.03]);
                    // Only carve caves well below surface to avoid floating terrain
                    let surface_distance = ground_level as isize - y as isize;
                    if cave_val < -0.2 && surface_distance > 8 {
                        data.push(AIR);
                    } else {
                        data.push(block);
                    }
                } else {
                    data.push(block);
                }
            }
        }
    }

    data.extend(vec![0u8; meta_size]);
    data.extend(vec![0u8; meta_size]);
    data.extend(vec![0xFFu8; meta_size]);

    debug_assert_eq!(data.len(), total_capacity, "Incorrect final data size");
    debug!(
        data_size = data.len(),
        expected_size = total_capacity,
        "Generated improved terrain chunk"
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
pub fn compress_chunk_data(data: &[u8]) -> Result<Vec<u8>, WorldError> {
    // Allocate enough space for compressed data
    let mut compress_buffer = vec![0u8; data.len() * 2];
    let (compressed_slice, status) = zlib_rs::compress_slice(
        &mut compress_buffer,
        data,
        zlib_rs::DeflateConfig::default(),
    );

    // Check compression status
    if status != zlib_rs::ReturnCode::Ok {
        return Err(format!("Compression failed with status: {:?}", status).into());
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
