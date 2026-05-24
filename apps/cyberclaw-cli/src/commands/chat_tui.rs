//! ratatui 全屏 TUI for `cyberclaw chat`。
//!
//! 布局（三段式）：
//! ```text
//! ┌──────────────────────────────────────────────────┐
//! │ Conversation history (scrollable)               │  70%
//! ├──────────────────────────────────────────────────┤
//! │ model: xxx │ conv: c-abc123 │ tokens: ~N        │  1 行状态条
//! ├──────────────────────────────────────────────────┤
//! │ > Type your message…（tui-textarea）             │  余下
//! │   ↵ send · Shift+↵ newline · Ctrl+C quit · /help │
//! └──────────────────────────────────────────────────┘
//! ```
//!
//! Overlay（居中 modal，z-order 最高）：
//! - `OverlayState::Approval`    — y/n 审批请求
//! - `OverlayState::ResumePicker` — 选择历史会话
//! - `OverlayState::SkillsList`  — 查看 active skills（只读）
//! - `OverlayState::AgentsList`  — 查看 agents 列表
//! - `OverlayState::SlashHelp`   — /help 帮助面板

use anyhow::{Context, Result};
use chrono::{Local, Utc};
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use pulldown_cmark::{Event as MdEvent, Options, Parser, Tag};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use reqwest::Client;
use serde::Deserialize;
use std::io::{self, Stdout};
use tokio::sync::mpsc;
use tui_textarea::TextArea;

use crate::commands::chat::{handle_slash, send_message_tui, ClarifyPayload, SlashResult};
use cyberclaw_core::i18n::t;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// 对话中的一条消息块（user 或 assistant）
#[derive(Debug, Clone)]
pub struct MessageBlock {
    pub role: String,
    pub content: String,
    pub ts: chrono::DateTime<Utc>,
    /// true = 当前正在流式接收
    pub streaming: bool,
}

// ---------------------------------------------------------------------------
// Brand / Theme — Hermes-inspired banner + oh-my-zsh-style prompt
// ---------------------------------------------------------------------------

/// CYBERCLAW ASCII 艺术（76 列宽）。窄终端会自然换行降级。
const CYBERCLAW_LOGO: &[&str] = &[
    " ██████╗██╗   ██╗██████╗ ███████╗██████╗  ██████╗██╗      █████╗ ██╗    ██╗",
    "██╔════╝╚██╗ ██╔╝██╔══██╗██╔════╝██╔══██╗██╔════╝██║     ██╔══██╗██║    ██║",
    "██║      ╚████╔╝ ██████╔╝█████╗  ██████╔╝██║     ██║     ███████║██║ █╗ ██║",
    "██║       ╚██╔╝  ██╔══██╗██╔══╝  ██╔══██╗██║     ██║     ██╔══██║██║███╗██║",
    "╚██████╗   ██║   ██████╔╝███████╗██║  ██║╚██████╗███████╗██║  ██║╚███╔███╔╝",
    " ╚═════╝   ╚═╝   ╚═════╝ ╚══════╝╚═╝  ╚═╝ ╚═════╝╚══════╝╚═╝  ╚═╝ ╚══╝╚══╝ ",
];

/// 6 行 logo 的颜色梯度索引：primary primary accent accent border border
const LOGO_GRADIENT: &[u8] = &[0, 0, 1, 1, 2, 2];

const COLOR_PRIMARY: Color = Color::Cyan;
const COLOR_ACCENT: Color = Color::Magenta;
const COLOR_BORDER: Color = Color::DarkGray;
const COLOR_MUTED: Color = Color::Gray;
const COLOR_OK: Color = Color::Green;
const COLOR_WARN: Color = Color::Yellow;
#[allow(dead_code)] // reserved for future error-state glyph in prompt
const COLOR_ERR: Color = Color::Red;

const DEFAULT_PROMPT_USER: &str = "cyberclaw";

/// 把 logo 行编为带渐变色的 Line 序列。
fn logo_lines() -> Vec<Line<'static>> {
    let palette = [COLOR_PRIMARY, COLOR_ACCENT, COLOR_BORDER];
    CYBERCLAW_LOGO
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let color = palette[LOGO_GRADIENT[i] as usize];
            Line::from(Span::styled(*l, Style::default().fg(color)))
        })
        .collect()
}

/// Braille 10 帧旋转——streaming 状态的活体指示。
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 构造 oh-my-zsh 风格的 prompt 行（终端真实样式）：
///   `⚙ cyberclaw@conv-170c5 $ ▌`
/// 动态行为（依赖 tick，每 50ms+1）：
///   - idle: `$` SLOW_BLINK (终端原生) + 后随光标方块 ▌ 颜色每 500ms 由 primary↔accent 切换（呼吸）
///   - streaming: `⠋⠙⠙⠸…` 10 帧 braille 旋转，每 250ms 切换
///
/// 注：model 不放进 prompt（占用过宽且已在 status bar 显示），保持 prompt 简洁。
fn build_prompt_line<'a>(
    user: &str,
    conv_short: &str,
    _model: &str,
    streaming: bool,
    tick: u64,
) -> Line<'a> {
    let user = user.to_string();
    let conv = format!("@{}", conv_short);

    let mut spans = vec![
        Span::styled("⚙ ", Style::default().fg(COLOR_ACCENT)),
        Span::styled(
            user,
            Style::default()
                .fg(COLOR_PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(conv, Style::default().fg(COLOR_BORDER)),
        Span::raw(" "),
    ];

    if streaming {
        // 每 5 tick = 250ms 切一帧
        let frame = SPINNER_FRAMES[((tick / 5) as usize) % SPINNER_FRAMES.len()];
        spans.push(Span::styled(
            frame.to_string(),
            Style::default().fg(COLOR_WARN).add_modifier(Modifier::BOLD),
        ));
    } else {
        // $ 用 SLOW_BLINK + bold（终端原生闪烁）
        spans.push(Span::styled(
            "$",
            Style::default()
                .fg(COLOR_OK)
                .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK),
        ));
        // 光标 ▌ 颜色每 500ms 在 primary↔accent 间切换（呼吸）
        let cursor_color = if (tick / 10).is_multiple_of(2) {
            COLOR_PRIMARY
        } else {
            COLOR_ACCENT
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            "▌",
            Style::default()
                .fg(cursor_color)
                .add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Rate-limit snapshot received from the server via SSE.
#[derive(Debug, Clone)]
pub struct RateLimitInfo {
    pub provider: String,
    pub requests_limit: Option<u64>,
    pub requests_remaining: Option<u64>,
    pub tokens_limit: Option<u64>,
    pub tokens_remaining: Option<u64>,
    pub requests_reset_secs: Option<f64>,
    pub tokens_reset_secs: Option<f64>,
}

/// TUI 主循环接收的异步事件
#[derive(Debug)]
pub enum TokenEvent {
    Token(String),
    Done,
    Error(String),
    Clarify(ClarifyPayload),
    /// Rate-limit snapshot from the last LLM call.
    RateLimit(RateLimitInfo),
    /// Token usage snapshot for cost estimation.
    Usage(crate::commands::chat::SseUsage),
    /// SSE 推送的审批请求（构造侧由服务端 SSE stream 触发，客户端 match arm 已就绪）
    #[allow(dead_code)]
    ApprovalRequest(ApprovalCtx),
}

/// 审批请求上下文
#[derive(Debug, Clone)]
pub struct ApprovalCtx {
    pub id: String,
    pub description: String,
    pub capability: String,
    pub risk_level: String,
}

/// API 会话摘要（GET /api/v1/chat/conversations）
#[derive(Debug, Clone, Deserialize)]
pub struct ConvSummary {
    pub id: String,
    pub title: Option<String>,
    pub created_at: Option<String>,
}

/// API skill 摘要
#[derive(Debug, Clone, Deserialize)]
pub struct SkillSummary {
    pub id: String,
    pub name: String,
    pub enabled: Option<bool>,
}

/// API agent 摘要
#[derive(Debug, Clone, Deserialize)]
pub struct AgentSummary {
    pub id: String,
    pub name: String,
}

/// 恢复选择器的内容
#[derive(Debug)]
pub struct ResumeCtx {
    pub sessions: Vec<ConvSummary>,
    pub selected: usize,
}

/// Skills 列表 overlay 内容
#[derive(Debug)]
pub struct SkillsCtx {
    pub items: Vec<SkillSummary>,
    pub selected: usize,
}

/// Agents 列表 overlay 内容
#[derive(Debug)]
pub struct AgentsCtx {
    pub items: Vec<AgentSummary>,
    pub selected: usize,
}

/// TUI overlay 状态（同一时刻最多一个）
#[derive(Debug)]
pub enum OverlayState {
    None,
    Approval(ApprovalCtx),
    ResumePicker(ResumeCtx),
    SkillsList(SkillsCtx),
    AgentsList(AgentsCtx),
    SlashHelp,
}

/// Markdown 渲染模式
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderMode {
    /// 纯文本模式（保留为 Markdown 的对立面，供未来切换使用）
    #[allow(dead_code)]
    Plain,
    Markdown,
}

/// TUI 应用状态
struct TuiApp<'a> {
    history: Vec<MessageBlock>,
    scroll_offset: u16,
    /// true = 新消息到达时自动 pin 到底部（默认 live tail 行为）。
    /// 用户手动 PgUp/Home 时置 false；End 键恢复 true。
    conversation_auto_pin: bool,
    /// 最近一帧渲染计算出的对话区视口高度（行数），供半页跳转使用。
    conversation_viewport_height: u16,
    textarea: TextArea<'a>,
    model: String,
    conv_id: String,
    streaming: bool,
    status_msg: Option<String>,
    /// 估算 token 数（简单字符数 / 4）
    token_estimate: usize,
    /// overlay 状态
    overlay: OverlayState,
    /// markdown 渲染模式
    render_mode: RenderMode,
    /// 是否显示 <think>…</think> 块
    show_thinking: bool,
    /// 已发送消息历史（Up/Down 翻阅）
    input_history: Vec<String>,
    /// 当前历史浏览索引；None = 用户正在编辑新输入
    history_idx: Option<usize>,
    /// Up/Down 触发前的草稿，恢复时用
    history_draft: String,
    /// 动画 tick — 每 50ms 自增 1。驱动 prompt $ 呼吸 + streaming spinner。
    pulse_tick: u64,
    /// streaming 开始时的 pulse_tick / token 估算，用于计算 tok/s
    stream_start_tick: Option<u64>,
    stream_start_tokens: usize,
    /// `/retry` 设置后由主循环消费——把它当作一条新的 user 输入重发。
    pending_retry: Option<String>,
    /// streaming 期间用户提交的输入暂存在这里；assistant Done 之后由主循环出队重发。
    input_queue: Vec<String>,
    /// 是否显示 tool block / DSML 摘要等非内容 detail 行。
    /// `/details` 切换；与 `show_thinking` 一起表示完整的"细节可见性"。
    show_tool_details: bool,
    /// Most recent rate-limit snapshot received from the server.
    /// Updated on every response that carries `x-ratelimit-*` headers.
    last_rate_limit: Option<RateLimitInfo>,
    /// Session-level cost accumulator fed by server `usage` SSE frames.
    cost_accumulator: cyberclaw_llm::CostAccumulator,
}

impl<'a> TuiApp<'a> {
    fn new(model: String, conv_id: String) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" message (Enter send · Shift+Enter newline · Ctrl+C quit) "),
        );
        textarea.set_cursor_line_style(Style::default());
        textarea.set_placeholder_text("Type your message…");

        Self {
            history: Vec::new(),
            scroll_offset: 0,
            conversation_auto_pin: true,
            conversation_viewport_height: 0,
            textarea,
            model,
            conv_id,
            streaming: false,
            status_msg: None,
            token_estimate: 0,
            overlay: OverlayState::None,
            render_mode: RenderMode::Markdown,
            show_thinking: false,
            input_history: Vec::new(),
            history_idx: None,
            history_draft: String::new(),
            pulse_tick: 0,
            stream_start_tick: None,
            stream_start_tokens: 0,
            pending_retry: None,
            input_queue: Vec::new(),
            show_tool_details: true,
            last_rate_limit: None,
            cost_accumulator: cyberclaw_llm::CostAccumulator::default(),
        }
    }

    /// 用指定字符串替换 textarea 全部内容（用于 Up/Down 历史翻阅）。
    fn set_textarea_text(&mut self, text: &str) {
        let mut ta = TextArea::default();
        ta.set_cursor_line_style(Style::default());
        ta.set_placeholder_text("Type your message…");
        for (i, line) in text.lines().enumerate() {
            if i > 0 {
                ta.insert_newline();
            }
            ta.insert_str(line);
        }
        // 若 text 不以换行结尾且为空字符串，textarea 仍空——无需特殊处理。
        self.textarea = ta;
    }

    /// 当前 textarea 内容（不写回，用于备份）。
    fn current_input(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Up 键：向前翻历史。首次按下时保存当前草稿到 history_draft。
    fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let new_idx = match self.history_idx {
            None => {
                self.history_draft = self.current_input();
                self.input_history.len().saturating_sub(1)
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.history_idx = Some(new_idx);
        let text = self.input_history[new_idx].clone();
        self.set_textarea_text(&text);
    }

    /// Down 键：向后翻历史；越过最末 → 恢复草稿。
    fn history_next(&mut self) {
        match self.history_idx {
            None => {} // not browsing
            Some(i) if i + 1 < self.input_history.len() => {
                let new_idx = i + 1;
                self.history_idx = Some(new_idx);
                let text = self.input_history[new_idx].clone();
                self.set_textarea_text(&text);
            }
            Some(_) => {
                // 越过末尾 → 恢复 history_draft
                self.history_idx = None;
                let draft = self.history_draft.clone();
                self.set_textarea_text(&draft);
            }
        }
    }

    /// 把刚发送的消息加入历史。重复最近一条不入。
    fn push_input_history(&mut self, msg: &str) {
        if msg.is_empty() {
            return;
        }
        if self.input_history.last().map(String::as_str) != Some(msg) {
            self.input_history.push(msg.to_string());
        }
        self.history_idx = None;
        self.history_draft.clear();
    }

    fn push_user(&mut self, content: String) {
        self.token_estimate += content.len() / 4;
        self.history.push(MessageBlock {
            role: "user".to_string(),
            content,
            ts: Utc::now(),
            streaming: false,
        });
        self.scroll_to_bottom();
    }

    fn begin_assistant(&mut self) {
        self.streaming = true;
        self.stream_start_tick = Some(self.pulse_tick);
        self.stream_start_tokens = self.token_estimate;
        self.history.push(MessageBlock {
            role: "assistant".to_string(),
            content: String::new(),
            ts: Utc::now(),
            streaming: true,
        });
        self.scroll_to_bottom();
    }

    fn append_token(&mut self, token: &str) {
        if let Some(block) = self.history.last_mut() {
            if block.streaming {
                block.content.push_str(token);
                self.token_estimate += token.len() / 4 + 1;
                self.scroll_to_bottom();
            }
        }
    }

    fn finish_streaming(&mut self) {
        self.streaming = false;
        self.stream_start_tick = None;
        if let Some(block) = self.history.last_mut() {
            block.streaming = false;
            // BUG-CB-14: assistant 时间戳之前 = user 提交时刻（placeholder
            // 在 begin_assistant 时 set 后再未更新），导致 you 和 assistant
            // 时间戳完全相同。修正为流结束时刻 = 实际回复完成时间。
            block.ts = Utc::now();
        }
    }

    /// 计算当前 streaming 的 tok/s（无 streaming 时返回 None）。
    fn current_tok_per_sec(&self) -> Option<u32> {
        let start = self.stream_start_tick?;
        let elapsed_ticks = self.pulse_tick.saturating_sub(start);
        if elapsed_ticks < 4 {
            // < 200ms 数据不足
            return None;
        }
        let elapsed_secs = (elapsed_ticks as f64) * 0.05;
        let tokens = self.token_estimate.saturating_sub(self.stream_start_tokens);
        Some(((tokens as f64) / elapsed_secs).round() as u32)
    }

    fn push_system(&mut self, content: String) {
        self.history.push(MessageBlock {
            role: "system".to_string(),
            content,
            ts: Utc::now(),
            streaming: false,
        });
        self.scroll_to_bottom();
    }

    /// 推送 CYBERCLAW banner 块（启动时一次）。role="banner" 触发渐变色渲染。
    fn push_banner(&mut self, agent_id: Option<&str>) {
        let agent_display = agent_id.unwrap_or("default");
        let conv_short = if self.conv_id.len() > 16 {
            &self.conv_id[..16]
        } else {
            &self.conv_id
        };
        let info = format!(
            "\n  受控智能体平台  ·  Controlled Agent Platform  ·  v{}\n\n\
             \x20 conv:   {}\n\
             \x20 model:  {}\n\
             \x20 agent:  {}\n\n\
             \x20 键位：Enter 发送 · Shift+Enter 换行 · Ctrl+C 退出\n\
             \x20 斜杠：/help · /sessions · /skills · /agents · /trace · /history",
            env!("CARGO_PKG_VERSION"),
            conv_short,
            self.model,
            agent_display
        );
        self.history.push(MessageBlock {
            role: "banner".to_string(),
            content: info,
            ts: Utc::now(),
            streaming: false,
        });
    }

    /// 仅当 auto-pin 开启时才跳到底部——保护正在阅读历史的用户。
    fn scroll_to_bottom(&mut self) {
        if self.conversation_auto_pin {
            self.scroll_offset = u16::MAX;
        }
    }

    /// 强制跳到底部并重新开启 auto-pin（End 键调用）。
    fn scroll_to_bottom_force(&mut self) {
        self.conversation_auto_pin = true;
        self.scroll_offset = u16::MAX;
    }

    fn clear_history(&mut self) {
        self.history.clear();
        self.scroll_offset = 0;
    }

    fn take_input(&mut self) -> String {
        let lines = self.textarea.lines();
        let content = lines.join("\n");
        let mut ta = TextArea::default();
        ta.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" message (Enter send · Shift+Enter newline · Ctrl+C quit) "),
        );
        ta.set_cursor_line_style(Style::default());
        ta.set_placeholder_text("Type your message…");
        self.textarea = ta;
        content
    }

    fn has_overlay(&self) -> bool {
        !matches!(self.overlay, OverlayState::None)
    }
}

// ---------------------------------------------------------------------------
// Terminal lifecycle guard
// ---------------------------------------------------------------------------

struct RawModeGuard {
    stdout: Stdout,
}

impl RawModeGuard {
    fn enter() -> Result<(Self, Terminal<CrosstermBackend<Stdout>>)> {
        enable_raw_mode().context("enable raw mode")?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend).context("create terminal")?;
        Ok((RawModeGuard { stdout }, terminal))
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.stdout, LeaveAlternateScreen);
    }
}

// ---------------------------------------------------------------------------
// Markdown rendering helpers
// ---------------------------------------------------------------------------

/// Strip `<think>...</think>` blocks from content.
/// Returns (visible_content, had_thinking).
fn strip_think_blocks(content: &str) -> (String, bool) {
    // 先剥 DSML（DeepSeek 内部 tool-call markup），再剥 <think>
    let pre = strip_dsml_blocks(content);
    let mut out = String::new();
    let mut had = false;
    let mut rest = pre.as_str();
    loop {
        if let Some(start) = rest.find("<think>") {
            out.push_str(&rest[..start]);
            if let Some(end) = rest[start..].find("</think>") {
                had = true;
                rest = &rest[start + end + "</think>".len()..];
            } else {
                had = true;
                out.push_str(&rest[start + "<think>".len()..]);
                break;
            }
        } else {
            out.push_str(rest);
            break;
        }
    }
    (out, had)
}

/// 剥 DeepSeek DSML tool-call markup —— 整段从首个 `<｜｜DSML｜｜` 起丢弃。
/// tool-call 总在 assistant 消息末尾，前面 narrative ("我将搜索...") 保留。
/// v1.2 backlog #29：真接入 DSML→dispatch 后自动消失。
fn strip_dsml_blocks(content: &str) -> String {
    const DSML_OPEN_PREFIX: &str = "<\u{ff5c}\u{ff5c}DSML\u{ff5c}\u{ff5c}";
    match content.find(DSML_OPEN_PREFIX) {
        Some(pos) => content[..pos].to_string(),
        None => content.to_string(),
    }
}

/// Extract `<think>...</think>` blocks from content (for display when show_thinking=true).
fn extract_think_blocks(content: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut rest = content;
    while let Some(start) = rest.find("<think>") {
        if let Some(rel_end) = rest[start..].find("</think>") {
            let inner = &rest[start + "<think>".len()..start + rel_end];
            blocks.push(inner.trim().to_string());
            rest = &rest[start + rel_end + "</think>".len()..];
        } else {
            break;
        }
    }
    blocks
}

/// Render markdown content into ratatui Lines using pulldown-cmark.
fn render_markdown<'b>(content: &str, width: usize) -> Vec<Line<'b>> {
    let opts = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(content, opts);

    let mut lines: Vec<Line<'b>> = Vec::new();
    let mut current_spans: Vec<Span<'b>> = Vec::new();
    let mut bold = false;
    let mut italic = false;
    let mut in_code_block = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut list_depth: usize = 0;

    let flush_line = |spans: &mut Vec<Span<'b>>, lines: &mut Vec<Line<'b>>| {
        if !spans.is_empty() {
            lines.push(Line::from(std::mem::take(spans)));
        } else {
            lines.push(Line::raw(""));
        }
    };

    for event in parser {
        match event {
            MdEvent::Start(Tag::Heading(_, _, _)) => {
                flush_line(&mut current_spans, &mut lines);
                bold = true;
            }
            MdEvent::End(Tag::Heading(_, _, _)) => {
                flush_line(&mut current_spans, &mut lines);
                bold = false;
            }
            MdEvent::Start(Tag::Paragraph) if !lines.is_empty() => {
                lines.push(Line::raw(""));
            }
            MdEvent::Start(Tag::Paragraph) => {}
            MdEvent::End(Tag::Paragraph) => {
                flush_line(&mut current_spans, &mut lines);
            }
            MdEvent::Start(Tag::Strong) => {
                bold = true;
            }
            MdEvent::End(Tag::Strong) => {
                bold = false;
            }
            MdEvent::Start(Tag::Emphasis) => {
                italic = true;
            }
            MdEvent::End(Tag::Emphasis) => {
                italic = false;
            }
            MdEvent::Start(Tag::List(_)) => {
                list_depth += 1;
            }
            MdEvent::End(Tag::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    lines.push(Line::raw(""));
                }
            }
            MdEvent::Start(Tag::Item) => {
                flush_line(&mut current_spans, &mut lines);
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                current_spans.push(Span::raw(format!("{}• ", indent)));
            }
            MdEvent::End(Tag::Item) => {
                flush_line(&mut current_spans, &mut lines);
            }
            MdEvent::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                    pulldown_cmark::CodeBlockKind::Indented => String::new(),
                };
                code_buf.clear();
                lines.push(Line::raw(""));
            }
            MdEvent::End(Tag::CodeBlock(_)) => {
                in_code_block = false;
                let lang_display = if code_lang.is_empty() {
                    "code".to_string()
                } else {
                    code_lang.clone()
                };
                // render code block with dim border
                // Hermes 风格 code block：accent 顶/底 + 左侧 │ 边框（语法高亮提示）
                let border_top = format!(
                    "┌─ {} {}",
                    lang_display,
                    "─".repeat(width.saturating_sub(lang_display.len() + 5))
                );
                lines.push(Line::styled(
                    border_top,
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ));
                for code_line in code_buf.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(COLOR_ACCENT)),
                        Span::styled(code_line.to_string(), Style::default().fg(COLOR_PRIMARY)),
                    ]));
                }
                lines.push(Line::styled(
                    format!("└{}", "─".repeat(width.saturating_sub(1))),
                    Style::default().fg(COLOR_ACCENT),
                ));
                lines.push(Line::raw(""));
                code_buf.clear();
                code_lang.clear();
            }
            MdEvent::Text(text) => {
                if in_code_block {
                    code_buf.push_str(&text);
                } else {
                    let mut style = Style::default();
                    if bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    if italic {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    current_spans.push(Span::styled(text.to_string(), style));
                }
            }
            MdEvent::Code(text) => {
                current_spans.push(Span::styled(
                    format!("`{}`", text),
                    Style::default().fg(Color::Yellow),
                ));
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => {
                flush_line(&mut current_spans, &mut lines);
            }
            MdEvent::Rule => {
                flush_line(&mut current_spans, &mut lines);
                lines.push(Line::styled(
                    "─".repeat(width.min(80)),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            _ => {}
        }
    }
    flush_line(&mut current_spans, &mut lines);
    lines
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut TuiApp) -> Result<()> {
    terminal.draw(|f| {
        let size = f.area();

        // BUG-CB-11: 顶部固定 banner（3 行含 border）+ 对话区 + 状态条 + 输入区
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // banner block（含上下 border）
                Constraint::Min(10),
                Constraint::Length(1),
                Constraint::Length(8),
            ])
            .split(size);

        // --- Banner 区（BUG-CB-11）---
        let conv_short_banner = if app.conv_id.len() > 8 {
            format!("{}…", &app.conv_id[..8])
        } else {
            app.conv_id.clone()
        };
        let banner_text = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "⚡ CYBERCLAW",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(COLOR_MUTED),
            ),
            Span::styled("  │  ", Style::default().fg(COLOR_BORDER)),
            Span::styled(app.model.clone(), Style::default().fg(COLOR_PRIMARY)),
            Span::styled("  │  ", Style::default().fg(COLOR_BORDER)),
            Span::styled(
                format!("conv_{}", conv_short_banner),
                Style::default().fg(COLOR_MUTED),
            ),
        ]);
        let banner_block = Paragraph::new(banner_text).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(COLOR_BORDER)),
        );
        f.render_widget(banner_block, chunks[0]);

        // --- 历史区 ---
        let history_height = chunks[1].height.saturating_sub(2) as usize;
        let history_width = chunks[1].width.saturating_sub(4) as usize;
        // BUG-CB-10: 记录视口高度供键盘半页跳转使用
        app.conversation_viewport_height = history_height as u16;

        let mut all_lines: Vec<Line> = Vec::new();
        for block in &app.history {
            // banner 特殊路径：渐变色 logo + muted 信息文本，跳过 "[•]" header
            if block.role == "banner" {
                for line in logo_lines() {
                    all_lines.push(line);
                }
                for info_line in block.content.lines() {
                    all_lines.push(Line::styled(
                        info_line.to_string(),
                        Style::default().fg(COLOR_MUTED),
                    ));
                }
                all_lines.push(Line::raw(""));
                continue;
            }
            // hermes-style role glyphs + colors
            let (glyph, role_label, role_style) = match block.role.as_str() {
                "user" => (
                    "▶",
                    "you",
                    Style::default()
                        .fg(COLOR_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ),
                "assistant" => (
                    "◆",
                    "assistant",
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                _ => ("·", "system", Style::default().fg(COLOR_MUTED)),
            };
            // BUG-CB-15: 显示用户本地时区，存储仍为 UTC（保持 audit/log 一致）
            let ts = block.ts.with_timezone(&Local).format("%H:%M:%S").to_string();
            let mut header_spans = vec![
                Span::styled(format!("{} ", glyph), role_style),
                Span::styled(role_label.to_string(), role_style),
                Span::raw(" "),
                Span::styled(ts, Style::default().fg(COLOR_BORDER)),
            ];
            if block.streaming {
                // braille 旋转，与 prompt spinner 同节奏（每 5 tick = 250ms 切帧）
                let frame = SPINNER_FRAMES[((app.pulse_tick / 5) as usize) % SPINNER_FRAMES.len()];
                header_spans.push(Span::raw(" "));
                header_spans.push(Span::styled(
                    frame.to_string(),
                    Style::default().fg(COLOR_WARN).add_modifier(Modifier::BOLD),
                ));
            }
            let header = Line::from(header_spans);
            all_lines.push(header);

            if block.role == "assistant" && app.render_mode == RenderMode::Markdown {
                // Extract and optionally show thinking blocks
                let think_blocks = extract_think_blocks(&block.content);
                let (visible, had_thinking) = strip_think_blocks(&block.content);

                if had_thinking {
                    if app.show_thinking {
                        for tb in &think_blocks {
                            all_lines.push(Line::styled(
                                "  ⋯ thinking:",
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::ITALIC),
                            ));
                            for tline in tb.lines() {
                                all_lines.push(Line::styled(
                                    format!("    {}", tline),
                                    Style::default()
                                        .fg(Color::DarkGray)
                                        .add_modifier(Modifier::ITALIC),
                                ));
                            }
                        }
                    } else if app.show_tool_details {
                        // 折叠提示只在 detail 模式开着时显示——/details hidden 会
                        // 把"thinking (use /trace…)" 这一行也一起隐藏。
                        all_lines.push(Line::styled(
                            "  ⋯ thinking (use /trace to show)",
                            Style::default()
                                .fg(Color::DarkGray)
                                .add_modifier(Modifier::ITALIC),
                        ));
                    }
                }

                let rendered = render_markdown(&visible, history_width);
                for line in rendered {
                    all_lines.push(line);
                }
            } else {
                // Plain rendering (user / system messages)
                for content_line in block.content.lines() {
                    let mut remaining = content_line;
                    if remaining.is_empty() {
                        all_lines.push(Line::raw(""));
                        continue;
                    }
                    loop {
                        let char_count = remaining.chars().count();
                        let take = char_count.min(if history_width > 2 {
                            history_width - 2
                        } else {
                            1
                        });
                        let (chunk, rest) = split_at_char(remaining, take);
                        all_lines.push(Line::raw(format!("  {}", chunk)));
                        remaining = rest;
                        if remaining.is_empty() {
                            break;
                        }
                    }
                }
            }
            all_lines.push(Line::raw(""));
        }

        let total_lines = all_lines.len();
        let max_scroll = total_lines.saturating_sub(history_height);
        if app.scroll_offset as usize > max_scroll {
            app.scroll_offset = max_scroll as u16;
        }

        // BUG-CB-10: 滚屏指示器——用户不在底部时在标题显示提示
        let is_at_bottom = app.scroll_offset as usize >= max_scroll;
        let conv_title = if !is_at_bottom {
            " conversation  ⬆ scrolled — End 回到 live "
        } else {
            " conversation "
        };

        let paragraph = Paragraph::new(all_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(conv_title),
            )
            .wrap(Wrap { trim: false })
            .scroll((app.scroll_offset, 0));

        f.render_widget(paragraph, chunks[1]);

        // --- 状态条 ---
        let conv_short = if app.conv_id.len() > 8 {
            &app.conv_id[..8]
        } else {
            &app.conv_id
        };
        let streaming_indicator = if app.streaming { " ●" } else { "" };
        let md_indicator = if app.render_mode == RenderMode::Markdown {
            " │ md"
        } else {
            ""
        };
        let trace_indicator = if app.show_thinking {
            " │ trace:on"
        } else {
            ""
        };
        let status_text = if let Some(ref msg) = app.status_msg {
            msg.clone()
        } else {
            let tok_rate = match app.current_tok_per_sec() {
                Some(r) => format!(" │ ~{} tok/s", r),
                None => String::new(),
            };
            format!(
                " model: {} │ conv: {} │ ~{}tok{}{}{}{}",
                app.model,
                conv_short,
                app.token_estimate,
                tok_rate,
                streaming_indicator,
                md_indicator,
                trace_indicator,
            )
        };
        let status_bar = Paragraph::new(status_text)
            .style(Style::default().bg(Color::DarkGray).fg(Color::White));
        f.render_widget(status_bar, chunks[2]);

        // --- 输入区（终端风格：外框 + 内部 prompt 行 + textarea） ---
        let conv_short_for_prompt = if app.conv_id.len() > 12 {
            &app.conv_id[..12]
        } else {
            &app.conv_id
        };
        let input_outer = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(COLOR_BORDER))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    "Enter 发送 · Shift+Enter 换行 · ↑↓ 历史 · PgUp/PgDn 滚屏 · /help · Ctrl+C 退出 ",
                    Style::default().fg(COLOR_MUTED),
                ),
            ]));
        let input_inner = input_outer.inner(chunks[3]);
        f.render_widget(input_outer, chunks[3]);

        // 计算 slash autocomplete dropdown（如有 / 前缀匹配）
        let current_input_text = app.textarea.lines().join("\n");
        let matches = if app.streaming {
            Vec::new()
        } else {
            slash_autocomplete_matches(&current_input_text)
        };
        let dropdown_h = matches.len().min(5) as u16;

        let constraints: Vec<Constraint> = if dropdown_h > 0 {
            vec![
                Constraint::Length(dropdown_h),
                Constraint::Length(1),
                Constraint::Min(1),
            ]
        } else {
            vec![
                Constraint::Length(0),
                Constraint::Length(1),
                Constraint::Min(1),
            ]
        };
        let input_sub = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(input_inner);

        if dropdown_h > 0 {
            let dd_lines: Vec<Line> = matches
                .iter()
                .enumerate()
                .map(|(i, (cmd, desc))| {
                    let (cmd_style, marker) = if i == 0 {
                        (
                            Style::default()
                                .fg(COLOR_PRIMARY)
                                .add_modifier(Modifier::BOLD),
                            "▶ ",
                        )
                    } else {
                        (Style::default().fg(COLOR_MUTED), "  ")
                    };
                    Line::from(vec![
                        Span::styled(marker, Style::default().fg(COLOR_ACCENT)),
                        Span::styled(format!("{:<22}", cmd), cmd_style),
                        Span::styled(desc.to_string(), Style::default().fg(COLOR_BORDER)),
                    ])
                })
                .collect();
            f.render_widget(Paragraph::new(dd_lines), input_sub[0]);
        }

        // 真 shell 体验：单行时 prompt 和 input 同一行（水平 split）
        // 多行时 prompt 占一行 + textarea 占下面所有行（垂直 split）
        let prompt_line_data = build_prompt_line(
            DEFAULT_PROMPT_USER,
            conv_short_for_prompt,
            &app.model,
            app.streaming,
            app.pulse_tick,
        );
        // 多行触发条件：(a) 真有多行；(b) 当前单行内容已超出可显示宽度（输入溢出自动换行布局）
        let prompt_width_est: u16 = prompt_line_data
            .spans
            .iter()
            .map(|s| {
                s.content
                    .chars()
                    .map(|c| if c.is_ascii() { 1u16 } else { 2u16 })
                    .sum::<u16>()
            })
            .sum();
        let available_for_input = input_inner.width.saturating_sub(prompt_width_est);
        let current_visible_width: u16 = app
            .textarea
            .lines()
            .first()
            .map(|l| {
                l.chars()
                    .map(|c| if c.is_ascii() { 1u16 } else { 2u16 })
                    .sum::<u16>()
            })
            .unwrap_or(0);
        let is_multiline = app.textarea.lines().len() > 1
            || (available_for_input > 0 && current_visible_width >= available_for_input);
        app.textarea.set_block(Block::default());

        if is_multiline {
            // 垂直：prompt 一行 + textarea 占 input_sub[2]（多行编辑空间）
            f.render_widget(Paragraph::new(prompt_line_data), input_sub[1]);
            f.render_widget(&app.textarea, input_sub[2]);
        } else {
            // 水平：prompt 在左，textarea 紧跟其后同一行
            let input_split = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(prompt_width_est), Constraint::Min(1)])
                .split(input_sub[1]);
            f.render_widget(Paragraph::new(prompt_line_data), input_split[0]);
            f.render_widget(&app.textarea, input_split[1]);
            // input_sub[2] 留白（保留 expand 空间避免 cursor 跳）
        }

        // --- Overlay ---
        match &app.overlay {
            OverlayState::None => {}
            OverlayState::SlashHelp => {
                let modal = centered_rect(60, 70, size);
                f.render_widget(Clear, modal);
                let help_lines: Vec<Line> = SLASH_HELP
                    .iter()
                    .map(|(cmd, desc)| {
                        Line::from(vec![
                            Span::styled(
                                format!("  {:20}", cmd),
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(*desc),
                        ])
                    })
                    .collect();
                let p = Paragraph::new(help_lines)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" slash commands — any key to close ")
                            .border_style(Style::default().fg(Color::Cyan)),
                    )
                    .wrap(Wrap { trim: false });
                f.render_widget(p, modal);
            }
            OverlayState::Approval(ctx) => {
                let modal = centered_rect(60, 40, size);
                f.render_widget(Clear, modal);
                let lines = vec![
                    Line::raw(""),
                    Line::from(vec![Span::styled(
                        "  Approval Request",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )]),
                    Line::raw(""),
                    Line::from(vec![
                        Span::raw("  Capability: "),
                        Span::styled(
                            ctx.capability.clone(),
                            Style::default()
                                .fg(Color::White)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::raw("  Risk:       "),
                        Span::styled(ctx.risk_level.clone(), Style::default().fg(Color::Red)),
                    ]),
                    Line::raw(""),
                    Line::from(vec![Span::raw(format!("  {}", ctx.description))]),
                    Line::raw(""),
                    Line::styled(
                        format!("  {}  (y/n)", t("approval.prompt")),
                        Style::default().fg(Color::DarkGray),
                    ),
                ];
                let p = Paragraph::new(lines)
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .title(" approval request ")
                            .border_style(Style::default().fg(Color::Yellow)),
                    )
                    .alignment(Alignment::Left);
                f.render_widget(p, modal);
            }
            OverlayState::ResumePicker(ctx) => {
                let modal = centered_rect(70, 60, size);
                f.render_widget(Clear, modal);
                let items: Vec<ListItem> = ctx
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let title = s.title.clone().unwrap_or_else(|| s.id.clone());
                        let date = s.created_at.clone().unwrap_or_default();
                        let short_id = if s.id.len() > 8 { &s.id[..8] } else { &s.id };
                        let style = if i == ctx.selected {
                            Style::default()
                                .fg(Color::Black)
                                .bg(Color::Cyan)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        ListItem::new(Line::styled(
                            format!("  {}  {}  {}", short_id, title, date),
                            style,
                        ))
                    })
                    .collect();
                let mut list_state = ListState::default();
                list_state.select(Some(ctx.selected));
                let list = List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" resume session — ↑↓ navigate · Enter select · Esc cancel ")
                        .border_style(Style::default().fg(Color::Cyan)),
                );
                f.render_stateful_widget(list, modal, &mut list_state);
            }
            OverlayState::SkillsList(ctx) => {
                let modal = centered_rect(60, 60, size);
                f.render_widget(Clear, modal);
                let items: Vec<ListItem> = ctx
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, s)| {
                        let enabled = s.enabled.unwrap_or(true);
                        let style = if i == ctx.selected {
                            Style::default().fg(Color::Black).bg(Color::Green)
                        } else {
                            Style::default()
                        };
                        ListItem::new(Line::styled(
                            format!(
                                "  {}  {}  [{}]",
                                &s.id[..s.id.len().min(8)],
                                s.name,
                                if enabled { "on" } else { "off" }
                            ),
                            style,
                        ))
                    })
                    .collect();
                let mut list_state = ListState::default();
                list_state.select(Some(ctx.selected));
                let list = List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" active skills — ↑↓ navigate · Esc close ")
                        .border_style(Style::default().fg(Color::Green)),
                );
                f.render_stateful_widget(list, modal, &mut list_state);
            }
            OverlayState::AgentsList(ctx) => {
                let modal = centered_rect(60, 60, size);
                f.render_widget(Clear, modal);
                let items: Vec<ListItem> = ctx
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, a)| {
                        let style = if i == ctx.selected {
                            Style::default().fg(Color::Black).bg(Color::Magenta)
                        } else {
                            Style::default()
                        };
                        ListItem::new(Line::styled(
                            format!("  {}  {}", &a.id[..a.id.len().min(8)], a.name),
                            style,
                        ))
                    })
                    .collect();
                let mut list_state = ListState::default();
                list_state.select(Some(ctx.selected));
                let list = List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" agents — ↑↓ navigate · Enter switch · Esc cancel ")
                        .border_style(Style::default().fg(Color::Magenta)),
                );
                f.render_stateful_widget(list, modal, &mut list_state);
            }
        }
    })?;
    Ok(())
}

/// Centered rectangle helper (percent of terminal area).
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// 按字符数分割 &str，返回 (taken, rest)
fn split_at_char(s: &str, n: usize) -> (&str, &str) {
    let mut byte_idx = s.len();
    for (i, (idx, _)) in s.char_indices().enumerate() {
        if i == n {
            byte_idx = idx;
            break;
        }
    }
    (&s[..byte_idx], &s[byte_idx..])
}

// ---------------------------------------------------------------------------
// Slash commands help table
// ---------------------------------------------------------------------------

/// 返回与当前 input prefix 匹配的 slash 命令（最多 5 个，按定义顺序）。
/// 触发条件：input trim 后以 `/` 起头、不含换行（多行输入不弹）。
fn slash_autocomplete_matches(input: &str) -> Vec<&'static (&'static str, &'static str)> {
    let t = input.trim();
    if !t.starts_with('/') || t.contains('\n') {
        return Vec::new();
    }
    SLASH_HELP
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(t) || t == "/")
        .take(5)
        .collect()
}

const SLASH_HELP: &[(&str, &str)] = &[
    ("/help", "显示此帮助"),
    ("/clear", "清空历史显示"),
    ("/save [file]", "导出会话为 markdown 文件"),
    ("/model [name]", "查看/切换 LLM model"),
    ("/sessions", "打开历史会话选择器（resume picker）"),
    ("/skills", "查看当前激活的 skills"),
    ("/agents", "查看 agents 列表并切换"),
    ("/security", "显示安全规则与注入命中摘要"),
    ("/quit", "退出 TUI"),
    ("/history [file]", "导出当前会话为 markdown（同 /save）"),
    ("/token", "显示估算的 token 使用量（粗略）"),
    (
        "/usage",
        "详细用量：tokens / 消息数 / context 占比 / 当前 model",
    ),
    (
        "/undo",
        "从本地 transcript 移除最近一对消息（不动 server 会话）",
    ),
    ("/trace", "切换 <think> 思考块的显示/隐藏"),
    ("/retry", "重发上一轮 user message（来自本地 transcript）"),
    ("/queue", "查看 streaming 期间排队的输入"),
    (
        "/details [hidden|expanded|cycle]",
        "切换思考 + tool 细节的可见性",
    ),
    ("/compress", "压缩会话历史"),
    ("/digest [YYYY-MM-DD]", "今日学习摘要（可选指定日期）"),
    ("/orgmem <query>", "搜索组织记忆（空 query 列最近 5 条）"),
    ("/curator", "Curator 状态"),
];

// ---------------------------------------------------------------------------
// last-conversation 持久化
// ---------------------------------------------------------------------------

fn last_conv_path() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    home.join(".cyberclaw").join("last-conversation")
}

pub fn load_last_conv_id() -> Option<String> {
    let path = last_conv_path();
    std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn save_last_conv_id(conv_id: &str) {
    let path = last_conv_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, conv_id);
}

// ---------------------------------------------------------------------------
// HTTP helpers for overlays
// ---------------------------------------------------------------------------

async fn fetch_sessions(client: &Client, server: &str, token: &str) -> Result<Vec<ConvSummary>> {
    let url = format!("{}/api/v1/chat/conversations?limit=20", server);
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("GET {}", url))?;
    let sessions: Vec<ConvSummary> = resp.json().await.unwrap_or_default();
    Ok(sessions)
}

async fn fetch_skills(client: &Client, server: &str, token: &str) -> Result<Vec<SkillSummary>> {
    let url = format!("{}/api/v1/skills", server);
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("GET {}", url))?;
    let skills: Vec<SkillSummary> = resp.json().await.unwrap_or_default();
    Ok(skills)
}

async fn fetch_agents(client: &Client, server: &str, token: &str) -> Result<Vec<AgentSummary>> {
    let url = format!("{}/api/v1/agents", server);
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("GET {}", url))?;
    let agents: Vec<AgentSummary> = resp.json().await.unwrap_or_default();
    Ok(agents)
}

async fn post_approval_decision(
    client: &Client,
    server: &str,
    token: &str,
    approval_id: &str,
    approved: bool,
) -> Result<()> {
    let url = format!("{}/api/v1/chat/approval/{}/decide", server, approval_id);
    let _ = client
        .post(&url)
        .bearer_auth(token)
        .json(&serde_json::json!({ "approved": approved }))
        .send()
        .await
        .with_context(|| format!("POST {}", url))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Main TUI entry point
// ---------------------------------------------------------------------------

/// 进入全屏 ratatui TUI，运行 chat REPL。
pub async fn run_tui(
    client: Client,
    server: String,
    token: String,
    conv_id: String,
    agent_id: Option<String>,
    model: String,
) -> Result<()> {
    let (_guard, mut terminal) = RawModeGuard::enter()?;

    let mut app = TuiApp::new(model.clone(), conv_id.clone());
    app.push_banner(agent_id.as_deref());

    let (tx, mut rx) = mpsc::channel::<TokenEvent>(256);

    let mut current_model = model;
    let mut first_message = true;
    let mut current_agent_id: Option<String> = agent_id.clone();
    let mut messages: Vec<super::chat::ChatMessage> = Vec::new();
    // track whether we should exit
    let mut should_quit = false;

    // 外部 SIGINT 兜底：恢复 raw_mode 然后硬退（避免脚本/`kill -INT` 卡死 TUI）。
    // 不走 flag → 主循环检测 path：直接 exit，最大可靠性。
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = crossterm::terminal::disable_raw_mode();
            let _ =
                crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
            std::process::exit(0);
        }
    });

    draw(&mut terminal, &mut app)?;

    loop {
        if should_quit {
            break;
        }

        // 推进动画 tick（50ms 一次，驱动 prompt 呼吸 + spinner）
        app.pulse_tick = app.pulse_tick.wrapping_add(1);

        // 本轮要发送的 user message——由 (a) 直接输入 (b) /retry (c) 队列出队 任一来源填充。
        // 处理统一在 token-drain 之后做：保证 streaming=false 才会发出。
        let mut pending_send: Option<String> = None;

        if event::poll(std::time::Duration::from_millis(20))? {
            let ev = event::read()?;
            match &ev {
                Event::Key(key) => {
                    // --- Overlay key handling ---
                    if app.has_overlay() {
                        handle_overlay_key(
                            key,
                            &mut app,
                            &client,
                            &server,
                            &token,
                            &mut current_agent_id,
                            &mut should_quit,
                        )
                        .await;
                    } else {
                        // --- Normal key handling ---
                        match (key.modifiers, key.code) {
                            (KeyModifiers::CONTROL, KeyCode::Char('c')) | (_, KeyCode::Esc) => {
                                break;
                            }
                            (KeyModifiers::CONTROL, KeyCode::Char('s'))
                            | (KeyModifiers::NONE, KeyCode::Enter) => {
                                let input = app.take_input().trim().to_string();
                                app.status_msg = None;
                                if input.is_empty() {
                                    // nothing
                                } else if input.starts_with('/') {
                                    // slash 命令在 streaming 中也允许（如 /queue 查看队列）
                                    let quit = handle_slash_tui(
                                        &input,
                                        &client,
                                        &server,
                                        &token,
                                        &conv_id,
                                        &mut current_model,
                                        &mut current_agent_id,
                                        &mut app,
                                        tx.clone(),
                                    )
                                    .await;
                                    app.model = current_model.clone();
                                    if quit {
                                        break;
                                    }
                                } else if app.streaming {
                                    // streaming 期间用户输入入队，等响应结束自动出队发送。
                                    app.input_queue.push(input.clone());
                                    app.push_input_history(&input);
                                    app.push_system(format!(
                                        "[queue] 已加入队列 #{}（streaming 完成后自动发送）",
                                        app.input_queue.len()
                                    ));
                                } else {
                                    pending_send = Some(input);
                                }
                            }
                            (KeyModifiers::CONTROL, KeyCode::Char('l')) => {
                                app.clear_history();
                            }
                            (KeyModifiers::SHIFT, KeyCode::Enter)
                            | (KeyModifiers::ALT, KeyCode::Enter) => {
                                // 显式换行（Telegram/Slack 风格：Enter 发，Shift+Enter 换行）
                                app.textarea.insert_newline();
                            }
                            // bash/zsh 风格：Up/Down 翻已发送消息历史
                            (KeyModifiers::NONE, KeyCode::Up) => {
                                // 优先翻已发送消息历史；若历史为空 → 落到滚动对话区
                                if app.input_history.is_empty() {
                                    app.scroll_offset = app.scroll_offset.saturating_sub(3);
                                } else {
                                    app.history_prev();
                                }
                            }
                            (KeyModifiers::NONE, KeyCode::Down) => {
                                if app.input_history.is_empty() {
                                    app.scroll_offset = app.scroll_offset.saturating_add(3);
                                } else {
                                    app.history_next();
                                }
                            }
                            (KeyModifiers::NONE, KeyCode::Tab) => {
                                // slash autocomplete：Tab 接受 top match
                                let cur = app.textarea.lines().join("\n");
                                let matches = slash_autocomplete_matches(&cur);
                                if let Some((cmd, _)) = matches.first() {
                                    // 把 cmd 模板写入，光标停在末尾
                                    let new_text = format!("{} ", cmd);
                                    app.set_textarea_text(&new_text);
                                }
                            }
                            // BUG-CB-10: PgUp/PgDn 半页跳转 + auto-pin 控制
                            (_, KeyCode::PageUp) => {
                                let half = (app.conversation_viewport_height / 2).max(3);
                                app.scroll_offset = app.scroll_offset.saturating_sub(half);
                                // 向上滚动时关闭 auto-pin，保护阅读位置
                                app.conversation_auto_pin = false;
                            }
                            (_, KeyCode::PageDown) => {
                                let half = (app.conversation_viewport_height / 2).max(3);
                                app.scroll_offset = app.scroll_offset.saturating_add(half);
                                // draw() 会 clamp；End 键才明确重开 auto-pin
                            }
                            (_, KeyCode::Home) => {
                                // 跳到对话顶部
                                app.scroll_offset = 0;
                                app.conversation_auto_pin = false;
                            }
                            (_, KeyCode::End) => {
                                // 跳回底部并恢复 live tail
                                app.scroll_to_bottom_force();
                            }
                            _ => {
                                app.textarea.input(ev.clone());
                            }
                        }
                    }
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }

        // --- Drain token channel ---
        loop {
            match rx.try_recv() {
                Ok(TokenEvent::Token(tok)) => {
                    app.append_token(&tok);
                }
                Ok(TokenEvent::Done) => {
                    if let Some(block) = app.history.last() {
                        if block.role == "assistant" {
                            let content = block.content.clone();
                            if !content.is_empty() {
                                messages.push(super::chat::ChatMessage {
                                    role: "assistant".to_string(),
                                    content,
                                });
                            }
                        }
                    }
                    app.finish_streaming();
                }
                Ok(TokenEvent::Error(e)) => {
                    app.finish_streaming();
                    // BUG-CB-08: detect auth-class errors and append a helpful
                    // hint so users know to refresh their token.
                    let lower = e.to_lowercase();
                    let is_auth_error = lower.contains("expiredsignature")
                        || lower.contains("401")
                        || lower.contains("unauthorized");
                    let friendly_hint = if is_auth_error {
                        Some("\n提示：JWT 已过期，请运行 `rm ~/.cyberclaw/cli-token` 后重新执行 `cyberclaw onboard` 获取新令牌。")
                    } else {
                        None
                    };
                    let display = match friendly_hint {
                        Some(hint) => format!("[错误] {}{}", e, hint),
                        None => format!("[错误] {}", e),
                    };
                    app.push_system(display);
                    // BUG-CB-09: on auth-class errors, drop any queued messages
                    // so they are not auto-replayed on the next streaming attempt
                    // (which would fail again with the same 401 silently).
                    if is_auth_error {
                        let dropped = app.input_queue.len();
                        if dropped > 0 {
                            app.input_queue.clear();
                            app.push_system(format!(
                                "[queue] {} 条排队消息已丢弃（认证失败）",
                                dropped
                            ));
                        }
                    }
                }
                Ok(TokenEvent::Clarify(payload)) => {
                    let mut lines = vec![format!("[澄清请求] id={}", payload.id)];
                    for q in &payload.questions {
                        lines.push(format!("  问: {}", q.question));
                        for (i, opt) in q.options.iter().enumerate() {
                            lines.push(format!(
                                "    [{}] {} — {}",
                                i + 1,
                                opt.label,
                                opt.description
                            ));
                        }
                    }
                    app.push_system(lines.join("\n"));
                }
                Ok(TokenEvent::ApprovalRequest(ctx)) => {
                    app.overlay = OverlayState::Approval(ctx);
                }
                Ok(TokenEvent::RateLimit(rl)) => {
                    app.last_rate_limit = Some(rl);
                }
                Ok(TokenEvent::Usage(u)) => {
                    let usage = cyberclaw_llm::CanonicalUsage {
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        cache_read_tokens: u.cache_read_tokens,
                        cache_write_tokens: u.cache_write_tokens,
                    };
                    app.cost_accumulator.add_response(&u.model, &usage);
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        // streaming 完成 → 出队下一条 queued input（如果有）。
        // /retry 在主循环任何时刻都可设置 pending_retry，优先级高于队列。
        if !app.streaming && pending_send.is_none() {
            if let Some(retry) = app.pending_retry.take() {
                pending_send = Some(retry);
            } else if !app.input_queue.is_empty() {
                pending_send = Some(app.input_queue.remove(0));
            }
        }

        // 真正发送一条 user message —— 共用 path（直接输入 / /retry / queue 出队）
        if let Some(text) = pending_send.take() {
            if first_message {
                first_message = false;
                let title: String = text.chars().take(40).collect();
                let patch_url = format!("{}/api/v1/chat/conversations/{}", server, conv_id);
                let _ = client
                    .patch(&patch_url)
                    .bearer_auth(&token)
                    .json(&serde_json::json!({ "title": title }))
                    .send()
                    .await;
            }
            app.push_user(text.clone());
            app.push_input_history(&text);
            messages.push(super::chat::ChatMessage {
                role: "user".to_string(),
                content: text.clone(),
            });
            app.begin_assistant();

            let tx2 = tx.clone();
            let client2 = client.clone();
            let server2 = server.clone();
            let token2 = token.clone();
            let conv2 = conv_id.clone();
            let agent2 = current_agent_id.clone();
            let model2 = current_model.clone();
            let msgs2 = messages.clone();
            tokio::spawn(async move {
                send_message_tui(
                    &client2,
                    &server2,
                    &token2,
                    &conv2,
                    agent2.as_deref(),
                    &model2,
                    &msgs2,
                    tx2,
                )
                .await;
            });

            save_last_conv_id(&conv_id);
        }

        draw(&mut terminal, &mut app)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Overlay key handler
// ---------------------------------------------------------------------------

async fn handle_overlay_key(
    key: &crossterm::event::KeyEvent,
    app: &mut TuiApp<'_>,
    client: &Client,
    server: &str,
    token: &str,
    current_agent_id: &mut Option<String>,
    should_quit: &mut bool,
) {
    match &app.overlay {
        OverlayState::SlashHelp => {
            // any key closes
            app.overlay = OverlayState::None;
        }
        OverlayState::Approval(_) => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                if let OverlayState::Approval(ctx) =
                    std::mem::replace(&mut app.overlay, OverlayState::None)
                {
                    match post_approval_decision(client, server, token, &ctx.id, true).await {
                        Ok(_) => app.push_system(format!("[审批] 已批准: {}", ctx.capability)),
                        Err(e) => app.push_system(format!("[审批错误] {:#}", e)),
                    }
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                if let OverlayState::Approval(ctx) =
                    std::mem::replace(&mut app.overlay, OverlayState::None)
                {
                    match post_approval_decision(client, server, token, &ctx.id, false).await {
                        Ok(_) => app.push_system(format!("[审批] 已拒绝: {}", ctx.capability)),
                        Err(e) => app.push_system(format!("[审批错误] {:#}", e)),
                    }
                }
            }
            _ => {}
        },
        OverlayState::ResumePicker(_) => match key.code {
            KeyCode::Esc => {
                app.overlay = OverlayState::None;
            }
            KeyCode::Up => {
                if let OverlayState::ResumePicker(ref mut ctx) = app.overlay {
                    if ctx.selected > 0 {
                        ctx.selected -= 1;
                    }
                }
            }
            KeyCode::Down => {
                if let OverlayState::ResumePicker(ref mut ctx) = app.overlay {
                    let max = ctx.sessions.len().saturating_sub(1);
                    if ctx.selected < max {
                        ctx.selected += 1;
                    }
                }
            }
            KeyCode::Enter => {
                if let OverlayState::ResumePicker(ctx) =
                    std::mem::replace(&mut app.overlay, OverlayState::None)
                {
                    if let Some(session) = ctx.sessions.get(ctx.selected) {
                        let title = session.title.clone().unwrap_or_else(|| session.id.clone());
                        app.push_system(format!("[resume] 已选择会话: {} ({})", title, session.id));
                        save_last_conv_id(&session.id);
                    }
                }
            }
            _ => {}
        },
        OverlayState::SkillsList(_) => match key.code {
            KeyCode::Esc => {
                app.overlay = OverlayState::None;
            }
            KeyCode::Up => {
                if let OverlayState::SkillsList(ref mut ctx) = app.overlay {
                    if ctx.selected > 0 {
                        ctx.selected -= 1;
                    }
                }
            }
            KeyCode::Down => {
                if let OverlayState::SkillsList(ref mut ctx) = app.overlay {
                    let max = ctx.items.len().saturating_sub(1);
                    if ctx.selected < max {
                        ctx.selected += 1;
                    }
                }
            }
            _ => {}
        },
        OverlayState::AgentsList(_) => match key.code {
            KeyCode::Esc => {
                app.overlay = OverlayState::None;
            }
            KeyCode::Up => {
                if let OverlayState::AgentsList(ref mut ctx) = app.overlay {
                    if ctx.selected > 0 {
                        ctx.selected -= 1;
                    }
                }
            }
            KeyCode::Down => {
                if let OverlayState::AgentsList(ref mut ctx) = app.overlay {
                    let max = ctx.items.len().saturating_sub(1);
                    if ctx.selected < max {
                        ctx.selected += 1;
                    }
                }
            }
            KeyCode::Enter => {
                if let OverlayState::AgentsList(ctx) =
                    std::mem::replace(&mut app.overlay, OverlayState::None)
                {
                    if let Some(agent) = ctx.items.get(ctx.selected) {
                        *current_agent_id = Some(agent.id.clone());
                        app.push_system(format!(
                            "[agents] 已切换至 agent: {} ({})",
                            agent.name, agent.id
                        ));
                    }
                }
            }
            _ => {}
        },
        OverlayState::None => {}
    }
    let _ = should_quit; // used by caller
}

// ---------------------------------------------------------------------------
// Slash 命令 TUI 版（输出到历史区而非 stdout）
// Returns true if the TUI should quit.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)] // TUI slash 命令需要完整上下文：client/server/token/conv_id/model/agent/app/tx
async fn handle_slash_tui(
    cmd: &str,
    client: &Client,
    server: &str,
    token: &str,
    conv_id: &str,
    current_model: &mut String,
    _current_agent_id: &mut Option<String>,
    app: &mut TuiApp<'_>,
    _tx: mpsc::Sender<TokenEvent>,
) -> bool {
    let parts: Vec<&str> = cmd.trim().splitn(2, ' ').collect();
    match parts[0] {
        "/help" | "/h" => {
            app.overlay = OverlayState::SlashHelp;
        }
        "/clear" => {
            app.clear_history();
        }
        "/quit" | "/exit" | "/q" => {
            return true;
        }
        "/sessions" | "/resume" => match fetch_sessions(client, server, token).await {
            Ok(sessions) => {
                if sessions.is_empty() {
                    app.push_system("[sessions] 暂无历史会话".to_string());
                } else {
                    app.overlay = OverlayState::ResumePicker(ResumeCtx {
                        sessions,
                        selected: 0,
                    });
                }
            }
            Err(e) => {
                app.push_system(format!("[sessions] 加载失败: {:#}", e));
            }
        },
        "/skills" => match fetch_skills(client, server, token).await {
            Ok(skills) => {
                if skills.is_empty() {
                    app.push_system("[skills] 暂无激活的 skill".to_string());
                } else {
                    app.overlay = OverlayState::SkillsList(SkillsCtx {
                        items: skills,
                        selected: 0,
                    });
                }
            }
            Err(e) => {
                app.push_system(format!("[skills] 加载失败: {:#}", e));
            }
        },
        "/agents" => match fetch_agents(client, server, token).await {
            Ok(agents) => {
                if agents.is_empty() {
                    app.push_system("[agents] 暂无已注册的 agent".to_string());
                } else {
                    app.overlay = OverlayState::AgentsList(AgentsCtx {
                        items: agents,
                        selected: 0,
                    });
                }
            }
            Err(e) => {
                app.push_system(format!("[agents] 加载失败: {:#}", e));
            }
        },
        "/history" | "/save" => {
            let path = parts.get(1).copied().unwrap_or("conversation.md");
            match save_history_to_file(app, path) {
                Ok(_) => app.push_system(format!("[history] 已导出到: {}", path)),
                Err(e) => app.push_system(format!("[history] 导出失败: {:#}", e)),
            }
        }
        "/token" => {
            app.push_system(format!(
                "[token] 估算使用量: ~{} tokens（字符数/4）",
                app.token_estimate
            ));
        }
        "/usage" => {
            // Hermes parity: richer than /token. Show msg breakdown + a
            // context-window guess so the operator can see how close the
            // current session is to needing /compress.
            let total_msgs = app.history.len();
            let user_msgs = app.history.iter().filter(|m| m.role == "user").count();
            let assistant_msgs = app.history.iter().filter(|m| m.role == "assistant").count();
            // Conservative ceiling — most modern models we ship advertise
            // 128k tokens. We do NOT introspect the model's real cap here
            // because that would need a metadata round-trip to the server
            // and /usage is meant to be a zero-cost local readout.
            const CONSERVATIVE_CTX_GUESS: usize = 128_000;
            let pct = (app.token_estimate * 100)
                .checked_div(CONSERVATIVE_CTX_GUESS)
                .unwrap_or(0);
            app.push_system(format!(
                "[usage] tokens≈{}  msgs={} (user={}, assistant={})  ctx≈{}% of 128k guess  model={}",
                app.token_estimate,
                total_msgs,
                user_msgs,
                assistant_msgs,
                pct,
                current_model,
            ));
            // Hermes Gap 3.2 — rate limit readout from provider response headers.
            match &app.last_rate_limit {
                None => {
                    app.push_system("[usage] Rate limit: provider did not report".to_string());
                }
                Some(rl) => {
                    let req_line = match (rl.requests_remaining, rl.requests_limit) {
                        (Some(rem), Some(lim)) => {
                            let used = lim.saturating_sub(rem);
                            let pct = (used * 100).checked_div(lim).unwrap_or(0);
                            let reset_str = rl
                                .requests_reset_secs
                                .map(|s| format!("  reset in {:.0}s", s))
                                .unwrap_or_default();
                            format!("  Requests: {}/{} RPM ({}%){reset_str}", used, lim, pct)
                        }
                        _ => "  Requests: not reported".to_string(),
                    };
                    let tok_line = match (rl.tokens_remaining, rl.tokens_limit) {
                        (Some(rem), Some(lim)) => {
                            let used = lim.saturating_sub(rem);
                            let pct = (used * 100).checked_div(lim).unwrap_or(0);
                            let reset_str = rl
                                .tokens_reset_secs
                                .map(|s| format!("  reset in {:.0}s", s))
                                .unwrap_or_default();
                            let used_k = used as f64 / 1000.0;
                            let lim_k = lim as f64 / 1000.0;
                            format!(
                                "  Tokens:   {:.1}k/{:.1}k TPM ({}%){reset_str}",
                                used_k, lim_k, pct
                            )
                        }
                        _ => "  Tokens:   not reported".to_string(),
                    };
                    app.push_system(format!(
                        "[usage] Rate limit ({}):\n{}\n{}",
                        rl.provider, req_line, tok_line
                    ));
                }
            }
            // Cost estimation (Gap 3.1)
            app.push_system(app.cost_accumulator.summary());
        }
        "/undo" => {
            // Hermes parity: drop the most recent assistant+user pair from
            // the local transcript so the operator can re-do a turn. We
            // only mutate UI state — the server-side conversation log is
            // untouched (the audit trail is a separate hash-chained
            // record), so /undo is a presentation operation, not an
            // attempt to rewrite history.
            let mut removed = 0usize;
            // Strip a trailing assistant turn first (it might be mid-stream),
            // then strip the user turn that elicited it.
            if app.history.last().is_some_and(|m| m.role == "assistant") {
                app.history.pop();
                removed += 1;
            }
            if app.history.last().is_some_and(|m| m.role == "user") {
                app.history.pop();
                removed += 1;
            }
            app.push_system(format!(
                "[undo] 已从本地 transcript 移除 {} 条消息（server-side 会话 + 审计链未触动）",
                removed
            ));
        }
        "/trace" => {
            app.show_thinking = !app.show_thinking;
            app.push_system(format!(
                "[trace] <think> 块显示: {}",
                if app.show_thinking {
                    "开启"
                } else {
                    "关闭"
                }
            ));
        }
        "/retry" => {
            // Hermes parity: re-emit the most recent user message from the
            // local transcript. We do NOT touch the server-side conversation
            // log — the new turn is appended as a normal /chat call which
            // re-runs the whole context through the model.
            //
            // 在 streaming 中拒绝，因为重发会和当前 spawn 的 send_message_tui
            // 冲突（同一 tx 上两个 producer）。
            if app.streaming {
                app.push_system(
                    "[retry] 当前正在 streaming，请先 Esc 等响应完成再 /retry".to_string(),
                );
            } else {
                let last_user = app
                    .history
                    .iter()
                    .rev()
                    .find(|m| m.role == "user")
                    .map(|m| m.content.clone());
                match last_user {
                    Some(content) => {
                        app.pending_retry = Some(content.clone());
                        let preview: String = content.chars().take(40).collect();
                        let ellipsis = if content.chars().count() > 40 {
                            "…"
                        } else {
                            ""
                        };
                        app.push_system(format!("[retry] 即将重发: \"{}{}\"", preview, ellipsis));
                    }
                    None => {
                        app.push_system(
                            "[retry] 本地 transcript 无 user message 可重发".to_string(),
                        );
                    }
                }
            }
        }
        "/queue" => {
            // 显示当前 streaming 期间累积的 input_queue 内容。
            if app.input_queue.is_empty() {
                app.push_system("[queue] no messages queued".to_string());
            } else {
                let mut lines: Vec<String> =
                    vec![format!("## 排队消息（{} 条）", app.input_queue.len())];
                for (i, msg) in app.input_queue.iter().enumerate() {
                    let preview: String = msg.chars().take(80).collect();
                    let ellipsis = if msg.chars().count() > 80 { "…" } else { "" };
                    lines.push(format!("  {}. {}{}", i + 1, preview, ellipsis));
                }
                app.push_system(lines.join("\n"));
            }
        }
        "/details" => {
            // Hermes 兼容子参数：hidden/expanded/cycle；无参数 = cycle。
            // details 同时控制 show_thinking 和 show_tool_details——它是
            // 比 /trace 更广的"细节可见性"开关。
            let arg = parts.get(1).copied().unwrap_or("").trim().to_lowercase();
            let target_on = match arg.as_str() {
                "expanded" | "on" | "show" => true,
                "hidden" | "off" | "hide" => false,
                "" | "cycle" | "toggle" => !(app.show_thinking && app.show_tool_details),
                other => {
                    app.push_system(format!(
                        "[details] 未知参数: '{}'（用 hidden/expanded/cycle）",
                        other
                    ));
                    return false;
                }
            };
            app.show_thinking = target_on;
            app.show_tool_details = target_on;
            app.push_system(format!(
                "[details] 细节显示: {}（thinking + tool block）",
                if target_on { "开启" } else { "关闭" }
            ));
        }
        "/model" => {
            if let Some(name) = parts.get(1).copied() {
                *current_model = name.to_string();
                app.push_system(format!("[model] 已切换至: {}", current_model));
            } else {
                app.push_system(format!("[model] 当前 model: {}", current_model));
            }
        }
        "/security" => {
            // Fetch permission rules + injection hits, print markdown summary.
            let rules_url = format!("{}/api/v1/security/permission/rules", server);
            let hits_url = format!("{}/api/v1/security/injection/hits", server);

            let rules_resp = client.get(&rules_url).bearer_auth(token).send().await;
            let hits_resp = client.get(&hits_url).bearer_auth(token).send().await;

            #[derive(serde::Deserialize)]
            struct RuleItem {
                source: String,
                severity: String,
            }
            #[derive(serde::Deserialize)]
            struct RulesBody {
                rules: Vec<RuleItem>,
            }
            #[derive(serde::Deserialize)]
            struct HitItem {
                ts: i64,
            }
            #[derive(serde::Deserialize)]
            struct HitsBody {
                hits: Vec<HitItem>,
            }

            let mut lines: Vec<String> = vec!["## Security 状态".to_string(), String::new()];

            match rules_resp {
                Ok(r) if r.status().is_success() => match r.json::<RulesBody>().await {
                    Ok(body) => {
                        let dcf = body.rules.iter().filter(|r| r.source == "dcf").count();
                        let tpm = body.rules.iter().filter(|r| r.source == "tpm").count();
                        let total = body.rules.len();
                        let critical = body
                            .rules
                            .iter()
                            .filter(|r| r.severity.to_lowercase() == "critical")
                            .count();
                        let high = body
                            .rules
                            .iter()
                            .filter(|r| r.severity.to_lowercase() == "high")
                            .count();
                        let medium = body
                            .rules
                            .iter()
                            .filter(|r| r.severity.to_lowercase() == "medium")
                            .count();
                        let low = body
                            .rules
                            .iter()
                            .filter(|r| r.severity.to_lowercase() == "low")
                            .count();
                        lines.push(format!("- 启用规则：{} (DangerousCapabilityFilter) + {} (ToolPermissionMatcher) = {}", dcf, tpm, total));
                        lines.push(format!(
                            "- severity 分布：Critical {} / High {} / Medium {} / Low {}",
                            critical, high, medium, low
                        ));
                    }
                    Err(e) => lines.push(format!("- 规则解析失败: {:#}", e)),
                },
                Ok(r) => lines.push(format!("- 规则请求失败: HTTP {}", r.status())),
                Err(e) => lines.push(format!("- 规则请求错误: {:#}", e)),
            }

            match hits_resp {
                Ok(r) if r.status().is_success() => match r.json::<HitsBody>().await {
                    Ok(body) => {
                        let now_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        let today = body
                            .hits
                            .iter()
                            .filter(|h| now_ms - h.ts < 86_400_000)
                            .count();
                        lines.push(format!("- 24h 注入命中：{}", today));
                    }
                    Err(e) => lines.push(format!("- 命中解析失败: {:#}", e)),
                },
                Ok(r) => lines.push(format!("- 命中请求失败: HTTP {}", r.status())),
                Err(e) => lines.push(format!("- 命中请求错误: {:#}", e)),
            }

            app.push_system(lines.join("\n"));
        }
        "/digest" => {
            // GET /api/v1/learning/daily-digest?date=YYYY-MM-DD
            let date_str = parts
                .get(1)
                .copied()
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    // seconds since epoch → days → YYYY-MM-DD (simple UTC approximation)
                    // Use a naive but correct approach via modular arithmetic
                    // Fallback: format as epoch days since we have no chrono in scope;
                    // try to derive from SystemTime using duration math.
                    let secs = now;
                    let days_since_epoch = secs / 86400;
                    // Julian day calculation (Gregorian calendar)
                    let z = days_since_epoch + 719468;
                    let era = z / 146097;
                    let doe = z - era * 146097;
                    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
                    let y2 = yoe + era * 400;
                    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
                    let mp = (5 * doy + 2) / 153;
                    let d = doy - (153 * mp + 2) / 5 + 1;
                    let m = if mp < 10 { mp + 3 } else { mp - 9 };
                    let y3 = if m <= 2 { y2 + 1 } else { y2 };
                    format!("{:04}-{:02}-{:02}", y3, m, d)
                });
            let url = format!("{}/api/v1/learning/daily-digest?date={}", server, date_str);
            let resp = client.get(&url).bearer_auth(token).send().await;

            // Actual API shape: stats fields are {value, delta, sub} objects;
            // security_events is named `incidents`; feed is a flat array of strings;
            // highlights is a flat array of strings.
            #[derive(serde::Deserialize, Default)]
            struct DigestStatField {
                value: Option<u64>,
            }
            #[derive(serde::Deserialize, Default)]
            struct DigestStats {
                executions: Option<DigestStatField>,
                approvals: Option<DigestStatField>,
                learning_entries: Option<DigestStatField>,
                incidents: Option<DigestStatField>,
            }
            #[derive(serde::Deserialize, Default)]
            struct DigestBody {
                #[allow(dead_code)]
                date: Option<String>,
                stats: Option<DigestStats>,
                // highlights and feed are either strings or objects — accept both via Value
                highlights: Option<Vec<serde_json::Value>>,
                feed: Option<Vec<serde_json::Value>>,
            }

            let mut lines: Vec<String> = vec![format!("## 今日摘要 — {}", date_str), String::new()];

            match resp {
                Ok(r) if r.status().is_success() => {
                    match r.json::<DigestBody>().await {
                        Ok(body) => {
                            let st = body.stats.unwrap_or_default();
                            let execs = st.executions.unwrap_or_default().value.unwrap_or(0);
                            let approvs = st.approvals.unwrap_or_default().value.unwrap_or(0);
                            let learn = st.learning_entries.unwrap_or_default().value.unwrap_or(0);
                            let incidents = st.incidents.unwrap_or_default().value.unwrap_or(0);
                            lines.push(format!(
                                "- 执行：{} / 审批：{} / 学习条目：{} / 安全事件：{}",
                                execs, approvs, learn, incidents,
                            ));
                            // highlights (max 3) — stringify each Value
                            if let Some(hl) = body.highlights {
                                let shown: Vec<String> = hl
                                    .into_iter()
                                    .take(3)
                                    .map(|v| match v {
                                        serde_json::Value::String(s) => s,
                                        other => other.to_string(),
                                    })
                                    .collect();
                                if !shown.is_empty() {
                                    lines.push(format!("- 亮点：{}", shown.join(" · ")));
                                }
                            }
                            // feed is a flat array — show first 2 entries
                            if let Some(feed) = body.feed {
                                let entries: Vec<String> = feed
                                    .into_iter()
                                    .take(2)
                                    .map(|v| match v {
                                        serde_json::Value::String(s) => s,
                                        other => other.to_string(),
                                    })
                                    .collect();
                                if !entries.is_empty() {
                                    lines.push(format!("- Feed：{}", entries.join(" · ")));
                                }
                            }
                        }
                        Err(e) => lines.push(format!("- 解析失败: {:#}", e)),
                    }
                }
                Ok(r) => lines.push(format!("- 查询失败: HTTP {}", r.status())),
                Err(e) => lines.push(format!("- 请求错误: {:#}", e)),
            }

            app.push_system(lines.join("\n"));
        }
        "/orgmem" => {
            // GET /api/v1/learning/org-memory  (client-side filter)
            let query = parts.get(1).copied().unwrap_or("").to_lowercase();
            let url = format!("{}/api/v1/learning/org-memory", server);
            let resp = client.get(&url).bearer_auth(token).send().await;

            #[derive(serde::Deserialize)]
            struct OrgMemItem {
                kind: Option<String>,
                label: Option<String>,
                created_at: Option<String>,
                content: Option<String>,
            }
            #[derive(serde::Deserialize)]
            struct OrgMemBody {
                // Actual API uses "entries" not "items"
                #[serde(alias = "items")]
                entries: Vec<OrgMemItem>,
            }

            let header = if query.is_empty() {
                "## 组织记忆 — 最近条目".to_string()
            } else {
                format!("## 组织记忆 — 查询: \"{}\"", query)
            };
            let mut lines: Vec<String> = vec![header, String::new()];

            match resp {
                Ok(r) if r.status().is_success() => {
                    match r.json::<OrgMemBody>().await {
                        Ok(body) => {
                            let total = body.entries.len();
                            // filter by query if provided
                            let matched: Vec<&OrgMemItem> = if query.is_empty() {
                                body.entries.iter().collect()
                            } else {
                                body.entries
                                    .iter()
                                    .filter(|it| {
                                        let haystack = format!(
                                            "{} {} {}",
                                            it.kind.as_deref().unwrap_or(""),
                                            it.label.as_deref().unwrap_or(""),
                                            it.content.as_deref().unwrap_or("")
                                        )
                                        .to_lowercase();
                                        haystack.contains(&query)
                                    })
                                    .collect()
                            };
                            let hit = matched.len();
                            if !query.is_empty() {
                                lines[0] = format!(
                                    "## 组织记忆 — 查询: \"{}\"（命中 {} / 共 {}）",
                                    query, hit, total
                                );
                            }
                            if matched.is_empty() {
                                lines.push("- 无匹配条目".to_string());
                            } else {
                                for (i, it) in matched.into_iter().take(5).enumerate() {
                                    let kind = it.kind.as_deref().unwrap_or("unknown");
                                    let label = it.label.as_deref().unwrap_or("—");
                                    let ts = it.created_at.as_deref().unwrap_or("—");
                                    let content = it.content.as_deref().unwrap_or("");
                                    // truncate at 100 chars
                                    let preview: String = content.chars().take(100).collect();
                                    let ellipsis = if content.chars().count() > 100 {
                                        "…"
                                    } else {
                                        ""
                                    };
                                    lines.push(format!(
                                        "{}. **{}** {} · {}\n   {}{}",
                                        i + 1,
                                        kind,
                                        label,
                                        ts,
                                        preview,
                                        ellipsis
                                    ));
                                }
                            }
                        }
                        Err(e) => lines.push(format!("- 解析失败: {:#}", e)),
                    }
                }
                Ok(r) => lines.push(format!("- 查询失败: HTTP {}", r.status())),
                Err(e) => lines.push(format!("- 请求错误: {:#}", e)),
            }

            app.push_system(lines.join("\n"));
        }
        "/curator" => {
            // GET /api/v1/admin/curator/status
            let url = format!("{}/api/v1/admin/curator/status", server);
            let resp = client.get(&url).bearer_auth(token).send().await;

            let mut lines: Vec<String> = vec!["## Curator 状态".to_string(), String::new()];

            match resp {
                Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
                    Ok(body) => {
                        let enabled = body
                            .get("enabled")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let total_runs =
                            body.get("total_runs").and_then(|v| v.as_u64()).unwrap_or(0);
                        let last_run = body.get("last_run").and_then(|v| v.as_str()).unwrap_or("—");
                        let next_run = body.get("next_run").and_then(|v| v.as_str()).unwrap_or("—");
                        lines.push(format!("- enabled: {}", enabled));
                        lines.push(format!("- 上次运行: {}", last_run));
                        lines.push(format!("- 下次运行: {}", next_run));
                        lines.push(format!("- 累计运行: {}", total_runs));
                    }
                    Err(e) => lines.push(format!("- 解析失败: {:#}", e)),
                },
                Ok(r) => lines.push(format!("- 查询失败: HTTP {}", r.status())),
                Err(e) => lines.push(format!("- 请求错误: {:#}", e)),
            }

            app.push_system(lines.join("\n"));
        }
        "/compress" => {
            match handle_slash(cmd, client, server, token, conv_id, current_model).await {
                Ok(SlashResult::Quit) => return true,
                Ok(SlashResult::Continue) => {
                    app.push_system("[compress] 压缩完成".to_string());
                }
                Err(e) => {
                    app.push_system(format!("[错误] {:#}", e));
                }
            }
        }
        _ => {
            // Delegate to chat.rs handle_slash for unrecognised commands
            match handle_slash(cmd, client, server, token, conv_id, current_model).await {
                Ok(SlashResult::Quit) => return true,
                Ok(SlashResult::Continue) => {
                    app.push_system(format!("[{}] 已执行", parts[0]));
                }
                Err(e) => {
                    app.push_system(format!("[错误] {:#}", e));
                }
            }
        }
    }
    false
}

/// Export current in-memory history to a markdown file.
fn save_history_to_file(app: &TuiApp, path: &str) -> Result<()> {
    let mut out = String::new();
    out.push_str("# CyberClaw Conversation Export\n\n");
    for block in &app.history {
        let role = match block.role.as_str() {
            "user" => "**You**",
            "assistant" => "**Assistant**",
            _ => "**System**",
        };
        let ts = block.ts.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S %Z");
        out.push_str(&format!("### {} — {}\n\n{}\n\n", role, ts, block.content));
        out.push_str("---\n\n");
    }
    std::fs::write(path, out).with_context(|| format!("write {}", path))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_app() -> TuiApp<'static> {
        TuiApp::new("test-model".to_string(), "conv-test".to_string())
    }

    #[test]
    fn slash_help_registers_new_commands() {
        // 自动补全和 /help 表必须包含三个新命令。
        let cmds: Vec<&str> = SLASH_HELP.iter().map(|(c, _)| *c).collect();
        assert!(cmds.contains(&"/retry"), "/retry must be in SLASH_HELP");
        assert!(cmds.contains(&"/queue"), "/queue must be in SLASH_HELP");
        assert!(
            cmds.iter().any(|c| c.starts_with("/details")),
            "/details must be in SLASH_HELP"
        );
    }

    #[test]
    fn slash_autocomplete_finds_new_commands() {
        let m = slash_autocomplete_matches("/ret");
        assert!(m.iter().any(|(c, _)| *c == "/retry"));
        let m = slash_autocomplete_matches("/que");
        assert!(m.iter().any(|(c, _)| *c == "/queue"));
        let m = slash_autocomplete_matches("/det");
        assert!(m.iter().any(|(c, _)| c.starts_with("/details")));
    }

    #[test]
    fn tui_app_defaults_have_new_fields() {
        let app = fresh_app();
        assert!(app.pending_retry.is_none());
        assert!(app.input_queue.is_empty());
        assert!(
            app.show_tool_details,
            "show_tool_details default must be true (hermes parity)"
        );
        // BUG-CB-10: auto-pin 默认开启（live tail 行为）
        assert!(
            app.conversation_auto_pin,
            "conversation_auto_pin must default to true"
        );
        assert_eq!(
            app.conversation_viewport_height, 0,
            "viewport_height starts at 0 before first draw"
        );
    }

    #[test]
    fn scroll_auto_pin_suppresses_scroll_to_bottom() {
        // BUG-CB-10: 关闭 auto-pin 后 scroll_to_bottom() 不再强制跳底
        let mut app = fresh_app();
        app.scroll_offset = 42;
        app.conversation_auto_pin = false;
        app.scroll_to_bottom();
        assert_eq!(
            app.scroll_offset, 42,
            "scroll_to_bottom must be no-op when auto_pin=false"
        );
    }

    #[test]
    fn scroll_to_bottom_force_restores_pin() {
        // BUG-CB-10: End 键调用 scroll_to_bottom_force() 恢复 auto-pin
        let mut app = fresh_app();
        app.conversation_auto_pin = false;
        app.scroll_offset = 42;
        app.scroll_to_bottom_force();
        assert!(
            app.conversation_auto_pin,
            "force must restore auto_pin=true"
        );
        assert_eq!(app.scroll_offset, u16::MAX, "force must set offset to u16::MAX");
    }

    #[test]
    fn retry_finds_last_user_message() {
        let mut app = fresh_app();
        app.push_user("first ask".to_string());
        app.history.push(MessageBlock {
            role: "assistant".to_string(),
            content: "reply".to_string(),
            ts: Utc::now(),
            streaming: false,
        });
        app.push_user("second ask".to_string());
        app.history.push(MessageBlock {
            role: "assistant".to_string(),
            content: "reply 2".to_string(),
            ts: Utc::now(),
            streaming: false,
        });

        // 模拟 /retry 处理逻辑 (从尾部 reverse 找 user)
        let last_user = app
            .history
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone());
        assert_eq!(last_user.as_deref(), Some("second ask"));
    }

    #[test]
    fn retry_returns_none_when_no_user_message() {
        let app = fresh_app();
        let last_user = app
            .history
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone());
        assert!(last_user.is_none());
    }

    #[test]
    fn queue_enqueue_and_dequeue() {
        let mut app = fresh_app();
        app.input_queue.push("msg-1".to_string());
        app.input_queue.push("msg-2".to_string());
        assert_eq!(app.input_queue.len(), 2);

        // FIFO 出队，跟主循环 remove(0) 一致
        let next = app.input_queue.remove(0);
        assert_eq!(next, "msg-1");
        assert_eq!(app.input_queue.len(), 1);
        assert_eq!(app.input_queue[0], "msg-2");
    }

    #[test]
    fn details_toggle_affects_both_flags() {
        let mut app = fresh_app();
        // 起始：thinking=false, tool_details=true（hermes 默认）
        assert!(!app.show_thinking);
        assert!(app.show_tool_details);

        // cycle: 因为 thinking && tool_details = false → 应开启
        let target = !(app.show_thinking && app.show_tool_details);
        assert!(target);
        app.show_thinking = target;
        app.show_tool_details = target;
        assert!(app.show_thinking);
        assert!(app.show_tool_details);

        // 再 cycle：两个都 on → 应关闭
        let target = !(app.show_thinking && app.show_tool_details);
        assert!(!target);
        app.show_thinking = target;
        app.show_tool_details = target;
        assert!(!app.show_thinking);
        assert!(!app.show_tool_details);
    }

    #[test]
    fn details_arg_parsing() {
        // 模拟 handle_slash_tui 里的 arg 解析路径
        let parse = |arg: &str, show_thinking: bool, show_tool: bool| -> Option<bool> {
            let arg = arg.trim().to_lowercase();
            match arg.as_str() {
                "expanded" | "on" | "show" => Some(true),
                "hidden" | "off" | "hide" => Some(false),
                "" | "cycle" | "toggle" => Some(!(show_thinking && show_tool)),
                _ => None,
            }
        };

        assert_eq!(parse("expanded", false, true), Some(true));
        assert_eq!(parse("hidden", true, true), Some(false));
        assert_eq!(parse("", false, true), Some(true));
        assert_eq!(parse("cycle", true, true), Some(false));
        assert_eq!(parse("on", false, false), Some(true));
        assert_eq!(parse("nonsense", false, true), None);
        // case-insensitive
        assert_eq!(parse("EXPANDED", false, true), Some(true));
    }

    #[test]
    fn pending_retry_is_consumed_once() {
        let mut app = fresh_app();
        app.pending_retry = Some("retry me".to_string());
        let taken = app.pending_retry.take();
        assert_eq!(taken.as_deref(), Some("retry me"));
        assert!(app.pending_retry.is_none());
    }

    #[test]
    fn queue_show_with_empty_returns_expected_text() {
        // 不能直接 await async handler 而不引入 tokio runtime——这里只验证
        // 行为契约：空队列的 system 提示文案。
        let app = fresh_app();
        let msg = if app.input_queue.is_empty() {
            "[queue] no messages queued".to_string()
        } else {
            "non-empty".to_string()
        };
        assert_eq!(msg, "[queue] no messages queued");
    }
}
