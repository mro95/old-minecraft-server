use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biome {
    Plains,
    Desert,
    Forest,
    Mountains,
    Ocean,
    Beach,
    Taiga,
    Swamp,
}

impl Biome {
    pub fn surface_block(&self) -> u8 {
        match self {
            Biome::Desert | Biome::Beach => 12, // Sand
            Biome::Swamp => 13,                 // Gravel
            _ => 2,                             // Grass
        }
    }

    pub fn subsurface_block(&self) -> u8 {
        match self {
            Biome::Desert | Biome::Beach => 12, // Sand
            Biome::Swamp => 13,                 // Gravel
            _ => 3,                             // Dirt
        }
    }

    pub fn has_trees(&self) -> bool {
        matches!(self, Biome::Forest | Biome::Taiga | Biome::Swamp)
    }

    pub fn has_snow(&self) -> bool {
        matches!(self, Biome::Mountains | Biome::Taiga)
    }

    pub fn is_warm(&self) -> bool {
        matches!(self, Biome::Desert | Biome::Plains)
    }
}

#[derive(Debug, Deserialize)]
pub struct WorldConfig {
    pub world: WorldSettings,
    pub terrain: TerrainConfig,
    pub biomes: BiomesConfig,
    pub caves: CavesConfig,
    pub ores: OresConfig,
    pub structures: StructuresConfig,
}

#[derive(Debug, Deserialize)]
pub struct WorldSettings {
    pub seed: u32,
}

#[derive(Debug, Deserialize)]
pub struct TerrainConfig {
    pub base_height: f64,
    pub height_variation: f64,
    pub sea_level: f64,
    pub base_terrain: NoiseSettings,
    pub detail: NoiseSettingsWithWeight,
    pub mountains: MountainSettings,
}

#[derive(Debug, Deserialize)]
pub struct NoiseSettings {
    pub frequency: f64,
    pub octaves: usize,
    pub persistence: f64,
    pub lacunarity: f64,
}

#[derive(Debug, Deserialize)]
pub struct NoiseSettingsWithWeight {
    pub frequency: f64,
    pub octaves: usize,
    pub persistence: f64,
    pub lacunarity: f64,
    pub weight: f64,
}

#[derive(Debug, Deserialize)]
pub struct MountainSettings {
    pub frequency: f64,
    pub octaves: usize,
    pub persistence: f64,
    pub lacunarity: f64,
    pub influence: f64,
    pub power: u32,
}

#[derive(Debug, Deserialize)]
pub struct BiomesConfig {
    pub enabled: bool,
    pub temperature_scale: f64,
    pub humidity_scale: f64,
}

#[derive(Debug, Deserialize)]
pub struct CavesConfig {
    pub enabled: bool,
    pub frequency: f64,
    pub octaves: usize,
    pub persistence: f64,
    pub lacunarity: f64,
    pub threshold: f64,
    pub min_depth: isize,
}

#[derive(Debug, Deserialize)]
pub struct OresConfig {
    pub enabled: bool,
    pub coal: OreSettings,
    pub iron: OreSettings,
    pub gold: OreSettings,
    pub diamond: OreSettings,
}

#[derive(Debug, Deserialize)]
pub struct OreSettings {
    pub block_id: u8,
    pub vein_size: usize,
    pub attempts_per_chunk: usize,
    pub min_height: usize,
    pub max_height: usize,
}

#[derive(Debug, Deserialize)]
pub struct StructuresConfig {
    pub trees_enabled: bool,
    pub tree_chance: f64,
}

impl Default for WorldConfig {
    fn default() -> Self {
        toml::from_str(DEFAULT_CONFIG).expect("Default config should be valid")
    }
}

impl WorldConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let contents = fs::read_to_string(path)?;
        let config: WorldConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn load_or_default<P: AsRef<Path>>(path: P) -> Self {
        Self::load(path).unwrap_or_default()
    }
}

const DEFAULT_CONFIG: &str = r#"
[world]
seed = 781378172

[terrain]
base_height = 60.0
height_variation = 25.0
sea_level = 54.0

[terrain.base_terrain]
frequency = 0.02
octaves = 5
persistence = 0.5
lacunarity = 2.0

[terrain.detail]
frequency = 0.08
octaves = 4
persistence = 0.5
lacunarity = 2.0
weight = 0.3

[terrain.mountains]
frequency = 0.015
octaves = 3
persistence = 0.5
lacunarity = 2.0
influence = 20.0
power = 5

[biomes]
enabled = true
temperature_scale = 0.008
humidity_scale = 0.008

[structures]
trees_enabled = true
tree_chance = 0.05

[structures.rivers]
enabled = true
frequency = 0.01

[caves]
enabled = true
frequency = 0.05
octaves = 4
persistence = 0.5
lacunarity = 2.0
threshold = -0.2
min_depth = 8

[ores]
enabled = true

[ores.coal]
block_id = 16
vein_size = 8
attempts_per_chunk = 15
min_height = 5
max_height = 128

[ores.iron]
block_id = 15
vein_size = 6
attempts_per_chunk = 10
min_height = 5
max_height = 64

[ores.gold]
block_id = 14
vein_size = 5
attempts_per_chunk = 5
min_height = 5
max_height = 32

[ores.diamond]
block_id = 56
vein_size = 4
attempts_per_chunk = 2
min_height = 5
max_height = 16
"#;
