# cyberclaw-llm

CyberClaw LLM 集成层，提供统一的多 LLM 提供商客户端接口。

## Overview

Unified LLM client abstraction supporting multiple providers through a common `LlmClient` trait (4 methods: `chat_completion`, `chat_completion_stream`, `embedding`, `model_info`).

## Supported Providers

| Provider | Module | Description |
|----------|--------|-------------|
| ARK (Volcengine) | `providers::ark` | 火山引擎方舟平台（推荐国内用户） |
| OpenAI | `providers::openai` | OpenAI 官方 API |
| Anthropic | `providers::anthropic` | Anthropic Claude API |
| Generic | `providers::generic` | 通用 OpenAI 兼容接口（DeepSeek、Ollama 等） |

## Modules

| Module | Description |
|--------|-------------|
| `client` | `LlmClient` trait — core abstraction for LLM interactions |
| `providers` | Provider-specific implementations |
| `error` | `LlmError` types |

## Usage

```rust
use cyberclaw_llm::prelude::*;

let client = OpenAiClient::new("sk-xxx".to_string(), "https://api.openai.com/v1")?;
let request = ChatRequest {
    model: "gpt-4".to_string(),
    messages: vec![Message::user("Hello")],
    ..Default::default()
};
let response = client.chat_completion(request).await?;
```

## Known Debt

- `client.rs` lacks unit tests — this is a high-priority gap as it's the core LLM call path.

## Testing

```bash
cargo test -p cyberclaw-llm
```
