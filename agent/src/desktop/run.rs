use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::core::gateway::Gateway;
use crate::core::runner::{LlmRunner, Runner};
use crate::desktop::background::BackgroundRunner;
use crate::desktop::telegram::{IncomingMessage, TelegramPoller};

use super::{start_api_server, AppState};

/// Desktop entry point.
///
/// Mirrors the embedded firmware's lifecycle on a host machine: loads
/// `config.json` from the current directory, constructs the same Gateway
/// the ESP32 firmware does, exposes the same HTTP API on `0.0.0.0:8080`,
/// optionally spawns the Telegram poller, and offers a stdin REPL when
/// stdout is a TTY (otherwise runs as a daemon until SIGINT).
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config_path = "config.json".to_string();
    if !std::path::Path::new(&config_path).exists() {
        eprintln!(
            "Error: {} not found in current directory.\n\
             Create one with at least: agent_name, providers.default, and a provider entry with api_key + model.",
            config_path
        );
        std::process::exit(1);
    }
    let config = Config::load(&config_path)?;

    info!("ZenClaw Agent v{}", env!("CARGO_PKG_VERSION"));
    info!(
        "Agent: {}, Provider: {}",
        config.agent_name, config.providers.default
    );

    let data_dir = "data";
    std::fs::create_dir_all(format!("{}/sessions", data_dir))?;
    std::fs::create_dir_all(format!("{}/memory", data_dir))?;
    crate::core::workspace::seed_defaults(data_dir);

    // Gateway: tools auto-registered via register_defaults() inside Gateway::new.
    // Same set as ESP32 — preserves parity so optimization signals transfer.
    let config_arc = Arc::new(config.clone());
    let runner: Box<dyn LlmRunner> = Box::new(Runner::new(config_arc));
    let gateway = Gateway::new(config.clone(), data_dir, runner);
    info!("Tools registered: {}", gateway.tools.len());

    let gateway = Arc::new(gateway);
    let start_time = Instant::now();

    let bg_cancel = CancellationToken::new();
    {
        let bg_gateway = gateway.clone();
        let bg_token = bg_cancel.clone();
        tokio::spawn(async move {
            let runner = BackgroundRunner::new(
                bg_gateway.config.clone(),
                bg_gateway.data_dir.clone(),
            );
            runner.run(bg_token).await;
        });
    }

    if let Some(ref tg) = config.channels.telegram {
        if tg.enabled && !tg.bot_token.is_empty() {
            spawn_telegram_loop(
                gateway.clone(),
                tg.bot_token.clone(),
                tg.allowed_chat_ids.clone(),
            );
            info!("Telegram poller started");
        }
    }

    let api_port: u16 = 8080;
    {
        let app_state = AppState {
            gateway: gateway.clone(),
            start_time,
            config_path: config_path.clone(),
        };
        tokio::spawn(async move {
            start_api_server(app_state, api_port).await;
        });
    }

    info!(
        "Ready. API on :{} — type a message, /quit to exit.",
        api_port
    );

    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        repl_loop(&gateway).await?;
    } else {
        info!("No TTY detected — running in daemon mode. SIGINT to stop.");
        tokio::signal::ctrl_c().await?;
    }

    bg_cancel.cancel();
    info!("Shutting down");
    Ok(())
}

fn spawn_telegram_loop(
    gateway: Arc<Gateway>,
    bot_token: String,
    allowed: Option<Vec<String>>,
) {
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<IncomingMessage>(32);

        let poller_token = bot_token.clone();
        tokio::spawn(async move {
            let mut poller = TelegramPoller::new(poller_token);
            if let Err(e) = poller.poll_loop(tx).await {
                error!(error = %e, "Telegram poller stopped");
            }
        });

        let deliver_client = reqwest::Client::new();
        while let Some(msg) = rx.recv().await {
            if let Some(ref ids) = allowed {
                if !ids.contains(&msg.chat_id) {
                    warn!(chat_id = %msg.chat_id, "Telegram message from disallowed chat");
                    continue;
                }
            }

            let gw = gateway.clone();
            let client = deliver_client.clone();
            let token = bot_token.clone();
            let chat_id = msg.chat_id.clone();

            tokio::spawn(async move {
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
                            error!(error = %e, chat_id = %chat_id, "Telegram sendMessage failed");
                        }
                    }
                    Err(e) => error!(error = %e, chat_id = %chat_id, "Telegram chat error"),
                }
            });
        }
    });
}

async fn repl_loop(gateway: &Arc<Gateway>) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    use tokio::io::{AsyncBufReadExt, BufReader};

    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();

    loop {
        print!("> ");
        std::io::stdout().flush()?;

        match lines.next_line().await? {
            Some(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line == "/quit" || line == "/exit" {
                    break;
                }
                match gateway.chat("cli", line, "cli").await {
                    Ok(response) => println!("\n{}\n", response),
                    Err(e) => eprintln!("\nError: {}\n", e),
                }
            }
            None => break,
        }
    }
    Ok(())
}
