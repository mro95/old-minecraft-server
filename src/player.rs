use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::{io::AsyncWriteExt as _, net::tcp::OwnedWriteHalf, sync::Mutex};

use crate::{PlayerRegistry, packets::ServerPacket, Result};

pub struct Player {
    socket: Arc<Mutex<OwnedWriteHalf>>,
    pub peer_addr: SocketAddr,

    username: Option<String>,
    pub entity_id: Option<i32>,
    pub position: (f64, f64, f64),
    pub rotation: (f32, f32), // yaw, pitch

    pub pending_keepalive: HashMap<i32, std::time::Instant>, // keep-alive ID -> timestamp when sent
    pub last_latency: Option<Duration>,
    pub next_keepalive_id: i32,
}

impl Player {
    pub fn new(socket: Arc<Mutex<OwnedWriteHalf>>, peer_addr: SocketAddr) -> Self {
        Player {
            socket,
            peer_addr,
            username: None,
            entity_id: None,
            position: (0.0, 0.0, 0.0),
            rotation: (0.0, 0.0),

            pending_keepalive: HashMap::new(),
            last_latency: None,
            next_keepalive_id: 1,
        }
    }

    pub async fn send_packet(
        &mut self,
        packet: ServerPacket,
    ) -> Result<()> {
        let bytes = packet.to_bytes();
        tracing::info!(packet = ?packet, hex = hex::encode(&bytes), len = bytes.len(), "Sending packet");
        let mut socket = self.socket.lock().await;
        socket.write_all(&bytes).await?;
        Ok(())
    }

    pub async fn broadcast_packet(
        &self,
        players: &PlayerRegistry,
        packet: ServerPacket,
        include_self: bool,
    ) -> Result<()> {
        let player_list = get_player_list(players).await;
        let bytes = packet.to_bytes();

        for (addr, _, _) in player_list {
            tracing::debug!(
                target_address = %addr,
                self_addr = %self.peer_addr,
                include_self,
                "Broadcasting packet"
            );
            if !include_self && addr == self.peer_addr {
                tracing::debug!("Skipping self in broadcast");
                continue;
            }
            let players_lock = players.read().await;
            if let Some(player_arc) = players_lock.get(&addr) {
                tracing::debug!("Sending packet to {}", addr);
                let mut player = player_arc.write().await;
                player.send_bytes(&bytes).await?;
            } else {
                tracing::warn!("Player {} not found in registry", addr);
            }
        }

        Ok(())
    }

    pub async fn send_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let mut socket = self.socket.lock().await;
        socket.write_all(bytes).await?;
        Ok(())
    }

    pub fn start_latency_measurement(&mut self) -> i32 {
        let id = self.next_keepalive_id;
        self.next_keepalive_id = self.next_keepalive_id.wrapping_add(1);

        self.pending_keepalive.insert(id, Instant::now());
        // Return the ID so caller can send it
        id
    }

    pub fn get_next_keepalive_id(&mut self) -> i32 {
        let id = self.next_keepalive_id;
        self.next_keepalive_id += 1;
        id
    }

    pub fn get_socket(&self) -> Arc<Mutex<OwnedWriteHalf>> {
        self.socket.clone()
    }

    pub fn set_username(&mut self, username: String) {
        self.username = Some(username);
    }

    pub fn get_username(&self) -> Option<String> {
        self.username.clone()
    }
}

pub async fn get_player_list(players: &PlayerRegistry) -> Vec<(SocketAddr, String, Option<i32>)> {
    let players_lock = players.read().await;

    let mut player_list = Vec::new();

    for (addr, player_arc) in players_lock.iter() {
        let player = player_arc.read().await;
        if let Some(username) = &player.username {
            tracing::trace!(addr = %addr, username = %username, entity_id = ?player.entity_id, "Found player in registry");
            player_list.push((*addr, username.clone(), player.entity_id));
        } else {
            tracing::trace!(addr = %addr, "Player has no username yet");
        }
    }

    tracing::debug!(count = player_list.len(), "get_player_list returning");
    player_list
}

pub async fn print_player_list(players: &PlayerRegistry) {
    let player_list = get_player_list(players).await;

    println!("\n=== Connected Players ({}) ===", player_list.len());
    for (addr, username, entity_id) in player_list {
        println!("  - {} ({}): Entity ID {:?}", username, addr, entity_id);
    }
    println!("========================\n");
}

pub async fn send_player_list_update(
    players: &PlayerRegistry,
) -> Result<()> {
    let player_list = get_player_list(players).await;

    for (addr, username, _) in player_list {
        let players_lock = players.read().await;
        if let Some(player_arc) = players_lock.get(&addr) {
            let mut player = player_arc.write().await;
            let latency = player.last_latency.map_or(0, |d| d.as_millis() as i16);
            player
                .send_packet(ServerPacket::PlayerListItem {
                    username: username.clone(),
                    online: true,
                    ping: latency,
                })
                .await?;
        }
    }

    Ok(())
}
