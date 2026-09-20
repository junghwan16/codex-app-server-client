# codex-app-server-client

Rust client for the Codex app-server protocol.

> Status: `spawn` / `rate_limits` are implemented. Try `cargo run --example rate_limits`.

## Usage

```rust
let mut client = CodexClient::spawn().await?;

let limits = client.rate_limits().await?;
```

`CodexClient::spawn()` launches `codex app-server` as a child process, performs
the handshake, and leaves the client ready for protocol calls.

## Design

- `Connection` — transport abstraction. The client depends only on this trait,
  so it can be backed by a fake in tests and by the app-server process in
  production. `request` strips the JSON-RPC envelope and returns just `result`,
  skipping any notifications that arrive before the matching response.
- `CodexClient<C: Connection>` — a thin wrapper exposing the protocol calls.

## Development

```sh
cargo test
```
