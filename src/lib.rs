use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

trait Connection {
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<String, String>;
    async fn notify(&mut self, method: &str) -> Result<(), String>;
}

struct ProcessConnection<W, R> {
    writer: W,
    reader: R,
    next_id: u64,
}

impl<W, R> ProcessConnection<W, R> {
    fn new(writer: W, reader: R) -> Self {
        Self {
            writer,
            reader,
            next_id: 1,
        }
    }
}

impl<W, R> Connection for ProcessConnection<W, R>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<String, String> {
        let id = self.next_id;
        self.next_id += 1;

        let message = build_request(1, method, params);

        let mut bytes = serde_json::to_vec(&message).map_err(|err| err.to_string())?;
        bytes.push(b'\n');

        self.writer
            .write_all(&bytes)
            .await
            .map_err(|err| err.to_string())?;

        self.writer.flush().await.map_err(|err| err.to_string())?;

        // TODO: 여기서 무한 루프 돌아서 문제 생기는 케이스는 없을까? 타임아웃을 주는건?
        loop {
            let mut response = String::new();

            self.reader
                .read_line(&mut response)
                .await
                .map_err(|err| err.to_string())?;

            let message: Value =
                serde_json::from_str(response.trim_end()).map_err(|err| err.to_string())?;

            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(response.trim_end().to_string());
            }
        }
    }

    async fn notify(&mut self, method: &str) -> Result<(), String> {
        let message = serde_json::json!({
            "method": method
        });

        let mut bytes = serde_json::to_vec(&message).map_err(|err| err.to_string())?;

        bytes.push(b'\n');

        self.writer
            .write_all(&bytes)
            .await
            .map_err(|err| err.to_string())?;

        self.writer.flush().await.map_err(|err| err.to_string())?;

        Ok(())
    }
}

struct CodexClient<C> {
    connection: C,
}

impl<C: Connection> CodexClient<C> {
    async fn connect(mut connection: C) -> Result<Self, String> {
        connection
            .request(
                "initialize",
                Some(serde_json::json!({
                    "clientInfo": {
                        "name": "codex-app-server-client",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                })),
            )
            .await?;
        connection.notify("initialized").await?;

        Ok(Self { connection })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimitResponse {
    rate_limits_by_limit_id: Option<HashMap<String, LimitGroup>>,
}

impl RateLimitResponse {
    fn codex(&self) -> Option<&LimitGroup> {
        self.rate_limits_by_limit_id.as_ref()?.get("codex")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitGroup {
    limit_id: Option<String>,
    primary: Option<RateLimit>,
    secondary: Option<RateLimit>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateLimit {
    used_percent: u8,
    window_duration_mins: u32,
    resets_at: u64,
}

impl<C: Connection> CodexClient<C> {
    fn new(connection: C) -> Self {
        Self { connection }
    }

    async fn rate_limits(&mut self) -> Result<RateLimitResponse, String> {
        let response = self
            .connection
            .request("account/rateLimits/read", None)
            .await?;

        serde_json::from_str(&response).map_err(|err| err.to_string())
    }
}

fn build_request(id: u64, method: &str, params: Option<Value>) -> Value {
    let mut message = serde_json::json!({
        "id": id,
        "method": method,
    });

    if let Some(params) = params {
        message["params"] = params;
    }

    message
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeConnection {
        response: String,
        requests: Vec<String>,
        request_params: Vec<Option<Value>>,
        notifications: Vec<String>,
    }

    impl FakeConnection {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
                requests: vec![],
                request_params: vec![],
                notifications: vec![],
            }
        }
    }

    impl Connection for FakeConnection {
        async fn request(&mut self, method: &str, params: Option<Value>) -> Result<String, String> {
            self.requests.push(method.to_string());
            self.request_params.push(params);

            Ok(self.response.clone())
        }

        async fn notify(&mut self, method: &str) -> Result<(), String> {
            self.notifications.push(method.to_string());

            Ok(())
        }
    }

    #[tokio::test]
    async fn reads_rate_limits_through_connection() {
        let connection = FakeConnection::new(
            r#"{
                "rateLimitsByLimitId": {
                    "codex": {
                        "primary": {
                            "usedPercent": 25,
                            "windowDurationMins": 300,
                            "resetsAt": 123
                        },
                        "secondary": null
                    }
                }
            }"#,
        );

        let mut client = CodexClient::new(connection);

        let limits = client.rate_limits().await.unwrap();

        assert_eq!(
            limits
                .codex()
                .unwrap()
                .primary
                .as_ref()
                .unwrap()
                .used_percent,
            25
        );
    }

    #[tokio::test]
    async fn connect_sends_initialize_request() {
        let connection = FakeConnection::new("{}");

        let client = CodexClient::connect(connection).await.unwrap();

        // initialize가 요청되었음을 검증
        assert_eq!(client.connection.requests, vec!["initialize"]);
        assert_eq!(client.connection.notifications, vec!["initialized"]);

        assert_eq!(
            client.connection.request_params[0],
            Some(serde_json::json!({
                "clientInfo": {
                    "name": "codex-app-server-client",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }))
        );
    }

    #[test]
    fn builds_request_message() {
        let params = serde_json::json!({
             "clientInfo": {
                "name": "codex-app-server-client",
                "version": "0.1.0"
            }
        });

        let message = build_request(1, "initialize", Some(params));

        assert_eq!(
            message,
            serde_json::json!({
                "id": 1,
                "method": "initialize",
                "params": {
                    "clientInfo": {
                        "name": "codex-app-server-client",
                        "version": "0.1.0"
                    }
                }
            })
        );
    }

    #[test]
    fn omits_params_when_none() {
        let message = build_request(2, "account/rateLimits/read", None);

        assert_eq!(
            message,
            serde_json::json!({
                "id": 2,
                "method": "account/rateLimits/read"
            })
        );
    }

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[tokio::test]
    async fn process_connection_sends_request_and_reads_response() {
        // 가짜 서버 <> 클라이언트 구조를 Fake하기 위해 사용한다.
        let (client_io, server_io) = tokio::io::duplex(1024);

        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);

        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let mut line = String::new();

            reader.read_line(&mut line).await.unwrap();

            let request: Value = serde_json::from_str(line.trim()).unwrap();

            assert_eq!(
                request,
                serde_json::json!({
                    "id": 1,
                    "method": "initialize"
                })
            );

            server_writer
                .write_all(b"{\"id\":1,\"result\":{}}\n")
                .await
                .unwrap();
        });

        let mut connection = ProcessConnection::new(client_writer, BufReader::new(client_reader));

        let response = connection.request("initialize", None).await.unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();

        assert_eq!(
            response,
            serde_json::json!({
                "id": 1,
                "result": {}
            })
        );

        server.await.unwrap();
    }

    // 사실 이것 때문에 이 프로젝트를 시작했다.
    #[tokio::test]
    async fn request_skips_notification_before_response() {
        let (client_io, server_io) = tokio::io::duplex(1024);

        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, mut server_writer) = tokio::io::split(server_io);

        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_reader);
            let mut line = String::new();

            reader.read_line(&mut line).await.unwrap();

            server_writer
                .write_all(b"{\"method\":\"remoteControl/status/changed\",\"params\":{}}\n")
                .await
                .unwrap();

            server_writer
                .write_all(b"{\"id\":1,\"result\":{\"ok\":true}}\n")
                .await
                .unwrap();
        });

        let mut connection = ProcessConnection::new(client_writer, BufReader::new(client_reader));

        let response = connection.request("initialize", None).await.unwrap();

        let response: Value = serde_json::from_str(&response).unwrap();

        assert_eq!(
            response,
            serde_json::json!({
                "id": 1,
                "result": {
                    "ok": true
                }
            })
        );

        server.await.unwrap();
    }
}
