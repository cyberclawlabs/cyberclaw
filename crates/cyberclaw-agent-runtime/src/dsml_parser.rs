//! DeepSeek DSML tool-call markup parser.
//!
//! DeepSeek v4 (and some Qwen variants) emit tool-call intent as in-content
//! XML-ish markup using full-width pipe `｜` (U+FF5C) instead of populating
//! the OpenAI `tool_calls` response field. Example:
//!
//! ```text
//! <｜｜DSML｜｜tool_calls>
//! <｜｜DSML｜｜invoke name="web_search">
//! <｜｜DSML｜｜parameter name="query" string="true">max luo</｜｜DSML｜｜parameter>
//! <｜｜DSML｜｜parameter name="count" string="false">10</｜｜DSML｜｜parameter>
//! </｜｜DSML｜｜invoke>
//! </｜｜DSML｜｜tool_calls>
//! ```
//!
//! This module parses that into `Vec<ToolCall>` so the existing agentic-loop
//! dispatch path (which already handles OpenAI `tool_calls`) just works.

use cyberclaw_llm::types::{FunctionCall, ToolCall};
use serde_json::Value;

const MARKER: &str = "\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}";

/// Multi-format dialect descriptors. Each dialect uses the same
/// `<invoke name="X">...<parameter name="Y" ...>VAL</parameter></invoke>`
/// structure but with vendor-specific tag prefixes.
struct Dialect {
    invoke_open_prefix: String,
    invoke_close: String,
    param_open_prefix: String,
    param_close: String,
    id_prefix: &'static str,
}

fn dialects() -> Vec<Dialect> {
    vec![
        // DeepSeek DSML — full-width pipe wrapped
        Dialect {
            invoke_open_prefix: format!("<{}invoke ", MARKER),
            invoke_close: format!("</{}invoke>", MARKER),
            param_open_prefix: format!("<{}parameter ", MARKER),
            param_close: format!("</{}parameter>", MARKER),
            id_prefix: "dsml",
        },
        // Anthropic-style bare tags (also emitted by some Qwen/Yi/Mistral)
        Dialect {
            invoke_open_prefix: "<invoke ".to_string(),
            invoke_close: "</invoke>".to_string(),
            param_open_prefix: "<parameter ".to_string(),
            param_close: "</parameter>".to_string(),
            id_prefix: "anthropic",
        },
    ]
}

/// Returns `Some(Vec<ToolCall>)` if any vendor tool-call markup was parsed,
/// else `None`. Supports DeepSeek DSML and Anthropic-style invoke blocks.
pub fn parse_dsml_tool_calls(content: &str) -> Option<Vec<ToolCall>> {
    let mut calls: Vec<ToolCall> = Vec::new();
    for d in dialects() {
        let mut rest = content;
        let mut idx = 0usize;
        while let Some(open_at) = rest.find(&d.invoke_open_prefix) {
            let after_open_prefix = &rest[open_at + d.invoke_open_prefix.len()..];
            let gt = match after_open_prefix.find('>') {
                Some(p) => p,
                None => break,
            };
            let attrs_str = &after_open_prefix[..gt];
            let name = match extract_attr(attrs_str, "name") {
                Some(n) => n,
                None => {
                    rest = &after_open_prefix[gt + 1..];
                    continue;
                }
            };
            let body_start = open_at + d.invoke_open_prefix.len() + gt + 1;
            let close_at_rel = match rest[body_start..].find(&d.invoke_close) {
                Some(p) => p,
                None => break,
            };
            let body = &rest[body_start..body_start + close_at_rel];
            let args = parse_parameters_with(body, &d);
            let args_json =
                serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string());
            calls.push(ToolCall {
                id: format!("{}-{}", d.id_prefix, idx),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name,
                    arguments: args_json,
                },
            });
            idx += 1;
            rest = &rest[body_start + close_at_rel + d.invoke_close.len()..];
        }
    }
    if calls.is_empty() {
        None
    } else {
        Some(calls)
    }
}

/// Strip every vendor tool-call markup block (DSML, Anthropic invoke,
/// DeepSeek's fake `<functionResults>` hallucination) from content,
/// leaving narrative text intact by cutting at the first marker.
pub fn strip_dsml(content: &str) -> String {
    let tc_open = format!("<{}tool_calls>", MARKER);
    let invoke_dsml = format!("<{}invoke ", MARKER);
    let any_dsml = format!("<{}", MARKER);

    let cut_at = [
        tc_open.as_str(),
        invoke_dsml.as_str(),
        any_dsml.as_str(),
        // Anthropic-style
        "<invoke ",
        "<invoke>",
        // DeepSeek hallucinated tool-result wrap (we never produced this; if
        // model emits it, it's pretending — strip everything from here)
        "<functionResults>",
        "<function_results>",
        // OpenAI text-mode (sometimes leaks when not using tool_calls field)
        "<function_call>",
    ]
    .iter()
    .filter_map(|m| content.find(m))
    .min();

    match cut_at {
        Some(pos) => content[..pos].trim_end().to_string(),
        None => content.to_string(),
    }
}

fn extract_attr(attrs: &str, key: &str) -> Option<String> {
    // attrs looks like: `name="web_search"` or `name="web_search" string="true"`
    let needle = format!("{}=\"", key);
    let i = attrs.find(&needle)?;
    let val_start = i + needle.len();
    let val_end_rel = attrs[val_start..].find('"')?;
    Some(attrs[val_start..val_start + val_end_rel].to_string())
}

// Legacy convenience for tests and external callers — uses DSML dialect.
#[allow(dead_code)]
fn parse_parameters(body: &str) -> serde_json::Map<String, Value> {
    parse_parameters_with(
        body,
        &Dialect {
            invoke_open_prefix: format!("<{}invoke ", MARKER),
            invoke_close: format!("</{}invoke>", MARKER),
            param_open_prefix: format!("<{}parameter ", MARKER),
            param_close: format!("</{}parameter>", MARKER),
            id_prefix: "dsml",
        },
    )
}

fn parse_parameters_with(body: &str, d: &Dialect) -> serde_json::Map<String, Value> {
    let param_open_prefix = &d.param_open_prefix;
    let param_close = &d.param_close;

    let mut map = serde_json::Map::new();
    let mut rest = body;
    while let Some(open_at) = rest.find(param_open_prefix.as_str()) {
        let after_open = &rest[open_at + param_open_prefix.len()..];
        let gt = match after_open.find('>') {
            Some(p) => p,
            None => break,
        };
        let attrs_str = &after_open[..gt];
        let name = match extract_attr(attrs_str, "name") {
            Some(n) => n,
            None => {
                rest = &after_open[gt + 1..];
                continue;
            }
        };
        // string="true" → keep as String; string="false" → parse as number/json
        let is_string = extract_attr(attrs_str, "string")
            .map(|v| v == "true")
            .unwrap_or(true);

        let body_start = open_at + param_open_prefix.len() + gt + 1;
        let close_at_rel = match rest[body_start..].find(param_close.as_str()) {
            Some(p) => p,
            None => break,
        };
        let raw_val = &rest[body_start..body_start + close_at_rel];
        let val = if is_string {
            Value::String(raw_val.trim().to_string())
        } else {
            // try number / bool / json, fallback to string
            serde_json::from_str(raw_val.trim()).unwrap_or_else(|_| Value::String(raw_val.trim().to_string()))
        };
        map.insert(name, val);

        rest = &rest[body_start + close_at_rel + param_close.len()..];
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "我将搜索。\n<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>\n<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke name=\"web_search\">\n<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter name=\"query\" string=\"true\">max luo</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter>\n<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter name=\"count\" string=\"false\">10</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}parameter>\n</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}invoke>\n</\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}tool_calls>";

    #[test]
    fn parses_single_invoke() {
        let calls = parse_dsml_tool_calls(SAMPLE).expect("should parse");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "web_search");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["query"], "max luo");
        assert_eq!(args["count"], 10);
    }

    #[test]
    fn strip_keeps_narrative() {
        let out = strip_dsml(SAMPLE);
        assert_eq!(out, "我将搜索。");
    }

    #[test]
    fn returns_none_when_no_dsml() {
        assert!(parse_dsml_tool_calls("plain text reply").is_none());
        assert_eq!(strip_dsml("plain text reply"), "plain text reply");
    }

    #[test]
    fn parses_anthropic_invoke() {
        let s = "我会搜索。\n<invoke name=\"web_search\">\n<parameter name=\"query\" string=\"true\">rust</parameter>\n<parameter name=\"limit\" string=\"false\">5</parameter>\n</invoke>";
        let calls = parse_dsml_tool_calls(s).expect("should parse");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "web_search");
        let args: Value = serde_json::from_str(&calls[0].function.arguments).unwrap();
        assert_eq!(args["query"], "rust");
        assert_eq!(args["limit"], 5);
        // Strip cuts at first <invoke
        assert_eq!(strip_dsml(s), "我会搜索。");
    }

    #[test]
    fn strip_handles_hallucinated_function_results() {
        let s = "我搜索到了。<functionResults>{\"fake\": true}</functionResults>";
        assert_eq!(strip_dsml(s), "我搜索到了。");
    }
}
