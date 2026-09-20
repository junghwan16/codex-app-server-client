use std::collections::HashMap;
use std::process::Stdio;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout, Command};

#[allow(async_fn_in_trait)]
pub trait Connection {
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String>;
    async fn notify(&mut self, method: &str) -> Result<(), String>;
}

pub struct ProcessConnection<W, R> {
    writer: W,
    reader: R,
    next_id: u64,
}

/// `codex app-server` 자식 프로세스
pub type ChildConnection = ProcessConnection<ChildStdin, BufReader<ChildStdout>>;

impl<W, R> ProcessConnection<W, R> {
    fn new(writer: W, reader: R) -> Self {
        Self {
            writer,
            reader,
            next_id: 1,
        }
    }
}

impl ChildConnection {
    /// 주어진 커맨드를 stdin/stdout 파이프로 띄우고 그 위에 연결을 만든다.
    pub async fn spawn(mut command: Command) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|err| err.to_string())?;

        let stdin = child.stdin.take().ok_or("failed to open codex stdin")?;
        let stdout = child.stdout.take().ok_or("failed to open codex stdout")?;

        Ok(Self::new(stdin, BufReader::new(stdout)))
    }
}

impl<W, R> ProcessConnection<W, R>
where
    W: AsyncWrite + Unpin,
{
    async fn send(&mut self, message: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(message).map_err(|err| err.to_string())?;
        bytes.push(b'\n');

        self.writer
            .write_all(&bytes)
            .await
            .map_err(|err| err.to_string())?;

        self.writer.flush().await.map_err(|err| err.to_string())
    }
}

impl<W, R> Connection for ProcessConnection<W, R>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;

        self.send(&build_request(id, method, params)).await?;

        // 응답이 오기 전에 서버가 보내는 notification은 건너뛴다.
        // TODO: 여기서 무한 루프 돌아서 문제 생기는 케이스는 없을까? 타임아웃을 주는건?
        let mut line = String::new();
        loop {
            line.clear();

            let bytes_read = self
                .reader
                .read_line(&mut line)
                .await
                .map_err(|err| err.to_string())?;
            if bytes_read == 0 {
                return Err("connection closed".to_string());
            }

            let mut message: Value =
                serde_json::from_str(line.trim_end()).map_err(|err| err.to_string())?;

            if message.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(message
                    .get_mut("result")
                    .map(Value::take)
                    .unwrap_or(Value::Null));
            }
        }
    }

    async fn notify(&mut self, method: &str) -> Result<(), String> {
        self.send(&serde_json::json!({ "method": method })).await
    }
}

pub struct CodexClient<C> {
    connection: C,
}

impl<C: Connection> CodexClient<C> {
    fn new(connection: C) -> Self {
        Self { connection }
    }

    /// `initialize` 요청과 `initialized` 요청으로 핸드셰이크
    pub async fn connect(mut connection: C) -> Result<Self, String> {
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

        Ok(Self::new(connection))
    }

    pub async fn rate_limits(&mut self) -> Result<RateLimitResponse, String> {
        let result = self
            .connection
            .request("account/rateLimits/read", None)
            .await?;

        serde_json::from_value(result).map_err(|err| err.to_string())
    }
}

impl CodexClient<ChildConnection> {
    /// `codex app-server`를 띄우고 연결한다.
    pub async fn spawn() -> Result<Self, String> {
        let mut command = Command::new("codex");
        command.arg("app-server");

        Self::connect(ChildConnection::spawn(command).await?).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitResponse {
    pub account_id: Option<String>,
    rate_limits_by_limit_id: Option<HashMap<String, LimitGroup>>,
}

impl RateLimitResponse {
    pub fn codex(&self) -> Option<&LimitGroup> {
        self.rate_limits_by_limit_id.as_ref()?.get("codex")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitGroup {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub plan_type: Option<String>,
    pub primary: Option<RateLimit>,
    pub secondary: Option<RateLimit>,
    pub credits: Option<Credits>,
    #[serde(default)]
    pub spend_control_reached: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    #[serde(default)]
    pub has_credits: bool,
    #[serde(default)]
    pub unlimited: bool,
    pub balance: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimit {
    /// 서버가 소수점 값을 보내므로 f64로 받는다.
    pub used_percent: f64,
    pub window_duration_mins: u32,
    /// 창이 초기화되는 시각 (unix epoch seconds).
    pub resets_at: u64,
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

    use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

    struct FakeConnection {
        result: Value,
        requests: Vec<String>,
        request_params: Vec<Option<Value>>,
        notifications: Vec<String>,
    }

    impl FakeConnection {
        fn new(result: Value) -> Self {
            Self {
                result,
                requests: vec![],
                request_params: vec![],
                notifications: vec![],
            }
        }
    }

    impl Connection for FakeConnection {
        async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, String> {
            self.requests.push(method.to_string());
            self.request_params.push(params);

            Ok(self.result.clone())
        }

        async fn notify(&mut self, method: &str) -> Result<(), String> {
            self.notifications.push(method.to_string());

            Ok(())
        }
    }

    type TestConnection =
        ProcessConnection<WriteHalf<DuplexStream>, BufReader<ReadHalf<DuplexStream>>>;
    type ServerReader = BufReader<ReadHalf<DuplexStream>>;
    type ServerWriter = WriteHalf<DuplexStream>;

    /// 가짜 서버 <> 클라이언트 구조를 Fake하기 위해 사용한다.
    fn connected_pair() -> (TestConnection, ServerReader, ServerWriter) {
        let (client_io, server_io) = tokio::io::duplex(1024);

        let (client_reader, client_writer) = tokio::io::split(client_io);
        let (server_reader, server_writer) = tokio::io::split(server_io);

        (
            ProcessConnection::new(client_writer, BufReader::new(client_reader)),
            BufReader::new(server_reader),
            server_writer,
        )
    }

    /// 한 줄을 읽어 JSON으로 파싱한다.
    async fn read_json(reader: &mut ServerReader) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();

        serde_json::from_str(line.trim()).unwrap()
    }

    #[tokio::test]
    async fn reads_rate_limits_through_connection() {
        let connection = FakeConnection::new(serde_json::json!({
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
        }));

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
            25.0
        );
    }

    #[tokio::test]
    async fn connect_sends_initialize_request() {
        let connection = FakeConnection::new(serde_json::json!({}));

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

    #[tokio::test]
    async fn process_connection_sends_request_and_reads_response() {
        let (mut connection, mut server_reader, mut server_writer) = connected_pair();

        let server = tokio::spawn(async move {
            assert_eq!(
                read_json(&mut server_reader).await,
                serde_json::json!({
                    "id": 1,
                    "method": "initialize"
                })
            );

            server_writer
                .write_all(b"{\"id\":1,\"result\":{\"ok\":true}}\n")
                .await
                .unwrap();
        });

        // 봉투(id/result)는 벗겨지고 result만 돌아온다.
        let response = connection.request("initialize", None).await.unwrap();

        assert_eq!(response, serde_json::json!({ "ok": true }));

        server.await.unwrap();
    }

    // 사실 이것 때문에 이 프로젝트를 시작했다.
    #[tokio::test]
    async fn request_skips_notification_before_response() {
        let (mut connection, mut server_reader, mut server_writer) = connected_pair();

        let server = tokio::spawn(async move {
            read_json(&mut server_reader).await;

            server_writer
                .write_all(b"{\"method\":\"remoteControl/status/changed\",\"params\":{}}\n")
                .await
                .unwrap();

            server_writer
                .write_all(b"{\"id\":1,\"result\":{\"ok\":true}}\n")
                .await
                .unwrap();
        });

        let response = connection.request("initialize", None).await.unwrap();

        assert_eq!(response, serde_json::json!({ "ok": true }));

        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_returns_error_when_connection_closes_before_response() {
        let (mut connection, mut server_reader, mut server_writer) = connected_pair();

        let server = tokio::spawn(async move {
            read_json(&mut server_reader).await;

            // 응답 없이 서버의 write side를 명시적으로 닫는다.
            server_writer.shutdown().await.unwrap();
        });

        let result = connection.request("initialize", None).await;

        assert_eq!(result, Err("connection closed".to_string()));

        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_increments_request_id() {
        let (mut connection, mut server_reader, mut server_writer) = connected_pair();

        let server = tokio::spawn(async move {
            assert_eq!(read_json(&mut server_reader).await["id"], 1);

            server_writer
                .write_all(b"{\"id\":1,\"result\":{}}\n")
                .await
                .unwrap();

            assert_eq!(read_json(&mut server_reader).await["id"], 2);

            server_writer
                .write_all(b"{\"id\":2,\"result\":{}}\n")
                .await
                .unwrap();
        });

        connection.request("first", None).await.unwrap();
        connection.request("second", None).await.unwrap();

        server.await.unwrap();
    }
}
