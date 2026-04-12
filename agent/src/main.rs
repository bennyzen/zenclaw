#[cfg(feature = "desktop")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Resolve config path — try firmware/config.json if config.json doesn't exist
    let config_path = if std::path::Path::new("config.json").exists() {
        "config.json".to_string()
    } else if std::path::Path::new("firmware/config.json").exists() {
        "firmware/config.json".to_string()
    } else {
        eprintln!("Error: config.json not found. Copy from firmware/config.example.json");
        std::process::exit(1);
    };

    let config = zenclaw_agent::config::Config::load(&config_path)?;
    tracing::info!("ZenClaw Agent v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!(
        "Agent: {}, Provider: {}",
        config.agent_name,
        config.providers.default
    );

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

    // Initialize gateway
    let mut gateway = zenclaw_agent::core::gateway::Gateway::new(config.clone(), data_dir);

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
    let start_time = std::time::Instant::now();

    // Start background runner
    let bg_cancel = tokio_util::sync::CancellationToken::new();
    let bg_gateway = gateway.clone();
    let bg_token = bg_cancel.clone();
    tokio::spawn(async move {
        let runner = zenclaw_agent::core::background::BackgroundRunner::new(
            bg_gateway.config.clone(),
            bg_gateway.data_dir.clone(),
        );
        runner.run(bg_token).await;
    });

    // Start Telegram poller if configured
    if let Some(ref tg) = config.channels.telegram {
        if tg.enabled && !tg.bot_token.is_empty() {
            let tg_gateway = gateway.clone();
            let bot_token = tg.bot_token.clone();
            let allowed = tg.allowed_chat_ids.clone();

            tokio::spawn(async move {
                let (tx, mut rx) = tokio::sync::mpsc::channel::<
                    zenclaw_agent::core::telegram::IncomingMessage,
                >(32);

                // Spawn poller task
                let poller_token = bot_token.clone();
                tokio::spawn(async move {
                    let mut poller =
                        zenclaw_agent::core::telegram::TelegramPoller::new(poller_token);
                    if let Err(e) = poller.poll_loop(tx).await {
                        tracing::error!(error = %e, "Telegram poller stopped");
                    }
                });

                // Create delivery channel (reuse one reqwest client)
                let deliver_client = reqwest::Client::new();
                let deliver_token = bot_token.clone();

                // Process incoming messages
                while let Some(msg) = rx.recv().await {
                    // Check allowed chat IDs
                    if let Some(ref ids) = allowed {
                        if !ids.contains(&msg.chat_id) {
                            tracing::warn!(
                                chat_id = %msg.chat_id,
                                "Telegram message from disallowed chat"
                            );
                            continue;
                        }
                    }

                    let gw = tg_gateway.clone();
                    let client = deliver_client.clone();
                    let token = deliver_token.clone();
                    let chat_id = msg.chat_id.clone();

                    tokio::spawn(async move {
                        // Send typing indicator
                        let _ = client
                            .post(format!(
                                "https://api.telegram.org/bot{}/sendChatAction",
                                token
                            ))
                            .json(&serde_json::json!({
                                "chat_id": &chat_id,
                                "action": "typing"
                            }))
                            .send()
                            .await;

                        // Run chat
                        match gw.chat(&chat_id, &msg.text, "telegram").await {
                            Ok(reply) => {
                                if let Err(e) = client
                                    .post(format!(
                                        "https://api.telegram.org/bot{}/sendMessage",
                                        token
                                    ))
                                    .json(&serde_json::json!({
                                        "chat_id": &chat_id,
                                        "text": &reply
                                    }))
                                    .send()
                                    .await
                                {
                                    tracing::error!(
                                        error = %e,
                                        chat_id = %chat_id,
                                        "Telegram sendMessage failed"
                                    );
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    error = %e,
                                    chat_id = %chat_id,
                                    "Telegram chat error"
                                );
                            }
                        }
                    });
                }
            });
            tracing::info!("Telegram poller started");
        }
    }

    // Build app state for API server
    let app_state = zenclaw_agent::desktop::AppState {
        gateway: gateway.clone(),
        start_time,
        config_path,
    };

    // Start API server
    let api_port: u16 = 8080;
    tokio::spawn(async move {
        zenclaw_agent::desktop::start_api_server(app_state, api_port).await;
    });

    // Interactive CLI loop (only if stdin is a tty, otherwise run as daemon)
    tracing::info!(
        "Ready. API on :{}, type a message or Ctrl+C to quit.",
        api_port
    );

    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
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
                None => break,
            }
        }
    } else {
        // Daemon mode — wait for Ctrl+C
        tracing::info!("Running in daemon mode (no tty). Press Ctrl+C to stop.");
        tokio::signal::ctrl_c().await?;
    }

    bg_cancel.cancel();
    tracing::info!("Shutting down");
    Ok(())
}

#[cfg(not(feature = "desktop"))]
fn main() {
    unimplemented!("ESP32 target not yet supported");
}
