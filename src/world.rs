use thiserror::Error;
use tracing::{debug, warn};

use crate::config::{Biome, OreSettings, WorldConfig};
use rand::{rngs::StdRng, Rng};

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

pub const AIR: u8 = 0;
pub const STONE: u8 = 1;
pub const GRASS: u8 = 2;
pub const DIRT: u8 = 3;
pub const WATER: u8 = 9;
pub const SAND: u8 = 12;
pub const GRAVEL: u8 = 13;
pub const SNOW: u8 = 80;
pub const OAK_WOOD: u8 = 17;
pub const OAK_LEAVES: u8 = 18;

pub const COAL_ORE: u8 = 16;
pub const IRON_ORE: u8 = 15;
pub const GOLD_ORE: u8 = 14;
pub const DIAMOND_ORE: u8 = 56;

pub fn generate_perlin_noise_chunk(
    size_x: u8,
    size_y: u8,
    size_z: u8,
    chunk_x: i32,
    chunk_z: i32,
    config: &WorldConfig,
) -> Vec<u8> {
    use noise::{Fbm, NoiseFn, SuperSimplex};
    use rand::SeedableRng;

    assert!(
        size_x > 0 && size_y > 0 && size_z > 0,
        "Chunk dimensions must be positive"
    );

    let (sx, sy, sz) = (size_x as usize, size_y as usize, size_z as usize);
    let block_count = sx * sy * sz;
    let meta_size = (block_count + 1).div_ceil(2);

    let total_capacity = block_count + 3 * meta_size;
    let mut data = Vec::with_capacity(total_capacity);

    let base_x = (chunk_x * 16) as f64;
    let base_z = (chunk_z * 16) as f64;

    let terrain = &config.terrain;
    let caves = &config.caves;
    let ores = &config.ores;
    let biomes = &config.biomes;
    let _structures = &config.structures;

    let mut base_fbm = Fbm::<SuperSimplex>::new(config.world.seed);
    base_fbm.octaves = terrain.base_terrain.octaves;
    base_fbm.frequency = terrain.base_terrain.frequency;
    base_fbm.persistence = terrain.base_terrain.persistence;
    base_fbm.lacunarity = terrain.base_terrain.lacunarity;

    let mut detail_fbm = Fbm::<SuperSimplex>::new(config.world.seed.wrapping_add(22222));
    detail_fbm.octaves = terrain.detail.octaves;
    detail_fbm.frequency = terrain.detail.frequency;
    detail_fbm.persistence = terrain.detail.persistence;
    detail_fbm.lacunarity = terrain.detail.lacunarity;

    let mut mountain_fbm = Fbm::<SuperSimplex>::new(config.world.seed.wrapping_add(33333));
    mountain_fbm.octaves = terrain.mountains.octaves;
    mountain_fbm.frequency = terrain.mountains.frequency;
    mountain_fbm.persistence = terrain.mountains.persistence;
    mountain_fbm.lacunarity = terrain.mountains.lacunarity;

    let mut cave_fbm = Fbm::<SuperSimplex>::new(config.world.seed.wrapping_add(44444));
    cave_fbm.octaves = caves.octaves;
    cave_fbm.frequency = caves.frequency;
    cave_fbm.persistence = caves.persistence;
    cave_fbm.lacunarity = caves.lacunarity;

    let mut temp_fbm = Fbm::<SuperSimplex>::new(config.world.seed.wrapping_add(55555));
    temp_fbm.octaves = 3;
    temp_fbm.frequency = biomes.temperature_scale;
    temp_fbm.persistence = 0.5;
    temp_fbm.lacunarity = 2.0;

    let mut humidity_fbm = Fbm::<SuperSimplex>::new(config.world.seed.wrapping_add(66666));
    humidity_fbm.octaves = 3;
    humidity_fbm.frequency = biomes.humidity_scale;
    humidity_fbm.persistence = 0.5;
    humidity_fbm.lacunarity = 2.0;

    let mut river_fbm = Fbm::<SuperSimplex>::new(config.world.seed.wrapping_add(77777));
    river_fbm.octaves = 4;
    river_fbm.frequency = 0.025;
    river_fbm.persistence = 0.5;
    river_fbm.lacunarity = 2.0;

    let seed = config
        .world
        .seed
        .wrapping_add((chunk_x as u32).wrapping_mul(12345))
        .wrapping_add((chunk_z as u32).wrapping_mul(67890));
    let mut rng = StdRng::seed_from_u64(seed as u64);

    let heightmap = {
        let mut map = vec![0usize; sx * sz];
        for x in 0..sx {
            for z in 0..sz {
                let wx = base_x + x as f64;
                let wz = base_z + z as f64;

                let base_val = base_fbm.get([wx, wz]);
                let detail_val = detail_fbm.get([wx, wz]) * terrain.detail.weight;

                let mountain_val = mountain_fbm.get([wx, wz]);
                let normalized = (mountain_val + 1.0) * 0.5;
                let mountain_pow = normalized.powi(terrain.mountains.power as i32);
                let mountain_add = mountain_pow * terrain.mountains.influence * 1.5;

                let combined = base_val + detail_val;
                let ground_level = (terrain.base_height
                    + terrain.height_variation * combined
                    + mountain_add) as isize;
                map[x * sz + z] = ground_level.clamp(5, sy as isize - 1) as usize;
            }
        }
        map
    };

    let biomemap = {
        let mut map = vec![Biome::Plains; sx * sz];
        for x in 0..sx {
            for z in 0..sz {
                let wx = base_x + x as f64;
                let wz = base_z + z as f64;

                let ground_level = heightmap[x * sz + z];
                let is_below_sea = (ground_level as f64) < terrain.sea_level;

                let temp = temp_fbm.get([wx, wz]);
                let humidity = humidity_fbm.get([wx, wz]);

                let biome = if is_below_sea {
                    Biome::Ocean
                } else {
                    let height_factor = mountain_fbm.get([wx, wz]);

                    if height_factor > 0.5 {
                        Biome::Mountains
                    } else if temp > 0.3 && humidity < 0.0 {
                        Biome::Desert
                    } else if humidity > 0.2 && temp < 0.1 {
                        Biome::Taiga
                    } else if humidity > 0.3 && ground_level < (terrain.sea_level as usize + 2) {
                        Biome::Swamp
                    } else if humidity > 0.2 && temp > 0.0 {
                        Biome::Forest
                    } else if ground_level < (terrain.sea_level as usize + 1) {
                        Biome::Beach
                    } else {
                        Biome::Plains
                    }
                };

                map[x * sz + z] = biome;
            }
        }
        map
    };

    let river_map = {
        let mut map = vec![false; sx * sz];
        for x in 0..sx {
            for z in 0..sz {
                let wx = base_x + x as f64;
                let wz = base_z + z as f64;

                let river_val = river_fbm.get([wx, wz]);
                let ground_level = heightmap[x * sz + z];

                map[x * sz + z] = river_val > 0.55
                    && ground_level > (terrain.sea_level as usize - 3)
                    && ground_level < (terrain.sea_level as usize + 5);
            }
        }
        map
    };

    for x in 0..sx {
        for z in 0..sz {
            let ground_level = heightmap[x * sz + z];
            let biome = biomemap[x * sz + z];
            let is_river = river_map[x * sz + z];

            for y in 0..sy {
                let block = if is_river
                    && ground_level > (terrain.sea_level as usize - 2)
                    && ground_level < (terrain.sea_level as usize + 3)
                    && y >= (terrain.sea_level as usize - 2)
                    && y <= (terrain.sea_level as usize)
                {
                    if y < ground_level {
                        STONE
                    } else if y <= terrain.sea_level as usize {
                        WATER
                    } else {
                        AIR
                    }
                } else if y < ground_level {
                    if y < ground_level.saturating_sub(4) {
                        STONE
                    } else if y < ground_level.saturating_sub(1) {
                        biome.subsurface_block()
                    } else {
                        biome.surface_block()
                    }
                } else if (ground_level as f64) < terrain.sea_level
                    && y < terrain.sea_level as usize
                {
                    WATER
                } else {
                    AIR
                };

                if block != AIR && block != WATER {
                    if caves.enabled {
                        let cave_val =
                            cave_fbm.get([base_x + x as f64, y as f64, base_z + z as f64]);
                        let surface_distance = ground_level as isize - y as isize;
                        if cave_val < caves.threshold && surface_distance > caves.min_depth {
                            data.push(AIR);
                        } else {
                            data.push(block);
                        }
                    } else {
                        data.push(block);
                    }
                } else {
                    data.push(block);
                }
            }
        }
    }

    if ores.enabled {
        generate_ores(&mut data, sx, sy, sz, &ores.coal, &mut rng);
        generate_ores(&mut data, sx, sy, sz, &ores.iron, &mut rng);
        generate_ores(&mut data, sx, sy, sz, &ores.gold, &mut rng);
        generate_ores(&mut data, sx, sy, sz, &ores.diamond, &mut rng);
    }

    data.extend(vec![0u8; meta_size]);
    data.extend(vec![0u8; meta_size]);
    data.extend(vec![0xFFu8; meta_size]);

    debug_assert_eq!(data.len(), total_capacity, "Incorrect final data size");
    debug!(
        data_size = data.len(),
        expected_size = total_capacity,
        "Generated chunk"
    );

    data
}

fn generate_ores(
    data: &mut [u8],
    sx: usize,
    sy: usize,
    sz: usize,
    ore: &OreSettings,
    rng: &mut StdRng,
) {
    let block_count = sx * sy * sz;

    for _ in 0..ore.attempts_per_chunk {
        let ox = (rng.next_u32() as usize) % sx;
        let oy = ore.min_height
            + ((rng.next_u32() as usize) % (ore.max_height.saturating_sub(ore.min_height).max(1)));
        let oz = (rng.next_u32() as usize) % sz;

        for dx in 0..ore.vein_size {
            for dy in 0..ore.vein_size {
                for dz in 0..ore.vein_size {
                    let px = ox + dx;
                    let py = oy + dy;
                    let pz = oz + dz;

                    if px < sx && py < sy && pz < sz {
                        let idx = py + pz * sy + px * sy * sz;
                        if idx < block_count && data[idx] == STONE {
                            data[idx] = ore.block_id;
                        }
                    }
                }
            }
        }
    }
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
