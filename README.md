# codex-app-server-client

Rust client for the Codex app-server protocol.

> 상태: `spawn` / `rate_limits`까지 구현됨. 예제는 `cargo run --example rate_limits`.

## 목표 API

```rust
// initialize => initialized
let mut client = CodexClient::spawn().await?;

let limits = client.rate_limits().await?;
```

## 설계

- `Connection` — 전송 계층 추상화. 클라이언트는 이 trait에만 의존하므로
  테스트에서는 fake로, 실제 환경에서는 app-server 프로세스로 바꿔 끼울 수 있습니다.
- `CodexClient<C: Connection>` — 프로토콜 호출을 노출하는 얇은 래퍼.

## 개발

```sh
cargo test
```
