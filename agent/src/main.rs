#[cfg(feature = "desktop")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config = zenclaw_agent::config::Config::load("config.json")?;
    tracing::info!("ZenClaw Agent v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Agent: {}", config.agent_name);

    // TODO: Initialize gateway, start services

    Ok(())
}

#[cfg(not(feature = "desktop"))]
fn main() {
    // ESP32 entry point — to be implemented with embassy or similar
    unimplemented!("ESP32 target not yet supported");
}
