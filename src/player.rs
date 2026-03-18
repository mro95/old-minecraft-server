use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::{io::AsyncWriteExt as _, net::tcp::OwnedWriteHalf, sync::Mutex};

use crate::{PlayerRegistry, packets::ServerPacket};

pub struct Player {
    socket: Arc<Mutex<OwnedWriteHalf>>,
    username: Option<String>,
    entity_id: Option<i32>,
    position: (f64, f64, f64),

    pub pending_keepalive: HashMap<i32, std::time::Instant>, // keep-alive ID -> timestamp when sent
    pub last_latency: Option<Duration>,
    pub next_keepalive_id: i32,
}

impl Player {
    pub fn new(socket: Arc<Mutex<OwnedWriteHalf>>) -> Self {
        Player {
            socket,
            username: None,
            entity_id: None,
            position: (0.0, 0.0, 0.0),

            pending_keepalive: HashMap::new(),
            last_latency: None,
            next_keepalive_id: 1,
        }
    }

    pub async fn send_packet(
        &mut self,
        packet: ServerPacket,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let bytes = packet.to_bytes();
        tracing::debug!(packet = ?packet, hex = hex::encode(&bytes[..bytes.len().min(50)]), len = bytes.len(), "Sending packet");
        let mut socket = self.socket.lock().await;
        socket.write_all(&bytes).await?;
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
            player_list.push((*addr, username.clone(), player.entity_id));
        }
    }

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
) -> Result<(), Box<dyn std::error::Error>> {
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

pub async fn broadcast_packet(
    players: &PlayerRegistry,
    packet: ServerPacket,
) -> Result<(), Box<dyn std::error::Error>> {
    let player_list = get_player_list(players).await;

    for (addr, _, _) in player_list {
        let players_lock = players.read().await;
        if let Some(player_arc) = players_lock.get(&addr) {
            let mut player = player_arc.write().await;
            player.send_packet(packet.clone()).await?;
        }
    }

    Ok(())
}
