Simple Old Minecraft server implementation in Rust
=================
This is a simple Old Minecraft (1.8 beta) server implementation in Rust that can handle basic client connections and respond to some packets. It uses the Tokio async runtime for handling multiple clients concurrently.

This server needs minecraft client version 1.8 beta to connect, newer versions will not work due to protocol changes.

To run the server, make sure you have Rust installed, then navigate to the project directory and run:

```bash
cargo run
```

The server listens on port 25565 by default. You can connect to it using a Minecraft client (version 1.8 beta) by adding a new server with the address `localhost:25565`.