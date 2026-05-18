//! Skill 管理命令
//!
//! `skill list` 通过 HTTP 调用 server 的 `/api/v1/skills/registry`，
//! 显示 SkillHub 中所有已注册的 skill bundle。
//!
//! 历史背景：原版本读 CLI 进程内的 `state.package_registry`（一个永远空的
//! `InMemoryRegistry::new()`），是 client/server 拆分前 monolithic CLI 的
//! 残留死路径。W3 修复后改为 HTTP，跟 `tools state` / `cluster state` 的
//! 模式一致。

use crate::cli_state::CliState;
use crate::http_client::{get_json, server_url};
use crate::output::OutputFormat;
use clap::{Args, Subcommand};
use serde::Deserialize;

#[derive(Debug, Subcommand)]
pub enum SkillCommand {
    /// 列出已注册的 Skills（来自 server SkillHub）
    List(SkillListArgs),
    /// 从一次成功的对话提取 SKILL.md 草稿 — 人工 review 后再 mv 到 ecosystem/skills/.
    /// 不会自动写入 SkillHub，避免污染。
    Distill(SkillDistillArgs),
}

#[derive(Debug, Args)]
pub struct SkillDistillArgs {
    /// 要提取的请求 id（audit page 上的 target=request:&lt;UUID&gt;，复制 UUID 部分）。
    /// 注：audit 链以 request_id 为单位记录，不是 conversation_id。
    /// 一个对话可能有多个 request，需要从 audit page 找具体那一次成功的 request。
    #[arg(long, alias = "conv-id")]
    pub request_id: String,
    /// 新 skill 的 slug id（必填，例：write-q3-report）
    #[arg(long)]
    pub skill_id: String,
    /// 新 skill 的标题（可选，默认 = skill_id）
    #[arg(long)]
    pub title: Option<String>,
    /// 输出目录（默认 ./distilled-skills/<skill_id>/）— 写 SKILL.md 进去
    #[arg(long, default_value = "./distilled-skills")]
    pub out: String,
    /// CyberClaw server URL（覆盖 CYBERCLAW_SERVER 环境变量）
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Args)]
pub struct SkillListArgs {
    /// 输出格式
    #[arg(short, long, value_enum, default_value = "text")]
    pub format: OutputFormat,
    /// CyberClaw server URL（覆盖 CYBERCLAW_SERVER 环境变量）
    #[arg(long)]
    pub server: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistryResponse {
    #[serde(default)]
    bundles: Vec<SkillBundleView>,
    #[serde(default)]
    total: usize,
}

#[derive(Debug, Deserialize)]
struct SkillBundleView {
    #[serde(default)]
    skill_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    source: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    installed_at: String,
}

pub async fn handle_skill_command(cmd: SkillCommand, _state: &CliState) -> anyhow::Result<()> {
    match cmd {
        SkillCommand::List(args) => handle_list(args).await,
        SkillCommand::Distill(args) => handle_distill(args).await,
    }
}

/// v1.2 backlog #18 — Skills auto-distill MVP.
///
/// Read audit log filtered by action_prefix=agent.invoke, keep entries whose
/// `target` references the supplied conversation id, extract a sequence of
/// tool calls + the user's last prompt, render a SKILL.md draft, write to
/// `<out>/<skill_id>/SKILL.md`. **Does not auto-install into SkillHub** —
/// the human-in-loop step is intentional: a distilled skill that looks
/// usable but encodes a wrong success criterion would silently poison
/// future agent runs. User reviews + commits to `ecosystem/skills/`
/// themselves.
async fn handle_distill(args: SkillDistillArgs) -> anyhow::Result<()> {
    use serde_json::Value;
    let server = server_url(args.server.as_deref());

    // Pull all recent agent.invoke audit rows (server caps at ~1000 by default).
    let url = format!(
        "{}/api/v1/audit/logs?action_prefix=agent.invoke&limit=500",
        server.trim_end_matches('/')
    );
    let resp: Value = get_json(&server, &url[server.len()..])
        .await
        .map_err(|e| anyhow::anyhow!("failed to read audit logs: {e}"))?;
    let entries = resp
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Filter rows whose target == "request:<request_id>" — emitted by the
    // chat handler each loop iteration. request_id is per chat.message call,
    // not per conversation (audit chain doesn't index by conv_id).
    let target_pat = format!("request:{}", args.request_id);
    let matching: Vec<&Value> = entries
        .iter()
        .filter(|e| {
            e.get("target")
                .and_then(|t| t.as_str())
                .map(|s| s == target_pat || s.contains(&args.request_id))
                .unwrap_or(false)
        })
        .collect();

    if matching.is_empty() {
        anyhow::bail!(
            "no agent.invoke audit rows matched conv_id={}. \
             Either the conversation has no completed turns, or it never \
             went through /api/v1/chat/message.",
            args.request_id
        );
    }

    // Extract a flat tool-call list from the audit detail payloads.
    let mut tool_calls: Vec<(String, String)> = Vec::new(); // (name, args_snippet)
    let mut models_seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut last_finish: Option<String> = None;
    for entry in &matching {
        let detail = entry.get("detail").cloned().unwrap_or(Value::Null);
        if let Some(model) = detail.get("model").and_then(|v| v.as_str()) {
            models_seen.insert(model.to_string());
        }
        if let Some(finish) = detail.get("finish_reason").and_then(|v| v.as_str()) {
            last_finish = Some(finish.to_string());
        }
        if let Some(arr) = detail.get("tool_calls").and_then(|v| v.as_array()) {
            for tc in arr {
                let name = tc
                    .get("name")
                    .or_else(|| tc.get("function").and_then(|f| f.get("name")))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                let args_snip = tc
                    .get("arguments")
                    .or_else(|| tc.get("function").and_then(|f| f.get("arguments")))
                    .map(|v| {
                        let s = serde_json::to_string(v).unwrap_or_default();
                        if s.len() > 200 {
                            format!("{}…", &s[..200])
                        } else {
                            s
                        }
                    })
                    .unwrap_or_default();
                tool_calls.push((name, args_snip));
            }
        }
    }

    // Render the draft.
    let title = args
        .title
        .clone()
        .unwrap_or_else(|| args.skill_id.replace('-', " "));
    let models_line = if models_seen.is_empty() {
        "(unknown)".to_string()
    } else {
        models_seen.into_iter().collect::<Vec<_>>().join(", ")
    };
    let finish_line = last_finish.as_deref().unwrap_or("(unknown)");
    let tool_section = if tool_calls.is_empty() {
        "_No tool_calls captured. Either the audit detail schema changed, or this conversation produced only text answers._".to_string()
    } else {
        let mut out = String::new();
        for (i, (name, args_snip)) in tool_calls.iter().enumerate() {
            out.push_str(&format!("{}. `{name}`\n   - args: `{args_snip}`\n", i + 1));
        }
        out
    };
    let draft = format!(
        "---\n\
         name: {skill_id}\n\
         title: {title}\n\
         version: 0.1.0-draft\n\
         distilled_from_conv: {conv}\n\
         distilled_at: {now}\n\
         status: draft  # set to 'published' after human review + move to ecosystem/skills/\n\
         ---\n\
         \n\
         # {title}\n\
         \n\
         > **Status**: this skill was auto-distilled from a single successful conversation\n\
         > (`{conv}`). Treat it as a starting point. Before publishing:\n\
         > 1. Review the captured tool-call sequence below — is it the MINIMAL path?\n\
         > 2. Generalize parameters that were hard-coded in the original conversation.\n\
         > 3. Write a clear success criterion (what does \"done\" look like?).\n\
         > 4. Move this file to `ecosystem/skills/{skill_id}/SKILL.md` and bump status.\n\
         \n\
         ## When to invoke\n\
         \n\
         _TODO: describe the task type that should trigger this skill (e.g.,\n\
         \"user asks to generate a Q3 product roadmap deck\")._\n\
         \n\
         ## Required capabilities\n\
         \n\
         _TODO: list the Connector/Capability ids that this skill depends on,\n\
         derived from the tool-call sequence._\n\
         \n\
         ## Captured tool-call sequence\n\
         \n\
         Models observed: {models}\n\
         Last finish_reason: `{finish}`\n\
         \n\
         {tools}\n\
         ## Success criterion\n\
         \n\
         _TODO: replace this stub with a verifier (e.g., \"file at path X\n\
         exists and contains the requested sections\")._\n\
         \n\
         ## Failure modes\n\
         \n\
         _TODO: what should the agent NOT do? Common: hallucinated success,\n\
         path drift, skipping the verify step._\n",
        skill_id = args.skill_id,
        title = title,
        conv = args.request_id,
        now = chrono::Utc::now().to_rfc3339(),
        models = models_line,
        finish = finish_line,
        tools = tool_section,
    );

    let out_dir = std::path::PathBuf::from(&args.out).join(&args.skill_id);
    std::fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join("SKILL.md");
    std::fs::write(&out_path, draft)?;
    println!("Wrote distilled skill draft → {}", out_path.display());
    println!("Review and edit the TODO sections, then `mv` it under ecosystem/skills/ to publish.");
    Ok(())
}

async fn handle_list(args: SkillListArgs) -> anyhow::Result<()> {
    let server = server_url(args.server.as_deref());
    let resp: RegistryResponse =
        get_json(&server, "/api/v1/skills/registry")
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "❌ Could not fetch skill registry: {} (is the server running? \
                 check CYBERCLAW_SERVER, or run `cyberclaw chat` first to log in)",
                    e
                )
            })?;

    match args.format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "bundles": resp.bundles.iter().map(|b| serde_json::json!({
                        "skill_id": b.skill_id,
                        "name": b.name,
                        "category": b.category,
                        "source": b.source,
                        "description": b.description,
                        "installed_at": b.installed_at,
                    })).collect::<Vec<_>>(),
                    "total": resp.total,
                }))?
            );
        }
        OutputFormat::Text => {
            if resp.bundles.is_empty() {
                println!("No skills registered.");
            } else {
                println!("Total skills: {}\n", resp.total);
                for b in &resp.bundles {
                    println!("Name:        {}", b.name);
                    if !b.skill_id.is_empty() && b.skill_id != b.name {
                        println!("ID:          {}", b.skill_id);
                    }
                    if !b.category.is_empty() {
                        println!("Category:    {}", b.category);
                    }
                    if !b.source.is_empty() {
                        println!("Source:      {}", b.source);
                    }
                    if !b.description.is_empty() {
                        // Truncate long descriptions to keep the list readable.
                        let preview: String = b.description.chars().take(120).collect();
                        let suffix = if b.description.chars().count() > 120 {
                            "…"
                        } else {
                            ""
                        };
                        println!("Description: {}{}", preview, suffix);
                    }
                    if !b.installed_at.is_empty() {
                        println!("Installed at: {}", b.installed_at);
                    }
                    println!("---");
                }
            }
        }
    }

    Ok(())
}
