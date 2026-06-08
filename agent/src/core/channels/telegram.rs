//! Telegram bot integration — `Poller` (long-poll receiver) and
//! `TelegramChannel` (sender, impl `Channel`). Both go through
//! `&dyn HttpClient` so they work identically on ESP32 and desktop.
//!
//! Defaults:
//! - Long-poll timeout: caller-supplied; recommended 10s.
//! - Formatting: `deliver` renders LLM markdown to Telegram HTML via
//!   `markdown_html::render_telegram` (bold/italic/code/lists/quote/links +
//!   monospace `<pre>` tables), chunks output to Telegram's 4096-char limit,
//!   and sends each chunk with `parse_mode=HTML`. On a Telegram 400 (malformed
//!   entities) the chunk is re-sent as stripped plain text, so a formatting
//!   defect can never lose a message.
//! - allowed_chat_ids: not enforced inside Poller — caller filters
//!   returned `IncomingMessage`s, since Poller doesn't see config.

use async_trait::async_trait;
use std::sync::Arc;

use crate::core::channels::Channel;
use crate::platform::http_client::{Headers, HttpClient};

#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub chat_id: String,
    pub text: String,
    pub from_username: Option<String>,
}

pub struct Poller {
    bot_token: String,
    offset: i64,
}

impl Poller {
    pub fn new(bot_token: String) -> Self {
        Self {
            bot_token,
            offset: 0,
        }
    }

    /// One getUpdates round-trip with `timeout_secs` long-poll.
    /// Advances internal offset; returns whatever arrived (possibly empty).
    /// Caller drives cadence (interleaved on ESP32, tokio loop on desktop).
    pub async fn poll_once(
        &mut self,
        http: &dyn HttpClient,
        timeout_secs: u32,
    ) -> Result<Vec<IncomingMessage>, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/getUpdates?offset={}&timeout={}",
            self.bot_token, self.offset, timeout_secs
        );

        let resp = http.get(&url, &Headers::new()).await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!(
                "Telegram getUpdates HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            )
            .into());
        }

        let body: serde_json::Value = serde_json::from_slice(&resp.body)
            .map_err(|e| format!("Telegram parse: {}", e))?;

        let updates = match body.get("result").and_then(|r| r.as_array()) {
            Some(arr) => arr,
            None => return Ok(Vec::new()),
        };

        let mut messages = Vec::new();
        for update in updates {
            if let Some(uid) = update.get("update_id").and_then(|v| v.as_i64()) {
                if uid >= self.offset {
                    self.offset = uid + 1;
                }
            }

            let msg = match update.get("message") {
                Some(m) => m,
                None => continue,
            };

            let chat_id = msg
                .get("chat")
                .and_then(|c| c.get("id"))
                .and_then(|id| id.as_i64())
                .map(|id| id.to_string());

            let text = msg.get("text").and_then(|t| t.as_str()).map(String::from);

            let from_username = msg
                .get("from")
                .and_then(|f| f.get("username"))
                .and_then(|u| u.as_str())
                .map(String::from);

            if let (Some(chat_id), Some(text)) = (chat_id, text) {
                messages.push(IncomingMessage {
                    chat_id,
                    text,
                    from_username,
                });
            }
        }

        Ok(messages)
    }

    /// Register the bot's command list with Telegram.
    ///
    /// Idempotent — calling on every boot with the same payload is fine.
    /// Failures are non-fatal: caller should log and continue.
    pub async fn set_my_commands(
        &self,
        http: &dyn HttpClient,
        commands: &[(&str, &str)],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/setMyCommands",
            self.bot_token,
        );
        let payload = serde_json::json!({
            "commands": commands
                .iter()
                .map(|(name, desc)| serde_json::json!({
                    "command": name,
                    "description": desc,
                }))
                .collect::<Vec<_>>(),
        });
        let body = serde_json::to_vec(&payload)?;

        let mut headers = Headers::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let resp = http.post(&url, &headers, &body).await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!(
                "Telegram setMyCommands HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            )
            .into());
        }
        Ok(())
    }
}

pub struct TelegramChannel {
    bot_token: String,
    http: Arc<dyn HttpClient>,
    parse_mode: Option<String>,
}

impl TelegramChannel {
    pub fn new(bot_token: String, http: Arc<dyn HttpClient>) -> Self {
        Self {
            bot_token,
            http,
            parse_mode: None,
        }
    }

    pub fn with_parse_mode(mut self, mode: Option<String>) -> Self {
        self.parse_mode = mode;
        self
    }

    /// Telegram-specific (not on Channel trait — Cli has no notion of typing).
    pub async fn send_typing(
        &self,
        chat_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendChatAction",
            self.bot_token
        );
        let body = serde_json::json!({
            "chat_id": chat_id,
            "action": "typing",
        })
        .to_string();
        let mut headers = Headers::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let resp = self.http.post(&url, &headers, body.as_bytes()).await?;
        if resp.status < 200 || resp.status >= 300 {
            return Err(format!(
                "Telegram sendChatAction HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            )
            .into());
        }
        Ok(())
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    async fn deliver(
        &self,
        chat_id: &str,
        text: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for chunk in crate::core::channels::markdown_html::render_telegram(text) {
            self.send_one(chat_id, &chunk).await?;
        }
        Ok(())
    }
}

impl TelegramChannel {
    /// Send one already-rendered HTML chunk. Tries `parse_mode=HTML`; on a
    /// Telegram 400 (malformed entities) retries the same chunk as stripped
    /// plain text so a formatting defect can never lose a message.
    async fn send_one(
        &self,
        chat_id: &str,
        html: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let status = self.post_send(chat_id, html, Some("HTML")).await?;
        if (200..300).contains(&status) {
            return Ok(());
        }
        if status == 400 {
            let plain = strip_tags(html);
            let retry = self.post_send(chat_id, &plain, None).await?;
            if (200..300).contains(&retry) {
                return Ok(());
            }
            return Err(format!(
                "Telegram sendMessage HTTP {} (after plain-text fallback)",
                retry
            )
            .into());
        }
        Err(format!("Telegram sendMessage HTTP {}", status).into())
    }

    /// POST one sendMessage. Returns the HTTP status (transport errors are
    /// surfaced as `Err`).
    async fn post_send(
        &self,
        chat_id: &str,
        text: &str,
        parse_mode: Option<&str>,
    ) -> Result<u16, Box<dyn std::error::Error + Send + Sync>> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token
        );
        let mut payload = serde_json::Map::new();
        payload.insert(
            "chat_id".to_string(),
            serde_json::Value::String(chat_id.to_string()),
        );
        payload.insert(
            "text".to_string(),
            serde_json::Value::String(text.to_string()),
        );
        if let Some(mode) = parse_mode {
            payload.insert(
                "parse_mode".to_string(),
                serde_json::Value::String(mode.to_string()),
            );
        }
        let body = serde_json::Value::Object(payload).to_string();

        let mut headers = Headers::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let resp = self.http.post(&url, &headers, body.as_bytes()).await?;
        Ok(resp.status)
    }
}

/// Remove HTML tags and unescape entities for the plain-text fallback path.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[cfg(all(test, feature = "desktop"))]
mod tests {
    use super::*;
    use crate::platform::http_client::Response;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Records every request and returns canned responses popped FIFO.
    pub(crate) struct MockHttpClient {
        canned: Mutex<VecDeque<Result<Response, String>>>,
        recorded: Mutex<Vec<RecordedRequest>>,
    }

    #[derive(Debug, Clone)]
    pub(crate) struct RecordedRequest {
        pub method: &'static str,
        pub url: String,
        pub body: Vec<u8>,
    }

    impl MockHttpClient {
        pub fn new() -> Self {
            Self {
                canned: Mutex::new(VecDeque::new()),
                recorded: Mutex::new(Vec::new()),
            }
        }

        pub fn push_response(&self, status: u16, body: &str) {
            self.canned.lock().unwrap().push_back(Ok(Response {
                status,
                body: body.as_bytes().to_vec(),
                headers: Headers::new(),
            }));
        }

        pub fn requests(&self) -> Vec<RecordedRequest> {
            self.recorded.lock().unwrap().clone()
        }

        fn next_response(&self) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            match self.canned.lock().unwrap().pop_front() {
                Some(Ok(r)) => Ok(r),
                Some(Err(e)) => Err(e.into()),
                None => Err("MockHttpClient: no canned response remaining".into()),
            }
        }
    }

    #[async_trait]
    impl HttpClient for MockHttpClient {
        async fn get(
            &self,
            url: &str,
            _headers: &Headers,
        ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            self.recorded.lock().unwrap().push(RecordedRequest {
                method: "GET",
                url: url.to_string(),
                body: Vec::new(),
            });
            self.next_response()
        }

        async fn post(
            &self,
            url: &str,
            _headers: &Headers,
            body: &[u8],
        ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            self.recorded.lock().unwrap().push(RecordedRequest {
                method: "POST",
                url: url.to_string(),
                body: body.to_vec(),
            });
            self.next_response()
        }

        async fn put(
            &self,
            url: &str,
            _headers: &Headers,
            body: &[u8],
        ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            self.recorded.lock().unwrap().push(RecordedRequest {
                method: "PUT",
                url: url.to_string(),
                body: body.to_vec(),
            });
            self.next_response()
        }

        async fn delete(
            &self,
            url: &str,
            _headers: &Headers,
        ) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
            self.recorded.lock().unwrap().push(RecordedRequest {
                method: "DELETE",
                url: url.to_string(),
                body: Vec::new(),
            });
            self.next_response()
        }

        async fn stream_post(
            &self,
            _url: &str,
            _headers: &Headers,
            _body: &[u8],
            _on_chunk: Box<dyn FnMut(String) + Send>,
        ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            unreachable!("stream_post not used in Telegram path")
        }
    }

    // ───────── Poller tests ─────────

    #[tokio::test]
    async fn poll_empty_result_returns_empty_vec() {
        let http = MockHttpClient::new();
        http.push_response(200, r#"{"ok":true,"result":[]}"#);
        let mut p = Poller::new("TOKEN".to_string());
        let msgs = p.poll_once(&http, 10).await.unwrap();
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn poll_one_text_message_extracted() {
        let http = MockHttpClient::new();
        http.push_response(
            200,
            r#"{"ok":true,"result":[{
                "update_id": 100,
                "message": {
                    "chat": {"id": 42},
                    "text": "hello bot",
                    "from": {"username": "alice"}
                }
            }]}"#,
        );
        let mut p = Poller::new("TOKEN".to_string());
        let msgs = p.poll_once(&http, 10).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].chat_id, "42");
        assert_eq!(msgs[0].text, "hello bot");
        assert_eq!(msgs[0].from_username.as_deref(), Some("alice"));
    }

    #[tokio::test]
    async fn poll_advances_offset() {
        let http = MockHttpClient::new();
        http.push_response(
            200,
            r#"{"ok":true,"result":[
                {"update_id": 100, "message": {"chat": {"id": 1}, "text": "a"}},
                {"update_id": 102, "message": {"chat": {"id": 1}, "text": "b"}}
            ]}"#,
        );
        http.push_response(200, r#"{"ok":true,"result":[]}"#);
        let mut p = Poller::new("TOKEN".to_string());
        let _ = p.poll_once(&http, 10).await.unwrap();
        // Second call should request offset=103.
        let _ = p.poll_once(&http, 10).await.unwrap();
        let urls: Vec<String> = http.requests().into_iter().map(|r| r.url).collect();
        assert_eq!(urls.len(), 2);
        assert!(urls[0].contains("offset=0"), "first url={}", urls[0]);
        assert!(urls[1].contains("offset=103"), "second url={}", urls[1]);
    }

    #[tokio::test]
    async fn poll_skips_update_without_message() {
        let http = MockHttpClient::new();
        http.push_response(
            200,
            r#"{"ok":true,"result":[
                {"update_id": 1, "callback_query": {"id": "x"}},
                {"update_id": 2, "message": {"chat": {"id": 9}, "text": "real"}}
            ]}"#,
        );
        let mut p = Poller::new("TOKEN".to_string());
        let msgs = p.poll_once(&http, 10).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "real");
    }

    #[tokio::test]
    async fn poll_skips_message_without_text() {
        let http = MockHttpClient::new();
        http.push_response(
            200,
            r#"{"ok":true,"result":[
                {"update_id": 1, "message": {"chat": {"id": 9}, "photo": []}},
                {"update_id": 2, "message": {"chat": {"id": 9}, "text": "hi"}}
            ]}"#,
        );
        let mut p = Poller::new("TOKEN".to_string());
        let msgs = p.poll_once(&http, 10).await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].text, "hi");
    }

    #[tokio::test]
    async fn poll_malformed_json_errors() {
        let http = MockHttpClient::new();
        http.push_response(200, "not json at all{{{");
        let mut p = Poller::new("TOKEN".to_string());
        let result = p.poll_once(&http, 10).await;
        assert!(result.is_err(), "expected parse error, got {:?}", result.is_ok());
    }

    #[tokio::test]
    async fn poll_non_200_errors() {
        let http = MockHttpClient::new();
        http.push_response(401, r#"{"ok":false,"description":"unauthorized"}"#);
        let mut p = Poller::new("BAD".to_string());
        let result = p.poll_once(&http, 10).await;
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("401"), "error should mention status: {}", msg);
    }

    // ───────── TelegramChannel tests ─────────

    fn parse_body_json(req: &RecordedRequest) -> serde_json::Value {
        serde_json::from_slice(&req.body)
            .unwrap_or_else(|e| panic!("body not JSON: {} ({:?})", e, req.body))
    }

    #[tokio::test]
    async fn channel_deliver_posts_sendmessage_with_chat_and_text() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("99", "hello").await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert!(
            reqs[0].url.contains("/botTOKEN/sendMessage"),
            "url={}",
            reqs[0].url
        );
        let body = parse_body_json(&reqs[0]);
        assert_eq!(body["chat_id"], "99");
        assert_eq!(body["text"], "hello");
    }

    #[tokio::test]
    async fn channel_deliver_uses_html_parse_mode_and_renders_markdown() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("1", "be **bold**").await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        let body = parse_body_json(&reqs[0]);
        assert_eq!(body["parse_mode"], "HTML");
        assert_eq!(body["text"], "be <b>bold</b>");
    }

    #[tokio::test]
    async fn channel_deliver_falls_back_to_plain_text_on_400() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(400, r#"{"ok":false,"description":"can't parse entities"}"#);
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("1", "be **bold**").await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 2, "expected an HTML attempt then a plain retry");
        // First attempt: HTML.
        assert_eq!(parse_body_json(&reqs[0])["parse_mode"], "HTML");
        // Retry: no parse_mode, tags stripped.
        let retry = parse_body_json(&reqs[1]);
        assert!(retry.get("parse_mode").is_none(), "retry must be plain: {:?}", retry);
        assert_eq!(retry["text"], "be bold");
    }

    #[tokio::test]
    async fn channel_deliver_403_propagates_without_fallback() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(403, r#"{"ok":false,"description":"forbidden"}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        let result = ch.deliver("1", "hi").await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("403"));
        assert_eq!(http.requests().len(), 1, "403 must not trigger a retry");
    }

    #[tokio::test]
    async fn channel_deliver_long_message_is_chunked() {
        let http = Arc::new(MockHttpClient::new());
        // Enough 200s for however many chunks; extra canned responses are fine.
        for _ in 0..10 {
            http.push_response(200, r#"{"ok":true}"#);
        }
        let big: String = (0..500)
            .map(|n| format!("paragraph number {}", n))
            .collect::<Vec<_>>()
            .join("\n\n");

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.deliver("1", &big).await.unwrap();

        let reqs = http.requests();
        assert!(reqs.len() > 1, "long message should be split into multiple sends");
        for r in &reqs {
            let body = parse_body_json(r);
            let text = body["text"].as_str().unwrap();
            assert!(text.chars().count() <= 4096);
        }
    }

    #[tokio::test]
    async fn channel_deliver_non_200_errors() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(403, r#"{"ok":false,"description":"forbidden"}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        let result = ch.deliver("99", "blocked").await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("403"));
    }

    #[tokio::test]
    async fn channel_send_typing_posts_chataction() {
        let http = Arc::new(MockHttpClient::new());
        http.push_response(200, r#"{"ok":true}"#);

        let ch = TelegramChannel::new("TOKEN".to_string(), http.clone());
        ch.send_typing("99").await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        assert!(
            reqs[0].url.contains("/botTOKEN/sendChatAction"),
            "url={}",
            reqs[0].url
        );
        let body = parse_body_json(&reqs[0]);
        assert_eq!(body["chat_id"], "99");
        assert_eq!(body["action"], "typing");
    }

    // ───────── Poller::set_my_commands tests ─────────

    #[tokio::test]
    async fn set_my_commands_posts_correct_url_and_body() {
        let http = MockHttpClient::new();
        http.push_response(200, r#"{"ok":true,"result":true}"#);

        let p = Poller::new("TOKEN".to_string());
        let cmds = &[
            ("new",    "Start a fresh chat"),
            ("clear",  "Wipe history"),
        ];
        p.set_my_commands(&http, cmds).await.unwrap();

        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert!(reqs[0].url.contains("/setMyCommands"),
            "URL was: {}", reqs[0].url);

        let body = parse_body_json(&reqs[0]);
        let arr = body["commands"].as_array().expect("commands array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["command"], "new");
        assert_eq!(arr[0]["description"], "Start a fresh chat");
    }

    #[tokio::test]
    async fn set_my_commands_non_200_errors() {
        let http = MockHttpClient::new();
        http.push_response(429, r#"{"ok":false,"description":"Too Many Requests"}"#);

        let p = Poller::new("TOKEN".to_string());
        let result = p.set_my_commands(&http, &[("ping", "Test")]).await;
        assert!(result.is_err(), "non-200 should bubble as Err");
    }
}
