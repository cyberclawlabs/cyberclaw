//! Sanitize tool JSON schemas for broad LLM-backend compatibility.
//!
//! Mirrors `tools/schema_sanitizer.py` from hermes-agent v0.12.0. Some local
//! inference backends (notably llama.cpp's GBNF grammar generator) reject
//! shapes that OpenAI / Anthropic silently accept:
//!
//! - `{"type": "object"}` with no `properties` → add `properties: {}`.
//! - `additionalProperties: "object"` (string instead of dict) → replace.
//! - `"type": ["string", "null"]` → keep only `"string"` (collapse null).
//! - `anyOf` / `oneOf` whose only purpose is to permit `null` (common
//!   Pydantic/MCP optional-field shape) → collapse to the non-null branch.
//! - Unconstrained `additionalProperties` on empty-properties objects.
//!
//! This module walks the final tool schema tree (after standard mappings +
//! MCP-level normalization) and fixes the known-hostile constructs in-place
//! on a deep clone. Intentionally conservative: only modifies shapes that
//! the LLM backend couldn't use anyway.

use serde_json::{Map, Value};

/// Return a sanitized clone of `tools`. Input is the OpenAI-format tool list
/// `[{"type": "function", "function": {"name": ..., "parameters": {...}}}]`.
/// The output is independent of the input.
pub fn sanitize_tool_schemas(tools: &[Value]) -> Vec<Value> {
    tools.iter().map(sanitize_single_tool).collect()
}

fn sanitize_single_tool(tool: &Value) -> Value {
    let mut out = tool.clone();
    if let Some(function) = out.get_mut("function").and_then(Value::as_object_mut) {
        if let Some(params) = function.get_mut("parameters") {
            sanitize_node(params);
        }
        if let Some(input_schema) = function.get_mut("input_schema") {
            sanitize_node(input_schema);
        }
    }
    // Anthropic-style: `input_schema` lives at the top level.
    if let Some(input_schema) = out.as_object_mut().and_then(|o| o.get_mut("input_schema")) {
        sanitize_node(input_schema);
    }
    out
}

/// Walk a schema node and fix known-hostile constructs in place.
fn sanitize_node(node: &mut Value) {
    // 1. Collapse `anyOf` / `oneOf` whose only purpose is to permit null.
    collapse_nullable_union(node);

    // 2. Collapse `type: [...]` arrays to single string when possible.
    collapse_type_array(node);

    // 3. Coerce `additionalProperties` of bare string `"object"` → empty dict.
    fix_additional_properties_type(node);

    // 4. Add empty `properties: {}` to `type: object` schemas missing it.
    ensure_object_has_properties(node);

    // Recurse into common child positions.
    if let Some(obj) = node.as_object_mut() {
        for key in [
            "properties",
            "items",
            "additionalProperties",
            "patternProperties",
            "definitions",
            "$defs",
        ] {
            if let Some(child) = obj.get_mut(key) {
                if child.is_object() {
                    if let Some(map) = child.as_object_mut() {
                        // properties: walk each property's schema
                        if matches!(
                            key,
                            "properties" | "patternProperties" | "definitions" | "$defs"
                        ) {
                            for (_k, v) in map.iter_mut() {
                                sanitize_node(v);
                            }
                        } else {
                            // items / additionalProperties: schema directly
                            sanitize_node(child);
                        }
                    }
                } else if child.is_array() {
                    if let Some(arr) = child.as_array_mut() {
                        for v in arr.iter_mut() {
                            sanitize_node(v);
                        }
                    }
                }
            }
        }
        for key in ["allOf", "anyOf", "oneOf"] {
            if let Some(arr) = obj.get_mut(key).and_then(Value::as_array_mut) {
                for v in arr.iter_mut() {
                    sanitize_node(v);
                }
            }
        }
    }
}

/// `{"anyOf":[{"type":"string"},{"type":"null"}]}` → `{"type":"string"}`.
fn collapse_nullable_union(node: &mut Value) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    for key in ["anyOf", "oneOf"] {
        let drop = if let Some(arr) = obj.get(key).and_then(Value::as_array) {
            // Check: exactly two members, one with type=null, the other a real schema.
            if arr.len() == 2 {
                let null_idx = arr.iter().position(is_null_schema);
                let other_idx = arr.iter().position(|v| !is_null_schema(v));
                match (null_idx, other_idx) {
                    (Some(_), Some(idx)) => Some(arr[idx].clone()),
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };
        if let Some(replacement) = drop {
            obj.remove(key);
            // Merge replacement into node (replacement is the non-null branch).
            if let Some(rmap) = replacement.as_object() {
                for (k, v) in rmap.iter() {
                    obj.insert(k.clone(), v.clone());
                }
            }
            return; // Only collapse one of anyOf/oneOf per pass.
        }
    }
}

fn is_null_schema(v: &Value) -> bool {
    v.as_object()
        .and_then(|o| o.get("type"))
        .and_then(Value::as_str)
        .map(|s| s == "null")
        .unwrap_or(false)
}

/// `"type": ["string", "null"]` → `"type": "string"`.
fn collapse_type_array(node: &mut Value) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    let Some(arr) = obj.get("type").and_then(Value::as_array).cloned() else {
        return;
    };
    let non_null: Vec<&Value> = arr.iter().filter(|v| v.as_str() != Some("null")).collect();
    if non_null.len() == 1 {
        if let Some(s) = non_null[0].as_str() {
            obj.insert("type".to_string(), Value::String(s.to_string()));
        }
    }
}

/// `additionalProperties: "object"` (bare string) → `additionalProperties: {}`.
fn fix_additional_properties_type(node: &mut Value) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    if let Some(ap) = obj.get("additionalProperties") {
        if ap.is_string() {
            obj.insert(
                "additionalProperties".to_string(),
                Value::Object(Map::new()),
            );
        }
    }
}

/// `{"type":"object"}` without `properties` → add `properties: {}`.
fn ensure_object_has_properties(node: &mut Value) {
    let Some(obj) = node.as_object_mut() else {
        return;
    };
    let is_object = obj.get("type").and_then(Value::as_str) == Some("object");
    if is_object && !obj.contains_key("properties") {
        obj.insert("properties".to_string(), Value::Object(Map::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wrap_tool(params: Value) -> Value {
        json!({
            "type": "function",
            "function": { "name": "t", "parameters": params }
        })
    }

    #[test]
    fn nullable_anyof_collapses_to_non_null_branch() {
        let tool = wrap_tool(json!({
            "type": "object",
            "properties": {
                "x": {
                    "anyOf": [
                        { "type": "string", "minLength": 1 },
                        { "type": "null" }
                    ]
                }
            }
        }));
        let out = sanitize_tool_schemas(&[tool]);
        let x = &out[0]["function"]["parameters"]["properties"]["x"];
        assert_eq!(x["type"], "string");
        assert_eq!(x["minLength"], 1);
        assert!(x.get("anyOf").is_none());
    }

    #[test]
    fn type_array_with_null_collapses_to_single_type() {
        let tool = wrap_tool(json!({
            "type": "object",
            "properties": {
                "y": { "type": ["string", "null"] }
            }
        }));
        let out = sanitize_tool_schemas(&[tool]);
        assert_eq!(
            out[0]["function"]["parameters"]["properties"]["y"]["type"],
            "string"
        );
    }

    #[test]
    fn empty_object_gets_properties_added() {
        let tool = wrap_tool(json!({"type": "object"}));
        let out = sanitize_tool_schemas(&[tool]);
        let p = &out[0]["function"]["parameters"];
        assert_eq!(p["type"], "object");
        assert!(p["properties"].is_object());
    }

    #[test]
    fn additional_properties_string_is_replaced() {
        let tool = wrap_tool(json!({
            "type": "object",
            "properties": {},
            "additionalProperties": "object"
        }));
        let out = sanitize_tool_schemas(&[tool]);
        assert!(out[0]["function"]["parameters"]["additionalProperties"].is_object());
    }

    #[test]
    fn nested_oneof_inside_array_items_collapses() {
        let tool = wrap_tool(json!({
            "type": "object",
            "properties": {
                "list": {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            { "type": "integer" },
                            { "type": "null" }
                        ]
                    }
                }
            }
        }));
        let out = sanitize_tool_schemas(&[tool]);
        let item = &out[0]["function"]["parameters"]["properties"]["list"]["items"];
        assert_eq!(item["type"], "integer");
        assert!(item.get("oneOf").is_none());
    }

    #[test]
    fn anthropic_input_schema_is_also_sanitized() {
        let tool = json!({
            "name": "t",
            "input_schema": { "type": "object" }
        });
        let out = sanitize_tool_schemas(&[tool]);
        assert!(out[0]["input_schema"]["properties"].is_object());
    }

    #[test]
    fn untouched_schemas_pass_through_unchanged() {
        let tool = wrap_tool(json!({
            "type": "object",
            "properties": { "foo": { "type": "string" } },
            "required": ["foo"]
        }));
        let out = sanitize_tool_schemas(std::slice::from_ref(&tool));
        assert_eq!(out[0], tool);
    }
}
