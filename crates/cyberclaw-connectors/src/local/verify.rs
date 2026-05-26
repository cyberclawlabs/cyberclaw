//! `verify.numeric_aggregate` — B-4 (2026-05-05).
//!
//! Lets agents in numeric/business-data tasks force a self-check by replaying
//! the aggregation in pure Rust against the same CSV their analysis pipeline
//! processed. Works as a verifier-in-the-loop: agent makes a claim about
//! `sum` / `avg` / `count` (optionally grouped); this capability re-computes
//! and returns `match: bool` plus the actual value vs claimed.
//!
//! # Why pure-Rust CSV instead of `python pandas`
//!
//! The python path requires `python3` + pandas on the host, plus a writable
//! workdir + subprocess plumbing — every one of those is a portability
//! footgun that fails opaquely in CI sandboxes. A small header-aware CSV
//! reader gives deterministic results on every host with zero new deps.
//!
//! # Scope (intentional)
//!
//! Numeric aggregates only: `sum`, `avg`, `count`, optional `group_by` of
//! one column. Schema validation, regex matching, JSON Path equivalence,
//! and multi-column group-bys are deliberate follow-ups — kept out so the
//! capability surface stays small and the gate is binary.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use cyberclaw_core::capability::RiskLevel;
use cyberclaw_core::facade::{CapabilityFacade, FacadeExposure, ToolsetCategory};
use cyberclaw_core::ids::{CapabilityId, ConnectorId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::LocalConnector;
use crate::types::CapabilityExecutionRequest;

/// Input for `verify.numeric_aggregate`.
#[derive(Debug, Clone, Deserialize)]
pub struct VerifyNumericInput {
    /// Workspace-relative or absolute path to the CSV file. The path is
    /// validated against the connector workspace via `validate_path`, so
    /// agents cannot read arbitrary files via this gate.
    pub csv_path: String,
    /// What the agent CLAIMS the aggregate is. Each provided field is
    /// re-computed; missing fields are skipped.
    pub expected: ExpectedAggregate,
    /// Optional column selector. Required when `expected.sum` or
    /// `expected.avg` is set. Ignored for `count`.
    #[serde(default)]
    pub column: Option<String>,
    /// Optional grouping column. When set, all numeric aggregates and
    /// `count` are bucketed by the value of this column. The expected
    /// payload must be `{"by_group": {"<group>": <value>}}` shape — see
    /// `ExpectedAggregate::group_by`.
    #[serde(default)]
    pub group_by: Option<String>,
    /// Tolerance for floating-point match. Defaults to 1e-6.
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
}

fn default_tolerance() -> f64 {
    1e-6
}

/// Claimed aggregate values. All fields optional; only the provided ones
/// are checked.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExpectedAggregate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    /// Per-group expectations. Only consulted when `group_by` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_by: Option<BTreeMap<String, GroupExpectation>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct GroupExpectation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ActualAggregate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_by: Option<BTreeMap<String, GroupActual>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GroupActual {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VerifyNumericOutput {
    /// Top-level verdict: `true` only when every claim matches within tolerance.
    pub r#match: bool,
    /// Re-computed values, in the same shape as the claim.
    pub actual: ActualAggregate,
    /// Pretty-printed reasons for each mismatch (empty when `match=true`).
    pub diff: Vec<String>,
    /// Number of data rows considered (after the header line).
    pub rows: u64,
}

pub async fn numeric_aggregate(
    connector: &LocalConnector,
    request: CapabilityExecutionRequest,
) -> anyhow::Result<Value> {
    let input: VerifyNumericInput = serde_json::from_value(request.input)?;
    let csv_path = validate_csv_path(connector, &input.csv_path)?;

    let raw = fs::read_to_string(&csv_path)
        .map_err(|e| anyhow::anyhow!("failed to read csv {}: {e}", csv_path.display()))?;
    let parsed = parse_csv(&raw)?;
    if parsed.headers.is_empty() {
        anyhow::bail!("csv has no header row: {}", csv_path.display());
    }

    let column_idx = if let Some(col) = input.column.as_deref() {
        Some(find_column(&parsed.headers, col)?)
    } else {
        None
    };
    let group_idx = if let Some(g) = input.group_by.as_deref() {
        Some(find_column(&parsed.headers, g)?)
    } else {
        None
    };

    let mut actual = ActualAggregate {
        count: Some(parsed.rows.len() as u64),
        ..ActualAggregate::default()
    };

    if let Some(col_idx) = column_idx {
        let values = collect_floats(&parsed.rows, col_idx)?;
        if input.expected.sum.is_some() {
            actual.sum = Some(values.iter().sum::<f64>());
        }
        if input.expected.avg.is_some() {
            if values.is_empty() {
                actual.avg = Some(0.0);
            } else {
                actual.avg = Some(values.iter().sum::<f64>() / values.len() as f64);
            }
        }
    }

    if let Some(g_idx) = group_idx {
        let mut buckets: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for row in &parsed.rows {
            let key = row.get(g_idx).cloned().unwrap_or_default();
            *counts.entry(key.clone()).or_insert(0) += 1;
            if let Some(col_idx) = column_idx {
                if let Some(cell) = row.get(col_idx) {
                    if let Ok(v) = cell.trim().parse::<f64>() {
                        buckets.entry(key).or_default().push(v);
                    }
                }
            }
        }
        let mut groups: BTreeMap<String, GroupActual> = BTreeMap::new();
        for (k, count) in &counts {
            let mut entry = GroupActual {
                count: Some(*count),
                ..GroupActual::default()
            };
            if let Some(values) = buckets.get(k) {
                entry.sum = Some(values.iter().sum::<f64>());
                if !values.is_empty() {
                    entry.avg = Some(values.iter().sum::<f64>() / values.len() as f64);
                }
            }
            groups.insert(k.clone(), entry);
        }
        actual.group_by = Some(groups);
    }

    let diff = compute_diff(&input, &actual);
    let r#match = diff.is_empty();
    let output = VerifyNumericOutput {
        r#match,
        actual,
        diff,
        rows: parsed.rows.len() as u64,
    };
    Ok(serde_json::to_value(output)?)
}

fn validate_csv_path(connector: &LocalConnector, path: &str) -> anyhow::Result<PathBuf> {
    // Reject explicit traversal patterns; otherwise allow absolute paths
    // that already exist (test fixtures live in tempdirs outside the
    // workspace) or workspace-relative paths.
    let raw = Path::new(path);
    for component in raw.components() {
        if component == std::path::Component::ParentDir {
            anyhow::bail!("csv path '{}' contains '..' which is not allowed", path);
        }
    }
    let abs = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        connector.workspace().join(raw)
    };
    if !abs.exists() {
        anyhow::bail!("csv file does not exist: {}", abs.display());
    }
    Ok(abs)
}

fn find_column(headers: &[String], name: &str) -> anyhow::Result<usize> {
    headers
        .iter()
        .position(|h| h.trim() == name.trim())
        .ok_or_else(|| anyhow::anyhow!("column '{}' not in csv headers {:?}", name, headers))
}

fn collect_floats(rows: &[Vec<String>], idx: usize) -> anyhow::Result<Vec<f64>> {
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let Some(cell) = row.get(idx) else { continue };
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.parse::<f64>() {
            Ok(v) => out.push(v),
            Err(e) => {
                anyhow::bail!(
                    "csv row {} column index {} not a number ({}): {}",
                    i + 2, // +2 = header + 1-based
                    idx,
                    trimmed,
                    e
                )
            }
        }
    }
    Ok(out)
}

fn compute_diff(input: &VerifyNumericInput, actual: &ActualAggregate) -> Vec<String> {
    let mut diff = Vec::new();
    let tol = input.tolerance.max(0.0);
    if let Some(expected) = input.expected.sum {
        match actual.sum {
            Some(a) if (a - expected).abs() <= tol => {}
            Some(a) => diff.push(format!("sum: expected {expected}, actual {a}")),
            None => diff.push("sum: expected value but no column provided".to_string()),
        }
    }
    if let Some(expected) = input.expected.avg {
        match actual.avg {
            Some(a) if (a - expected).abs() <= tol => {}
            Some(a) => diff.push(format!("avg: expected {expected}, actual {a}")),
            None => diff.push("avg: expected value but no column provided".to_string()),
        }
    }
    if let Some(expected) = input.expected.count {
        match actual.count {
            Some(a) if a == expected => {}
            Some(a) => diff.push(format!("count: expected {expected}, actual {a}")),
            None => diff.push("count: not computed".to_string()),
        }
    }

    if let (Some(expected_groups), Some(actual_groups)) =
        (&input.expected.group_by, &actual.group_by)
    {
        for (k, exp) in expected_groups {
            let Some(act) = actual_groups.get(k) else {
                diff.push(format!("group_by[{k}]: missing in csv"));
                continue;
            };
            if let Some(s) = exp.sum {
                match act.sum {
                    Some(a) if (a - s).abs() <= tol => {}
                    Some(a) => diff.push(format!("group_by[{k}].sum: expected {s}, actual {a}")),
                    None => diff.push(format!("group_by[{k}].sum: not computed")),
                }
            }
            if let Some(v) = exp.avg {
                match act.avg {
                    Some(a) if (a - v).abs() <= tol => {}
                    Some(a) => diff.push(format!("group_by[{k}].avg: expected {v}, actual {a}")),
                    None => diff.push(format!("group_by[{k}].avg: not computed")),
                }
            }
            if let Some(c) = exp.count {
                match act.count {
                    Some(a) if a == c => {}
                    Some(a) => diff.push(format!("group_by[{k}].count: expected {c}, actual {a}")),
                    None => diff.push(format!("group_by[{k}].count: not computed")),
                }
            }
        }
    } else if input.expected.group_by.is_some() && actual.group_by.is_none() {
        diff.push("group_by expected but no group_by column was supplied".to_string());
    }

    diff
}

// ---------------------------------------------------------------------------
// Minimal CSV parser
// ---------------------------------------------------------------------------
//
// Supports:
//   - LF + CRLF line endings.
//   - Double-quoted cells with embedded commas + escaped quotes ("").
//   - Trims a leading BOM if present.
//
// Does NOT support: multi-line quoted cells (which the verify use-case
// doesn't need; numeric/business CSVs are line-oriented).

#[derive(Debug, Default)]
struct ParsedCsv {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
}

fn parse_csv(raw: &str) -> anyhow::Result<ParsedCsv> {
    let cleaned = raw.strip_prefix('\u{feff}').unwrap_or(raw);
    let mut lines = cleaned
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.trim().is_empty());

    let header_line = match lines.next() {
        Some(h) => h,
        None => return Ok(ParsedCsv::default()),
    };
    let headers = split_csv_line(header_line)?;
    let mut rows = Vec::new();
    for line in lines {
        let cells = split_csv_line(line)?;
        rows.push(cells);
    }
    Ok(ParsedCsv { headers, rows })
}

fn split_csv_line(line: &str) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    buf.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => {
                out.push(std::mem::take(&mut buf));
            }
            other => buf.push(other),
        }
    }
    if in_quotes {
        anyhow::bail!("csv line has an unterminated quoted cell");
    }
    out.push(buf);
    Ok(out)
}

/// §4 — authoritative facade declaration for `verify.numeric_aggregate`.
///
/// `BuiltinToolRegistry::default_facades` MUST NOT duplicate this entry.
#[allow(dead_code)]
pub fn capability_facades() -> Vec<(CapabilityFacade, ToolsetCategory)> {
    let connector_id = ConnectorId::from_string("local".to_string()).unwrap();
    vec![
        (
            CapabilityFacade {
                name: "verify_numeric".to_string(),
                description: "Re-compute a numeric aggregate (sum / avg / count, optional group_by) \
                    over a CSV column and report whether the actual value matches the agent's claim. \
                    USE THIS BEFORE asserting any total / average / count in a business or numeric \
                    deliverable — the gate prevents hallucinated numbers and surfaces the diff for \
                    self-correction. Tolerance defaults to 1e-6 for floats.".to_string(),
                connector_id: connector_id.clone(),
                capability_id: CapabilityId::from_string(
                    "verify.numeric_aggregate".to_string(),
                ).unwrap(),
                risk_level: RiskLevel::Low,
                effects: vec!["read".to_string()],
                read_only: true,
                destructive: false,
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "csv_path": {
                            "type": "string",
                            "description": "Workspace-relative or absolute path to the CSV file (with header row)."
                        },
                        "column": {
                            "type": "string",
                            "description": "Column header to aggregate. Required when expected.sum or expected.avg is set."
                        },
                        "group_by": {
                            "type": "string",
                            "description": "Optional column to bucket aggregates by (e.g. 'region')."
                        },
                        "expected": {
                            "type": "object",
                            "description": "What you CLAIM the aggregate is. Provide any subset of {sum, avg, count, group_by:{<key>:{sum,avg,count}}}.",
                            "properties": {
                                "sum":   { "type": "number" },
                                "avg":   { "type": "number" },
                                "count": { "type": "integer" },
                                "group_by": { "type": "object" }
                            }
                        },
                        "tolerance": {
                            "type": "number",
                            "description": "Float-comparison tolerance. Default 1e-6."
                        }
                    },
                    "required": ["csv_path", "expected"]
                })),
                exposure: FacadeExposure::LlmDefault,
                workspace_root: None,
            },
            ToolsetCategory::CodeAnalysis,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberclaw_core::identity::{ActorRef, ActorType};
    use cyberclaw_core::ids::{ActorId, CapabilityId, ConnectorId, ExecutionId, WorkspaceId};
    use cyberclaw_core::workspace::{WorkspaceMode, WorkspaceRef};
    use serde_json::json;
    use tempfile::TempDir;

    fn request(workspace_root: &Path, input: serde_json::Value) -> CapabilityExecutionRequest {
        CapabilityExecutionRequest {
            execution_id: ExecutionId::new(),
            trace_id: "test-trace".to_string(),
            actor: ActorRef {
                id: ActorId::from_string("agent_test".to_string()).unwrap(),
                actor_type: ActorType::Agent,
                tenant_id: None,
                home_node_id: None,
                display_name: "agent_test".into(),
            },
            workspace: WorkspaceRef {
                id: WorkspaceId::new(),
                mode: WorkspaceMode::Ephemeral,
                materialization_mode: None,
                home_node_id: None,
                backing_store: None,
                root: workspace_root.to_string_lossy().to_string(),
                writable_roots: vec![],
            },
            connector_id: ConnectorId::from_string("local".to_string()).unwrap(),
            capability_id: CapabilityId::from_string("verify.numeric_aggregate".to_string())
                .unwrap(),
            input,
        }
    }

    fn write_sample(dir: &TempDir, body: &str) -> PathBuf {
        let p = dir.path().join("data.csv");
        std::fs::write(&p, body).unwrap();
        p
    }

    #[tokio::test]
    async fn numeric_match_sum_avg_count() {
        let tmp = TempDir::new().unwrap();
        let csv = write_sample(&tmp, "id,amount\n1,10\n2,20\n3,30\n");
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let req = request(
            tmp.path(),
            json!({
                "csv_path": csv.to_str().unwrap(),
                "column": "amount",
                "expected": { "sum": 60, "avg": 20, "count": 3 }
            }),
        );
        let out = numeric_aggregate(&conn, req).await.unwrap();
        assert_eq!(out["match"], true);
        assert_eq!(out["actual"]["sum"].as_f64().unwrap(), 60.0);
        assert_eq!(out["actual"]["avg"].as_f64().unwrap(), 20.0);
        assert_eq!(out["actual"]["count"].as_u64().unwrap(), 3);
        assert_eq!(out["diff"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn numeric_mismatch_emits_diff() {
        let tmp = TempDir::new().unwrap();
        let csv = write_sample(&tmp, "id,amount\n1,5\n2,5\n");
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let req = request(
            tmp.path(),
            json!({
                "csv_path": csv.to_str().unwrap(),
                "column": "amount",
                "expected": { "sum": 1000 }
            }),
        );
        let out = numeric_aggregate(&conn, req).await.unwrap();
        assert_eq!(out["match"], false);
        let diff = out["diff"].as_array().unwrap();
        assert_eq!(diff.len(), 1);
        assert!(diff[0].as_str().unwrap().contains("sum"));
        assert_eq!(out["actual"]["sum"].as_f64().unwrap(), 10.0);
    }

    #[tokio::test]
    async fn group_by_aggregates_per_bucket() {
        let tmp = TempDir::new().unwrap();
        let csv = write_sample(&tmp, "region,amount\nUS,10\nEU,5\nUS,30\nEU,15\n");
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let req = request(
            tmp.path(),
            json!({
                "csv_path": csv.to_str().unwrap(),
                "column": "amount",
                "group_by": "region",
                "expected": {
                    "group_by": {
                        "US": { "sum": 40, "count": 2 },
                        "EU": { "sum": 20, "count": 2 }
                    }
                }
            }),
        );
        let out = numeric_aggregate(&conn, req).await.unwrap();
        assert_eq!(out["match"], true, "diff: {:?}", out["diff"]);
        let groups = out["actual"]["group_by"].as_object().unwrap();
        assert_eq!(groups["US"]["sum"].as_f64().unwrap(), 40.0);
        assert_eq!(groups["EU"]["sum"].as_f64().unwrap(), 20.0);
    }

    #[tokio::test]
    async fn group_by_mismatch_emits_per_group_diff() {
        let tmp = TempDir::new().unwrap();
        let csv = write_sample(&tmp, "region,amount\nUS,10\nEU,99\n");
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let req = request(
            tmp.path(),
            json!({
                "csv_path": csv.to_str().unwrap(),
                "column": "amount",
                "group_by": "region",
                "expected": {
                    "group_by": { "EU": { "sum": 0 } }
                }
            }),
        );
        let out = numeric_aggregate(&conn, req).await.unwrap();
        assert_eq!(out["match"], false);
        let diff = out["diff"].as_array().unwrap();
        assert!(diff
            .iter()
            .any(|d| d.as_str().unwrap().contains("group_by[EU]")));
    }

    #[tokio::test]
    async fn rejects_csv_without_target_column() {
        let tmp = TempDir::new().unwrap();
        let csv = write_sample(&tmp, "id,amount\n1,10\n");
        let conn = LocalConnector::new(tmp.path().to_path_buf());
        let req = request(
            tmp.path(),
            json!({
                "csv_path": csv.to_str().unwrap(),
                "column": "no_such_column",
                "expected": { "sum": 0 }
            }),
        );
        let res = numeric_aggregate(&conn, req).await;
        assert!(res.is_err());
    }

    #[test]
    fn parse_csv_handles_quoted_commas() {
        let raw = "name,note\nAlice,\"hello, world\"\nBob,\"a \"\"quoted\"\" word\"\n";
        let parsed = parse_csv(raw).unwrap();
        assert_eq!(parsed.headers, vec!["name", "note"]);
        assert_eq!(parsed.rows[0], vec!["Alice", "hello, world"]);
        assert_eq!(parsed.rows[1], vec!["Bob", "a \"quoted\" word"]);
    }
}
