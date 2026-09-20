use std::collections::HashMap;

use serde::Deserialize;

trait Connection {
    /**
     * 예시:
     * self.connection
     *   .request("accoutn/rateLimits/read")
     *   .await?
     */
    async fn request(&mut self, method: &str) -> Result<String, String>;
}

struct CodexClient<C> {
    connection: C,
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
        let response = self.connection.request("account/rateLimits/read").await?;

        serde_json::from_str(&response).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeConnection {
        response: String,
    }

    impl FakeConnection {
        fn new(response: &str) -> Self {
            Self {
                response: response.to_string(),
            }
        }
    }

    impl Connection for FakeConnection {
        async fn request(&mut self, method: &str) -> Result<String, String> {
            Ok(self.response.clone())
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
}
