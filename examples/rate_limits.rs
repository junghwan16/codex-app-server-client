//! `cargo run --example rate_limits`

use codex_app_server_client::CodexClient;

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut client = CodexClient::spawn().await?;

    let limits = client.rate_limits().await?;

    let Some(codex) = limits.codex() else {
        println!("no rate limit info");
        return Ok(());
    };

    println!("account  {}", limits.account_id.as_deref().unwrap_or("-"));
    println!("plan     {}", codex.plan_type.as_deref().unwrap_or("-"));

    if let Some(primary) = &codex.primary {
        println!("used     {}%", primary.used_percent);
        println!(
            "window   {}",
            opt(primary.window_duration_mins.map(|m| format!("{m} min")))
        );
        println!("resets   {}", opt(primary.resets_at));
    }

    if let Some(credits) = &codex.credits {
        println!("credits  {}", credits.balance.as_deref().unwrap_or("0"));
    }

    Ok(())
}

fn opt<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string())
}
