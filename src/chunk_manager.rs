use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::{interval, sleep};
use tokio::{sync::RwLock, time::MissedTickBehavior};
use tracing::{debug, error, info, warn};

use crate::config::WorldConfig;
use crate::world::{compress_chunk_data, generate_perlin_noise_chunk};

pub const CHUNK_SIZE_X: usize = 16;
pub const CHUNK_SIZE_Y: usize = 128;
pub const CHUNK_SIZE_Z: usize = 16;
pub const CHUNK_SIZE_X_U8: u8 = 16;
pub const CHUNK_SIZE_Y_U8: u8 = 128;
pub const CHUNK_SIZE_Z_U8: u8 = 16;

pub const VIEW_DISTANCE: i32 = 4;

const SAVE_INTERVAL_SECS: u64 = 60;
const PREFETCH_DELAY_MS: u64 = 20;
const CHUNKS_PER_BATCH: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub fn from_world_pos(world_x: i32, world_z: i32) -> Self {
        Self {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
        }
    }

    pub fn filename(&self) -> String {
        format!("chunk_{}_{}.bin", self.x, self.z)
    }

    pub fn get_block_pos(&self) -> (i32, i32) {
        (self.x * 16, self.z * 16)
    }

    pub fn chunks_in_radius(center: ChunkPos, radius: i32) -> Vec<ChunkPos> {
        let mut positions = Vec::with_capacity(((2 * radius + 1) * (2 * radius + 1)) as usize);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                positions.push(ChunkPos::new(center.x + dx, center.z + dz));
            }
        }
        positions
    }

    pub fn chunks_to_load(old_center: ChunkPos, new_center: ChunkPos, radius: i32) -> Vec<ChunkPos> {
        let old_set: HashSet<_> = Self::chunks_in_radius(old_center, radius).into_iter().collect();
        Self::chunks_in_radius(new_center, radius)
            .into_iter()
            .filter(|pos| !old_set.contains(pos))
            .collect()
    }

    pub fn chunks_to_unload(old_center: ChunkPos, new_center: ChunkPos, radius: i32) -> Vec<ChunkPos> {
        let new_set: HashSet<_> = Self::chunks_in_radius(new_center, radius).into_iter().collect();
        Self::chunks_in_radius(old_center, radius)
            .into_iter()
            .filter(|pos| !new_set.contains(pos))
            .collect()
    }
}

#[derive(Clone)]
pub struct Chunk {
    pub pos: ChunkPos,
    pub blocks: Box<[u8; CHUNK_SIZE_X * CHUNK_SIZE_Y * CHUNK_SIZE_Z]>,
    pub modified: bool,
}

impl Chunk {
    pub fn new(pos: ChunkPos) -> Self {
        Self {
            pos,
            blocks: Box::new([0u8; CHUNK_SIZE_X * CHUNK_SIZE_Y * CHUNK_SIZE_Z]),
            modified: false,
        }
    }

    pub fn index(x: usize, y: usize, z: usize) -> usize {
        debug_assert!(x < CHUNK_SIZE_X && y < CHUNK_SIZE_Y && z < CHUNK_SIZE_Z);
        y + z * CHUNK_SIZE_Y + x * CHUNK_SIZE_Y * CHUNK_SIZE_Z
    }

    pub fn get_block(&self, x: usize, y: usize, z: usize) -> u8 {
        self.blocks[Self::index(x, y, z)]
    }

    pub fn set_block(&mut self, x: usize, y: usize, z: usize, block_id: u8) {
        let idx = Self::index(x, y, z);
        if self.blocks[idx] != block_id {
            self.blocks[idx] = block_id;
            self.modified = true;
        }
    }

    pub fn to_network_data(&self) -> Vec<u8> {
        let block_count = CHUNK_SIZE_X * CHUNK_SIZE_Y * CHUNK_SIZE_Z;
        let meta_size = (block_count + 1).div_ceil(2);

        let mut data = Vec::with_capacity(block_count + 3 * meta_size);

        data.extend_from_slice(&self.blocks[..]);

        data.extend(vec![0u8; meta_size]);
        data.extend(vec![0u8; meta_size]);
        data.extend(vec![0xFFu8; meta_size]);

        data
    }
}

pub struct ChunkManager {
    chunks: RwLock<HashMap<ChunkPos, Chunk>>,
    world_dir: PathBuf,
    config: WorldConfig,
}

impl ChunkManager {
    pub fn new(world_dir: impl Into<PathBuf>, config: WorldConfig) -> Self {
        Self {
            chunks: RwLock::new(HashMap::new()),
            world_dir: world_dir.into(),
            config,
        }
    }

    pub fn world_dir(&self) -> &PathBuf {
        &self.world_dir
    }

    pub async fn get_chunk(&self, pos: ChunkPos) -> Option<Arc<Chunk>> {
        let chunks = self.chunks.read().await;
        chunks.get(&pos).map(|c| Arc::new(c.clone()))
    }

    pub async fn load_or_generate(&self, pos: ChunkPos) -> Arc<Chunk> {
        {
            let chunks = self.chunks.read().await;
            if let Some(chunk) = chunks.get(&pos) {
                return Arc::new(chunk.clone());
            }
        }

        let chunk = if let Some(data) = self.load_from_disk(pos).await {
            self.deserialize_chunk(pos, &data)
        } else {
            self.generate_chunk(pos)
        };

        let arc_chunk = Arc::new(chunk);

        {
            let mut chunks = self.chunks.write().await;
            if let Some(existing) = chunks.get(&pos) {
                return Arc::new(existing.clone());
            }
            chunks.insert(pos, (*arc_chunk).clone());
        }

        arc_chunk
    }

    async fn load_from_disk(&self, pos: ChunkPos) -> Option<Vec<u8>> {
        let path = self.world_dir.join("chunks").join(pos.filename());
        match tokio::fs::read(&path).await {
            Ok(data) => {
                debug!(?pos, path = %path.display(), "Loaded chunk from disk");
                Some(data)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!(?pos, "No chunk file on disk");
                None
            }
            Err(e) => {
                error!(?pos, error = %e, "Failed to load chunk from disk");
                None
            }
        }
    }

    fn deserialize_chunk(&self, pos: ChunkPos, data: &[u8]) -> Chunk {
        let mut chunk = Chunk::new(pos);
        let block_count = CHUNK_SIZE_X * CHUNK_SIZE_Y * CHUNK_SIZE_Z;

        if data.len() >= block_count {
            chunk.blocks.copy_from_slice(&data[..block_count]);
        } else {
            warn!(
                ?pos,
                data_len = data.len(),
                expected = block_count,
                "Chunk data truncated, regenerating"
            );
            return self.generate_chunk(pos);
        }

        chunk
    }

    fn generate_chunk(&self, pos: ChunkPos) -> Chunk {
        let data = generate_perlin_noise_chunk(
            CHUNK_SIZE_X as u8,
            CHUNK_SIZE_Y as u8,
            CHUNK_SIZE_Z as u8,
            pos.x,
            pos.z,
            &self.config,
        );

        let mut chunk = Chunk::new(pos);
        let block_count = CHUNK_SIZE_X * CHUNK_SIZE_Y * CHUNK_SIZE_Z;

        if data.len() >= block_count {
            chunk.blocks.copy_from_slice(&data[..block_count]);
        }

        debug!(?pos, "Generated new chunk");
        chunk
    }

    pub async fn set_block(
        &self,
        _pos: ChunkPos,
        x: usize,
        y: usize,
        z: usize,
        block_id: u8,
    ) -> bool {
        let chunk_x = (x as i32).div_euclid(16);
        let chunk_z = (z as i32).div_euclid(16);
        let chunk_pos = ChunkPos::new(chunk_x, chunk_z);
        let local_x = x.rem_euclid(16);
        let local_z = z.rem_euclid(16);

        let mut chunks = self.chunks.write().await;
        let chunk = match chunks.get_mut(&chunk_pos) {
            Some(c) => c,
            None => {
                warn!(?chunk_pos, "Cannot set block: chunk not loaded");
                return false;
            }
        };

        chunk.set_block(local_x, y, local_z, block_id);
        true
    }

    pub async fn save_all(&self) -> usize {
        let chunks = self.chunks.read().await;
        let modified: Vec<_> = chunks.values().filter(|c| c.modified).cloned().collect();
        drop(chunks);

        let mut saved = 0;
        for chunk in modified {
            if self.save_chunk(&chunk).await {
                let mut chunks = self.chunks.write().await;
                if let Some(c) = chunks.get_mut(&chunk.pos) {
                    c.modified = false;
                }
                saved += 1;
            }
        }

        if saved > 0 {
            info!(count = saved, "Saved modified chunks to disk");
        }
        saved
    }

    async fn save_chunk(&self, chunk: &Chunk) -> bool {
        let path = self.world_dir.join("chunks").join(chunk.pos.filename());

        if let Some(parent) = path.parent() && let Err(e) = tokio::fs::create_dir_all(parent).await {
            error!(error = %e, "Failed to create chunk directory");
            return false;
        }

        let data = chunk.to_network_data();
        let compressed = match compress_chunk_data(&data) {
            Ok(c) => c,
            Err(e) => {
                error!(?chunk.pos, error = %e, "Failed to compress chunk");
                return false;
            }
        };

        match tokio::fs::write(&path, &compressed).await {
            Ok(()) => {
                debug!(?chunk.pos, path = %path.display(), "Saved chunk to disk");
                true
            }
            Err(e) => {
                error!(?chunk.pos, error = %e, "Failed to write chunk file");
                false
            }
        }
    }

    pub async fn get_compressed_chunk_data(&self, pos: ChunkPos) -> Option<Vec<u8>> {
        let chunk = self.load_or_generate(pos).await;
        let data = chunk.to_network_data();
        compress_chunk_data(&data).ok()
    }

    pub async fn start_auto_save(self: Arc<Self>) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(SAVE_INTERVAL_SECS));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;
                let saved = manager.save_all().await;
                if saved > 0 {
                    info!(chunks_saved = saved, "Auto-save completed");
                }
            }
        });
    }

    pub async fn loaded_chunk_count(&self) -> usize {
        self.chunks.read().await.len()
    }

    pub async fn modified_chunk_count(&self) -> usize {
        self.chunks
            .read()
            .await
            .values()
            .filter(|c| c.modified)
            .count()
    }

    pub async fn is_chunk_loaded(&self, pos: ChunkPos) -> bool {
        self.chunks.read().await.contains_key(&pos)
    }

    pub fn start_prefetch(self: Arc<Self>, center_pos: Arc<tokio::sync::RwLock<ChunkPos>>) {
        let manager = self.clone();
        tokio::spawn(async move {
            let mut last_pos: Option<ChunkPos> = None;

            loop {
                let current_pos = {
                    let pos = center_pos.read().await;
                    *pos
                };

                if last_pos != Some(current_pos) {
                    let target_chunks = ChunkPos::chunks_in_radius(current_pos, VIEW_DISTANCE);
                    let loaded_chunks: HashSet<_> = {
                        let chunks = manager.chunks.read().await;
                        chunks.keys().cloned().collect()
                    };

                    let chunks_to_prefetch: Vec<_> = target_chunks
                        .into_iter()
                        .filter(|pos| !loaded_chunks.contains(pos))
                        .collect();

                    for chunk_pos in chunks_to_prefetch.into_iter().take(CHUNKS_PER_BATCH) {
                        let _ = manager.load_or_generate(chunk_pos).await;
                    }

                    last_pos = Some(current_pos);
                }

                sleep(Duration::from_millis(PREFETCH_DELAY_MS)).await;
            }
        });
    }
}

pub type SharedChunkManager = Arc<ChunkManager>;
pub type SharedChunkPos = Arc<tokio::sync::RwLock<ChunkPos>>;
