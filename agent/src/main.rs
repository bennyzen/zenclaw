#[cfg(feature = "desktop")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Resolve config path — try firmware/config.json if config.json doesn't exist
    let config_path = if std::path::Path::new("config.json").exists() {
        "config.json"
    } else if std::path::Path::new("firmware/config.json").exists() {
        "firmware/config.json"
    } else {
        eprintln!("Error: config.json not found. Copy from firmware/config.example.json");
        std::process::exit(1);
    };

    let config = zenclaw_agent::config::Config::load(config_path)?;
    tracing::info!("ZenClaw Agent v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Agent: {}, Provider: {}", config.agent_name, config.providers.default);

    // Resolve data directory
    let data_dir = if std::path::Path::new("data").exists() {
        "data"
    } else if std::path::Path::new("firmware/data").exists() {
        "firmware/data"
    } else {
        std::fs::create_dir_all("data/sessions")?;
        std::fs::create_dir_all("data/memory")?;
        "data"
    };

    // Initialize HTTP client
    let http: std::sync::Arc<dyn zenclaw_agent::platform::http_client::HttpClient> =
        std::sync::Arc::new(zenclaw_agent::desktop::ReqwestHttpClient::new());

    // Initialize gateway
    let mut gateway = zenclaw_agent::core::gateway::Gateway::new(config.clone(), data_dir, http);

    // Register tools
    use zenclaw_agent::core::tools::*;
    gateway.tools.register(Box::new(file_tools::FileTool));
    gateway.tools.register(Box::new(memory_tools::MemoryTool));
    gateway.tools.register(Box::new(session_tools::SessionTool));
    gateway.tools.register(Box::new(gateway_tool::GatewayTool));
    gateway.tools.register(Box::new(web_tools::WebFetchTool));
    gateway.tools.register(Box::new(web_tools::WebSearchTool));
    gateway.tools.register(Box::new(cron_tools::CronTool));
    gateway.tools.register(Box::new(message_tool::MessageTool));
    gateway.tools.register(Box::new(subagent_tools::SubagentTool));
    gateway.tools.register(Box::new(mcp_tools::McpTool));

    // Conditional tools
    if config.storage.is_some() {
        gateway.tools.register(Box::new(storage_tools::StorageTool));
    }
    if config.google.is_some() {
        gateway.tools.register(Box::new(gsheets_tools::GSheetsTool));
    }

    tracing::info!("Tools registered: {}", gateway.tools.len());

    let gateway = std::sync::Arc::new(gateway);

    // Start background runner
    let bg_cancel = tokio_util::sync::CancellationToken::new();
    let bg_gateway = gateway.clone();
    let bg_token = bg_cancel.clone();
    tokio::spawn(async move {
        let runner = zenclaw_agent::core::background::BackgroundRunner::new(bg_gateway.config.clone());
        runner.run(bg_token).await;
    });

    // Start API server
    let api_port: u16 = 8080;
    let api_gateway = gateway.clone();
    let api_handle = tokio::spawn(async move {
        zenclaw_agent::desktop::start_api_server(api_gateway, api_port).await;
    });

    // Interactive CLI loop
    tracing::info!("Ready. API on :{}, type a message or Ctrl+C to quit.", api_port);

    let stdin = tokio::io::BufReader::new(tokio::io::stdin());
    use tokio::io::AsyncBufReadExt;
    let mut lines = stdin.lines();

    loop {
        print!("> ");
        use std::io::Write;
        std::io::stdout().flush()?;

        match lines.next_line().await? {
            Some(line) => {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }
                if line == "/quit" || line == "/exit" {
                    break;
                }
                match gateway.chat("cli", &line, "cli").await {
                    Ok(response) => println!("\n{}\n", response),
                    Err(e) => eprintln!("\nError: {}\n", e),
                }
            }
            None => break, // EOF
        }
    }

    bg_cancel.cancel();
    tracing::info!("Shutting down");
    Ok(())
}

#[cfg(not(feature = "desktop"))]
fn main() {
    unimplemented!("ESP32 target not yet supported");
}
