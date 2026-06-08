//! Recovery of tool calls that a model leaked into assistant *content* as XML
//! markup instead of emitting structured `tool_calls`.
//!
//! Some models (seen with certain OpenAI-compat providers) render their tool
//! invocations as an XML block in the reply text:
//!
//! ```text
//! <calls>
//! <web action="request" method="POST" url="https://api…"
//!      headers='{"Authorization":"Bearer …"}' body='{"hive_id":79583}'></web>
//! <memory action="search" query="queen endpoint"></memory>
//! </calls>
//! ```
//!
//! The runner never sees a real `tool_calls` array, so without recovery these
//! calls never execute — they just dump into the Telegram/web reply as text.
//! [`recover_xml_tool_calls`] parses that block back into [`ToolCall`]s.
//!
//! Each XML tag name is the tool name and each attribute is an argument.
//! Attribute values are coerced to the tool's JSON-schema declared type, so
//! `web`'s `headers` (schema `object`) becomes a JSON object while `body`
//! (schema `string`) stays a string. Only tags whose name matches a known
//! tool are recovered, which keeps stray prose markup from becoming bogus
//! calls.

use crate::core::types::{FunctionCall, ToolCall, ToolDefinition};

const CALLS_OPEN: &str = "<calls>";
const CALLS_CLOSE: &str = "</calls>";

/// Recover tool calls leaked as `<calls>…</calls>` XML in `content`.
///
/// Returns `None` when the wrapper is absent or no known-tool tag is found, so
/// callers can treat the content as ordinary text. Engages only when the
/// `<calls>` marker is present to avoid false positives on normal replies.
pub fn recover_xml_tool_calls(content: &str, tools: &[ToolDefinition]) -> Option<Vec<ToolCall>> {
    let open = content.find(CALLS_OPEN)?;
    let region_start = open + CALLS_OPEN.len();
    let region_end = content[region_start..]
        .rfind(CALLS_CLOSE)
        .map(|e| region_start + e)
        .unwrap_or(content.len());
    let chars: Vec<char> = content[region_start..region_end].chars().collect();

    let mut calls = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // A tool tag opens with '<' followed by a name char (not a closing
        // '</…>'). Everything else — prose, closing tags, whitespace — is
        // skipped one char at a time.
        if chars[i] == '<' && i + 1 < chars.len() && chars[i + 1] != '/' {
            if let Some((maybe_call, next)) = parse_tag(&chars, i, tools, calls.len()) {
                if let Some(tc) = maybe_call {
                    calls.push(tc);
                }
                i = next;
                continue;
            }
        }
        i += 1;
    }

    if calls.is_empty() {
        None
    } else {
        Some(calls)
    }
}

/// Parse a single `<name attr="v" …>` open tag starting at `start` (which must
/// index a '<'). Returns `(maybe_call, index_after_open_tag)`: the call is
/// `Some` only when `name` matches a known tool. Returns `None` if the bytes at
/// `start` are not a well-formed open tag.
fn parse_tag(
    chars: &[char],
    start: usize,
    tools: &[ToolDefinition],
    idx: usize,
) -> Option<(Option<ToolCall>, usize)> {
    let mut j = start + 1;
    let name_start = j;
    while j < chars.len() && is_name_char(chars[j]) {
        j += 1;
    }
    let name: String = chars[name_start..j].iter().collect();
    if name.is_empty() {
        return None;
    }

    let mut attrs: Vec<(String, String)> = Vec::new();
    loop {
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j >= chars.len() {
            return None; // unterminated tag
        }
        match chars[j] {
            '>' => {
                j += 1;
                break;
            }
            '/' => {
                // self-closing "/>"
                j += 1;
                while j < chars.len() && chars[j] != '>' {
                    j += 1;
                }
                if j < chars.len() {
                    j += 1;
                }
                break;
            }
            _ => {}
        }

        let an_start = j;
        while j < chars.len() && is_name_char(chars[j]) {
            j += 1;
        }
        let aname: String = chars[an_start..j].iter().collect();
        if aname.is_empty() {
            return None; // garbage where an attribute name was expected
        }

        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j >= chars.len() || chars[j] != '=' {
            // Valueless attribute (e.g. a boolean flag).
            attrs.push((aname, String::new()));
            continue;
        }
        j += 1; // '='
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if j >= chars.len() {
            return None;
        }
        let quote = chars[j];
        if quote != '"' && quote != '\'' {
            return None;
        }
        j += 1;
        let mut val = String::new();
        loop {
            if j >= chars.len() {
                return None; // unterminated value
            }
            let c = chars[j];
            if c == '\\' && j + 1 < chars.len() && chars[j + 1] == quote {
                // Unescape an escaped delimiter so the inner value stays valid
                // (e.g. JSON in a double-quoted attribute).
                val.push(quote);
                j += 2;
                continue;
            }
            if c == quote {
                j += 1;
                break;
            }
            val.push(c);
            j += 1;
        }
        attrs.push((aname, val));
    }

    let schema = tools.iter().find(|t| t.name == name).map(|t| &t.parameters);
    let call = schema.map(|schema| {
        let arguments = build_arguments(&attrs, schema);
        ToolCall {
            id: format!("call_xml_{}", idx),
            function: FunctionCall {
                name: name.clone(),
                arguments,
            },
            extra_content: None,
        }
    });
    Some((call, j))
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Build the JSON arguments string from XML attributes, coercing each value to
/// the type its property declares in the tool schema.
fn build_arguments(attrs: &[(String, String)], schema: &serde_json::Value) -> String {
    let mut map = serde_json::Map::new();
    for (k, v) in attrs {
        let ty = schema
            .get("properties")
            .and_then(|p| p.get(k))
            .and_then(|prop| prop.get("type"))
            .and_then(|t| t.as_str());
        map.insert(k.clone(), coerce(v, ty));
    }
    serde_json::to_string(&serde_json::Value::Object(map)).unwrap_or_else(|_| "{}".to_string())
}

/// Coerce a string attribute value to its schema-declared JSON type, falling
/// back to a string when the value doesn't fit.
fn coerce(v: &str, ty: Option<&str>) -> serde_json::Value {
    match ty {
        Some("object") | Some("array") => {
            serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string()))
        }
        Some("integer") => v
            .trim()
            .parse::<i64>()
            .map(serde_json::Value::from)
            .unwrap_or_else(|_| serde_json::Value::String(v.to_string())),
        Some("number") => v
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(v.to_string())),
        Some("boolean") => v
            .trim()
            .parse::<bool>()
            .map(serde_json::Value::from)
            .unwrap_or_else(|_| serde_json::Value::String(v.to_string())),
        _ => serde_json::Value::String(v.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn web_def() -> ToolDefinition {
        ToolDefinition {
            name: "web".to_string(),
            description: String::new(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action":  {"type": "string"},
                    "url":     {"type": "string"},
                    "method":  {"type": "string"},
                    "headers": {"type": "object"},
                    "body":    {"type": "string"},
                    "count":   {"type": "integer"},
                }
            }),
        }
    }

    fn memory_def() -> ToolDefinition {
        ToolDefinition {
            name: "memory".to_string(),
            description: String::new(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "action": {"type": "string"},
                    "query":  {"type": "string"},
                }
            }),
        }
    }

    fn args(tc: &ToolCall) -> serde_json::Value {
        serde_json::from_str(&tc.function.arguments).expect("arguments are JSON")
    }

    #[test]
    fn no_calls_marker_returns_none() {
        let tools = [web_def(), memory_def()];
        assert!(recover_xml_tool_calls("just a normal reply, no markup", &tools).is_none());
        assert!(recover_xml_tool_calls("", &tools).is_none());
    }

    #[test]
    fn recovers_web_request_with_typed_headers_and_string_body() {
        let content = r#"Let me try. <calls>
<web action="request" method="POST" url="https://api.beep.nl/api/queens" headers='{"Accept": "application/json", "Authorization": "Bearer ABC", "Content-Type": "application/json"}' body='{"hive_id": 79583, "race_id": 924}'></web>
</calls> done."#;
        let tools = [web_def(), memory_def()];
        let calls = recover_xml_tool_calls(content, &tools).expect("recovered");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "web");
        let a = args(&calls[0]);
        assert_eq!(a["action"], "request");
        assert_eq!(a["method"], "POST");
        assert_eq!(a["url"], "https://api.beep.nl/api/queens");
        // headers schema type=object → nested JSON object, not a string.
        assert!(a["headers"].is_object(), "headers must be an object: {a}");
        assert_eq!(a["headers"]["Authorization"], "Bearer ABC");
        // body schema type=string → kept as a string, even though it is JSON.
        assert!(a["body"].is_string(), "body must stay a string: {a}");
        assert_eq!(a["body"], r#"{"hive_id": 79583, "race_id": 924}"#);
    }

    #[test]
    fn recovers_multiple_calls_in_one_block() {
        let content = r#"<calls>
<memory action="search" query="queen beep POST api endpoint"></memory>
<memory action="search" query="queens store create"></memory>
</calls>"#;
        let tools = [web_def(), memory_def()];
        let calls = recover_xml_tool_calls(content, &tools).expect("recovered");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "memory");
        assert_eq!(args(&calls[0])["query"], "queen beep POST api endpoint");
        assert_eq!(args(&calls[1])["query"], "queens store create");
        // Distinct ids so the agent loop can match results.
        assert_ne!(calls[0].id, calls[1].id);
    }

    #[test]
    fn unknown_tags_inside_calls_are_ignored() {
        // Only `memory` is known here; the `<thinking>` tag must not become a call.
        let content = r#"<calls>
<thinking>let me reason about this</thinking>
<memory action="list"></memory>
</calls>"#;
        let tools = [memory_def()];
        let calls = recover_xml_tool_calls(content, &tools).expect("recovered");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "memory");
        assert_eq!(args(&calls[0])["action"], "list");
    }

    #[test]
    fn integer_attribute_is_coerced_to_number() {
        let content = r#"<calls><web action="search" query="bees" count="5"></web></calls>"#;
        let tools = [web_def()];
        let calls = recover_xml_tool_calls(content, &tools).expect("recovered");
        let a = args(&calls[0]);
        assert_eq!(a["count"], 5);
        assert!(a["count"].is_number(), "count must be a number: {a}");
        assert_eq!(a["query"], "bees");
    }

    #[test]
    fn truncated_closing_tag_still_recovers() {
        // Missing </calls> (model output cut off) — recover what's there.
        let content = r#"<calls>
<memory action="search" query="x"></memory>"#;
        let tools = [memory_def()];
        let calls = recover_xml_tool_calls(content, &tools).expect("recovered");
        assert_eq!(calls.len(), 1);
        assert_eq!(args(&calls[0])["query"], "x");
    }

    #[test]
    fn calls_block_with_no_known_tools_returns_none() {
        let content = r#"<calls><foo bar="1"></foo></calls>"#;
        let tools = [web_def(), memory_def()];
        assert!(recover_xml_tool_calls(content, &tools).is_none());
    }
}
