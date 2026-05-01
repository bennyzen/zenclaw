use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::Config;
use crate::core::channels::telegram::{IncomingMessage, Poller, TelegramChannel};
use crate::core::channels::Channel;
use crate::core::gateway::Gateway;
use crate::core::runner::{LlmRunner, Runner};
use crate::desktop::background::BackgroundRunner;
use crate::desktop::http_client::ReqwestHttpClient;
use crate::platform::http_client::HttpClient;

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

    let config_arc = Arc::new(config.clone());
    let runner: Box<dyn LlmRunner> = Box::new(Runner::new(config_arc));
    let gateway = Gateway::new(config.clone(), data_dir, runner);
    info!("Tools registered: {}", gateway.tools.len());

    let gateway = Arc::new(gateway);
    let http: Arc<dyn HttpClient> = Arc::new(ReqwestHttpClient::new());
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
                http.clone(),
                tg.bot_token.clone(),
                tg.allowed_chat_ids.clone(),
            );
            info!("Telegram poller started");
        }
    }

    let api_port: u16 = std::env::var("ZENCLAW_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);
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
    http: Arc<dyn HttpClient>,
    bot_token: String,
    allowed: Option<Vec<String>>,
) {
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<IncomingMessage>(32);

        // Producer: poll_once in a loop, forward messages to the consumer.
        let producer_http = http.clone();
        let producer_token = bot_token.clone();
        tokio::spawn(async move {
            let mut poller = Poller::new(producer_token);
            loop {
                match poller.poll_once(&*producer_http, 10).await {
                    Ok(msgs) => {
                        for msg in msgs {
                            if tx.send(msg).await.is_err() {
                                tracing::info!("Poller channel closed, stopping");
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "Telegram poll error, retrying in 5s");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        // Consumer: spawn a task per inbound message so the next poll can
        // start while a turn is still running through the gateway.
        while let Some(msg) = rx.recv().await {
            if let Some(ref ids) = allowed {
                if !ids.contains(&msg.chat_id) {
                    warn!(chat_id = %msg.chat_id, "Telegram message from disallowed chat");
                    continue;
                }
            }

            let gw = gateway.clone();
            let http_for_task = http.clone();
            let token_for_task = bot_token.clone();
            let chat_id = msg.chat_id.clone();
            let text = msg.text.clone();

            tokio::spawn(async move {
                let channel = TelegramChannel::new(token_for_task, http_for_task);

                if let Err(e) = channel.send_typing(&chat_id).await {
                    warn!(error = %e, chat_id = %chat_id, "send_typing failed");
                }

                let reply = match gw.chat(&chat_id, &text, "telegram").await {
                    Ok(r) => r,
                    Err(e) => {
                        error!(error = %e, chat_id = %chat_id, "Telegram chat error");
                        format!("Error: {}", e)
                    }
                };

                if let Err(e) = channel.deliver(&chat_id, &reply).await {
                    error!(error = %e, chat_id = %chat_id, "Telegram deliver failed");
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
