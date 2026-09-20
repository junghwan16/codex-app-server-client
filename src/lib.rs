use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

trait Connection {
    /**
     * 예시:
     * self.connection
     *   .request("accoutn/rateLimits/read")
     *   .await?
     */
    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<String, String>;

    async fn notify(&mut self, method: &str) -> Result<(), String>;
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
}
