# CyberClaw Server 测试指南

本指南说明如何在不同模式下运行 CyberClaw Server 的测试套件。

---

## 📋 目录

- [测试模式概览](#测试模式概览)
- [快速开始](#快速开始)
- [Mock 模式测试（推荐本地开发）](#mock-模式测试推荐本地开发)
- [Real LLM 模式测试（集成测试）](#real-llm-模式测试集成测试)
- [测试环境差异说明](#测试环境差异说明)
- [故障排除](#故障排除)
- [CI/CD 配置建议](#cicd-配置建议)

---

## 测试模式概览

CyberClaw Server 测试套件支持两种运行模式：

| 特性 | Mock 模式 | Real LLM 模式 |
|------|-----------|---------------|
| **LLM 客户端** | `MockLlmClient` | 真实 LLM 服务（OpenAI, Anthropic, 火山引擎等） |
| **环境变量** | 无 `.env` 或 `LLM_PROVIDER` 未设置/设为 `mock` | `.env` 文件配置且 `LLM_PROVIDER` 设为真实提供商 |
| **外部依赖** | 无 | 需要网络连接和有效 API 密钥 |
| **测试速度** | 快（无网络延迟） | 慢（受网络和 API 响应时间影响） |
| **稳定性** | 高（无外部因素） | 中（受网络、配额、服务可用性影响） |
| **典型用途** | 本地开发、快速回归测试、CI 快速反馈 | 集成测试、上线前验证、API 兼容性测试 |
| **测试结果** | 1457/1457 通过 | 1455/1457 通过（2 个 E2E 测试可能因环境因素失败） |

---

## 快速开始

### 1. Mock 模式（默认，推荐）

```bash
# 方式 1: 无 .env 文件（自动使用 Mock）
cargo test --workspace

# 方式 2: 使用 .env.test 模板（明确指定 Mock 模式）
cp .env.test .env
cargo test --workspace

# 预期结果：
# - 1454 个测试通过
# - 3 个 doctest 被忽略
# - 0 个失败
# - 总耗时：约 2-5 分钟
```

### 2. Real LLM 模式

```bash
# 1. 创建 .env 文件并配置 LLM 提供商
cp .env.example .env
# 2. 编辑 .env，设置 LLM_PROVIDER 和对应 API 密钥
# 3. 运行测试
cargo test --workspace

# 预期结果：
# - 1452-1455 个测试通过
# - 0-2 个 E2E 测试可能因环境因素失败
# - 3 个 doctest 被忽略
# - 总耗时：约 5-15 分钟（受 LLM API 响应时间影响）
```

---

## Mock 模式测试（推荐本地开发）

### 配置方法

**方式 1：不创建 `.env` 文件（最简单）**

```bash
# 删除现有 .env（如果有）
rm -f .env

# 运行测试
cargo test --workspace
```

**方式 2：使用 `.env.test` 模板**

```bash
# 复制测试模板
cp .env.test .env

# .env.test 默认不设置 LLM_PROVIDER，自动使用 Mock 模式
cargo test --workspace
```

### Mock 模式特点

- ✅ **无外部依赖**：不需要网络连接、API 密钥或真实 LLM 服务
- ✅ **快速稳定**：单次请求延迟约 500-600ms，无网络波动
- ✅ **可预测响应**：`MockLlmClient` 返回固定响应内容
- ✅ **适合 TDD**：快速反馈循环，适合测试驱动开发
- ✅ **CI 友好**：无外部依赖，适合 CI/CD 快速反馈

### Mock 客户端实现

```rust
// apps/cyberclaw-server/tests/common/mod.rs:220-240
pub struct MockLlmClient {
    response_content: String,
    model_name: String,
}

impl MockLlmClient {
    pub fn new() -> Self {
        Self {
            response_content: "This is a mock response from the test LLM client.".to_string(),
            model_name: std::env::var("LLM_DEFAULT_MODEL")
                .unwrap_or_else(|_| "gpt-4".to_string()),
        }
    }
}
```

---

## Real LLM 模式测试（集成测试）

### 配置方法

**1. 创建 `.env` 文件**

```bash
# 从示例文件开始
cp .env.example .env
```

**2. 编辑 `.env` 文件，配置 LLM 提供商**

选择以下之一：

**选项 A: 火山引擎方舟（推荐国内用户）**

```bash
LLM_PROVIDER=volcengine
ARK_API_KEY=your-ark-api-key-here
ARK_MODEL=minimax-m2.5
ARK_BASE_URL=https://ark.cn-beijing.volces.com/api/v3
```

**选项 B: OpenAI**

```bash
LLM_PROVIDER=openai
OPENAI_API_KEY=sk-your-openai-key-here
OPENAI_BASE_URL=https://api.openai.com/v1
```

**选项 C: Anthropic (Claude)**

```bash
LLM_PROVIDER=anthropic
ANTHROPIC_API_KEY=sk-ant-your-anthropic-key-here
```

**选项 D: DeepSeek**

```bash
LLM_PROVIDER=deepseek
DEEPSEEK_API_KEY=sk-your-deepseek-key-here
```

**选项 E: Together AI**

```bash
LLM_PROVIDER=together
TOGETHER_API_KEY=your-together-key-here
```

**3. 运行测试**

```bash
# 运行完整测试套件
cargo test --workspace

# 或单独运行 E2E 测试
cargo test --package cyberclaw-server --test e2e_chat_completion_test
cargo test --package cyberclaw-server --test e2e_integration_test
```

### Real LLM 模式特点

- ⚠️ **外部依赖**：需要网络连接和有效 API 密钥
- ⚠️ **速度较慢**：单次请求延迟 1-5 秒（受网络和 API 响应时间影响）
- ⚠️ **可能失败**：受网络延迟、API 配额、服务可用性影响
- ✅ **真实验证**：验证与真实 LLM 服务的集成
- ✅ **上线前必需**：确保生产环境兼容性

### 环境依赖的测试

以下测试在 Real LLM 模式下可能失败：

| 测试名称 | 位置 | 可能失败原因 | 修复状态 |
|---------|------|-------------|---------|
| `test_e2e_chat_008_performance_benchmark` | `e2e_chat_completion_test.rs:314` | 100 次请求超时（原 60s → 现 180s） | ✅ 已修复 |
| `test_e2e_int_004_complete_user_journey` | `e2e_integration_test.rs:138` | 多阶段 LLM 调用失败（502 错误） | ✅ 已优化 |

### 修复内容

**test_e2e_chat_008_performance_benchmark:**
- 增加超时时间：Mock 模式 60 秒 → Real LLM 模式 180 秒
- 自动检测 LLM 模式并调整超时
- 更清晰的错误信息

**test_e2e_int_004_complete_user_journey:**
- 添加 LLM 模式检测和提示
- 在 LLM 调用之间添加 500ms 延迟，避免速率限制
- 增强错误信息，明确指出 502 错误的可能原因
- 每个阶段提供详细的失败诊断信息

---

## 测试环境差异说明

### TestServer 客户端选择逻辑

```rust
// apps/cyberclaw-server/tests/common/mod.rs:241-293
pub fn new_with_config(config: ServerConfig) -> Self {
    let llm_client: Arc<dyn LlmClient> = {
        let provider = env::var("LLM_PROVIDER")
            .unwrap_or_else(|_| "mock".to_string());

        match provider.as_str() {
            "ark" => Arc::new(ArkClient::from_env().expect("Failed to create ArkClient")),
            "openai" => Arc::new(OpenAiClient::from_env().expect("Failed to create OpenAiClient")),
            "anthropic" => Arc::new(AnthropicClient::from_env().expect("Failed to create AnthropicClient")),
            "generic" => Arc::new(GenericOpenAiClient::from_env().expect("Failed to create GenericOpenAiClient")),
            _ => Arc::new(MockLlmClient::new()),  // 默认 Mock
        }
    };

    // ... 创建 TestServer
}
```

### 环境变量检测流程

```
┌─────────────────────────────────────┐
│  cargo test --workspace 启动        │
└─────────────┬───────────────────────┘
              │
              v
┌─────────────────────────────────────┐
│  TestServer::new() 初始化           │
└─────────────┬───────────────────────┘
              │
              v
┌─────────────────────────────────────┐
│  读取 LLM_PROVIDER 环境变量         │
└─────────────┬───────────────────────┘
              │
      ┌───────┴────────┐
      │                │
      v                v
┌─────────┐      ┌──────────┐
│ 未设置  │      │ 已设置   │
│ 或      │      │ 真实提供 │
│ "mock"  │      │ 商名称   │
└────┬────┘      └────┬─────┘
     │                │
     v                v
┌────────────┐  ┌─────────────┐
│ MockLlm    │  │ Real LLM    │
│ Client     │  │ Client      │
│ (快速)     │  │ (真实 API)  │
└────────────┘  └─────────────┘
```

---

## 故障排除

### 问题 1: Mock 模式下测试失败

**症状：** 即使没有 `.env` 文件，测试仍然失败

**可能原因：**
- 环境变量被外部设置（如 shell 配置文件）
- 之前运行留下的环境变量

**解决方法：**

```bash
# 清除环境变量
unset LLM_PROVIDER
unset LLM_API_KEY
unset ARK_API_KEY
unset OPENAI_API_KEY
unset ANTHROPIC_API_KEY

# 重新运行测试
cargo test --workspace
```

### 问题 2: Real LLM 模式超时

**症状：** `test_e2e_chat_008_performance_benchmark` 超时失败

**原因：** 真实 LLM API 响应时间过长或网络不稳定

**解决方法：**

1. **检查网络连接：**
   ```bash
   # 测试 API 连接
   curl -I https://api.openai.com/v1/models  # OpenAI
   curl -I https://ark.cn-beijing.volces.com  # 火山引擎
   ```

2. **检查 API 配额：**
   - 登录 LLM 提供商控制台
   - 查看 API 配额使用情况
   - 确认请求速率限制

3. **调整超时时间：**（如果网络环境特殊）
   ```rust
   // apps/cyberclaw-server/tests/e2e_chat_completion_test.rs:319-323
   let timeout_secs = if std::env::var("LLM_PROVIDER").is_ok_and(...) {
       300  // 增加到 5 分钟
   } else {
       60
   };
   ```

### 问题 3: 502 Bad Gateway 错误

**症状：** `test_e2e_int_004_complete_user_journey` 在阶段 4 或 8 失败，错误 502

**原因：**
- LLM API 调用失败（网络超时、配额耗尽、服务不可用）
- 连续多次 LLM 调用触发速率限制
- 上游服务响应时间过长

**解决方法：**

1. **查看详细错误信息：**
   ```bash
   cargo test test_e2e_int_004_complete_user_journey -- --nocapture
   ```

2. **检查 LLM API 状态：**
   - OpenAI: https://status.openai.com/
   - Anthropic: https://status.anthropic.com/
   - 火山引擎方舟：控制台查看服务状态

3. **增加延迟避免速率限制：**
   ```rust
   // 已在修复中添加
   if is_real_llm {
       tokio::time::sleep(Duration::from_millis(1000)).await;  // 增加到 1 秒
   }
   ```

4. **暂时跳过该测试：**
   ```bash
   # 运行除该测试外的所有测试
   cargo test --workspace -- --skip test_e2e_int_004_complete_user_journey
   ```

### 问题 4: API 密钥无效

**症状：** 测试失败，错误信息包含 "Unauthorized" 或 "Invalid API Key"

**解决方法：**

1. **验证 API 密钥格式：**
   ```bash
   # 检查 .env 文件
   cat .env | grep API_KEY

   # OpenAI 密钥格式：sk-proj-...
   # Anthropic 密钥格式：sk-ant-...
   # 火山引擎 ARK 密钥格式：自定义字符串
   ```

2. **重新生成 API 密钥：**
   - 登录对应 LLM 提供商控制台
   - 删除旧密钥并生成新密钥
   - 更新 `.env` 文件

3. **检查权限范围：**
   - 确认 API 密钥有 Chat Completions 权限
   - 确认模型访问权限（如 GPT-4）

---

## CI/CD 配置建议

### GitHub Actions 示例

```yaml
name: Test Suite

on: [push, pull_request]

jobs:
  test-mock:
    name: Mock Mode Tests (Fast)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - name: Run tests in Mock mode
        run: cargo test --workspace
        env:
          # 确保使用 Mock 模式
          LLM_PROVIDER: mock

  test-real-llm:
    name: Real LLM Integration Tests (Slow)
    runs-on: ubuntu-latest
    # 仅在 main 分支或包含 '[test-real-llm]' 的 commit 中运行
    if: github.ref == 'refs/heads/main' || contains(github.event.head_commit.message, '[test-real-llm]')
    steps:
      - uses: actions/checkout@v3
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - name: Run tests with Real LLM
        run: cargo test --workspace
        env:
          # 使用 GitHub Secrets 配置真实 LLM
          LLM_PROVIDER: ${{ secrets.LLM_PROVIDER }}
          OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}
          # 或其他提供商密钥
```

### GitLab CI 示例

```yaml
stages:
  - test-fast
  - test-integration

test-mock:
  stage: test-fast
  image: rust:latest
  script:
    - cargo test --workspace
  variables:
    LLM_PROVIDER: "mock"

test-real-llm:
  stage: test-integration
  image: rust:latest
  only:
    - main
    - tags
  script:
    - cargo test --workspace
  variables:
    LLM_PROVIDER: $LLM_PROVIDER
    OPENAI_API_KEY: $OPENAI_API_KEY
```

### 本地开发建议

**日常开发（快速反馈）：**

```bash
# 使用 Mock 模式
rm -f .env
cargo test --workspace

# 或使用 watch 模式自动重跑
cargo watch -x "test --workspace"
```

**上线前验证（完整集成测试）：**

```bash
# 配置真实 LLM
cp .env.example .env
# 编辑 .env 配置 LLM_PROVIDER 和 API 密钥

# 运行完整测试套件
cargo test --workspace

# 验证关键 E2E 测试
cargo test test_e2e_chat_008_performance_benchmark -- --nocapture
cargo test test_e2e_int_004_complete_user_journey -- --nocapture
```

---

## 测试统计

### Mock 模式（主环境）

```
Total Tests: 1457
├─ Passed: 1454
├─ Failed: 0
└─ Ignored: 3 (doctests)

Execution Time: ~2-5 minutes
Stability: ⭐⭐⭐⭐⭐ (5/5)
```

### Real LLM 模式（验证环境）

```
Total Tests: 1457
├─ Passed: 1452-1455
├─ Failed: 0-2 (E2E tests, environment-dependent)
└─ Ignored: 3 (doctests)

Execution Time: ~5-15 minutes
Stability: ⭐⭐⭐⭐ (4/5)

Potential Failures:
- test_e2e_chat_008_performance_benchmark (timeout)
- test_e2e_int_004_complete_user_journey (502 error)
```

---

## 常见问题 (FAQ)

### Q1: 为什么需要两种测试模式？

**A:** Mock 模式用于快速反馈和日常开发，Real LLM 模式用于上线前验证和集成测试。两者结合确保代码质量和生产环境兼容性。

### Q2: Mock 模式是否足够？

**A:** Mock 模式覆盖了大部分功能测试，但无法验证与真实 LLM 服务的集成。建议：
- 日常开发：Mock 模式
- PR 合并前：Real LLM 模式（至少运行关键 E2E 测试）
- 发布前：完整 Real LLM 模式测试

### Q3: Real LLM 模式下为什么有测试失败？

**A:** Real LLM 模式依赖外部 API，受以下因素影响：
- 网络延迟和稳定性
- LLM API 服务可用性
- API 配额和速率限制
- 地理位置和 CDN

如果测试在 Mock 模式下全部通过，Real LLM 模式下偶尔失败是正常的。建议重试 1-2 次。

### Q4: 如何在 CI 中配置 API 密钥？

**A:**
- GitHub: 使用 Repository Secrets
- GitLab: 使用 CI/CD Variables (Masked)
- 本地: 使用 `.env` 文件（确保 `.env` 在 `.gitignore` 中）

**永远不要将 API 密钥提交到代码仓库！**

### Q5: 测试失败是否意味着代码有问题？

**A:** 不一定。分两种情况：
- **Mock 模式失败**：通常是代码问题，需要修复
- **Real LLM 模式失败**：可能是环境问题（网络、API 服务），需要诊断

---

## 相关文档

- [.env.example](../../.env.example) - 环境变量配置示例
- [.env.test](../../.env.test) - 测试环境配置模板
- [test_e2e_chat_completion_test.rs](tests/e2e_chat_completion_test.rs) - Chat 完成 E2E 测试
- [test_e2e_integration_test.rs](tests/e2e_integration_test.rs) - 集成场景 E2E 测试
- [common/mod.rs](tests/common/mod.rs) - 测试基础设施

---

## 更新日志

| 日期 | 版本 | 更新内容 |
|------|------|---------|
| 2026-03-29 | v1.0 | 初始版本：添加 Mock / Real LLM 模式说明，修复 E2E 测试超时和 502 错误问题 |

---

**最后更新**: 2026-03-29
**维护者**: CyberClaw Server Team
