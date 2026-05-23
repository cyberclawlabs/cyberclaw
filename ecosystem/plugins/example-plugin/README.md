# Example CyberClaw Plugin

这是一个示例 CyberClaw Plugin，演示了 Plugin 开发的最佳实践。

## 功能特性

- ✅ 完整的 Hook 生命周期实现
- ✅ 参数验证和修改
- ✅ 审计日志记录
- ✅ 失败处理和重试
- ✅ 健康检查
- ✅ 指标收集

## 项目结构

```
example-plugin/
├── Cargo.toml              # Rust 项目配置
├── cyberclaw-plugin.toml   # Plugin 清单
├── src/
│   └── lib.rs             # Plugin 实现
├── tests/                  # 测试文件
└── README.md              # 本文件
```

## 构建方法

```bash
# 开发构建
cargo build

# 发布构建
cargo build --release

# 运行测试
cargo test
```

## 安装方法

```bash
# 使用 CyberClaw CLI 安装
cyberclaw plugin install ./cyberclaw-plugin.toml

# 启用 Plugin
cyberclaw plugin enable example-plugin

# 验证安装
cyberclaw plugin list
```

## Hook 说明

### BeforeExecution

在执行开始前触发，用于：
- 参数验证
- 审计日志
- 注入额外参数

### AfterExecution

在执行完成后触发，用于：
- 结果处理
- 资源清理
- 性能统计

### OnFailure

在执行失败时触发，用于：
- 错误处理
- 失败通知
- 重试决策

### OnReview

在需要审核时触发，用于：
- 风险评估
- 合规检查
- 审批流程

## 配置说明

### 权限要求

```toml
[plugin.capabilities]
required = ["fs.read", "network.http", "env.read"]
```

### 资源限制

```toml
[plugin.resources]
memory = 104857600  # 100 MB
cpu_ms = 10000      # 10 seconds
```

## 开发指南

### 添加新 Hook

1. 创建 Handler 结构体：

```rust
struct MyHookHandler;

#[async_trait]
impl HookHandler for MyHookHandler {
    async fn handle(&self, context: &HookContext) -> Result<HookOutput> {
        // 实现逻辑
    }
}
```

2. 注册 Hook：

```rust
let hook = HookRegistration {
    plugin_id: "example-plugin".to_string(),
    hook_type: HookType::Custom("my_hook"),
    handler: Arc::new(MyHookHandler),
    priority: 100,
    timeout_ms: 5000,
};
```

### 状态管理

使用 `Arc<RwLock<T>>` 管理共享状态：

```rust
struct PluginState {
    counter: u64,
}

let state = Arc::new(RwLock::new(PluginState { counter: 0 }));
```

### 错误处理

```rust
use anyhow::{Result, Context};

fn risky_operation() -> Result<()> {
    something_that_might_fail()
        .context("Failed to perform operation")?;
    Ok(())
}
```

## 测试

### 单元测试

```bash
cargo test --lib
```

### 集成测试

```bash
cargo test --test integration
```

## 性能优化

- 使用异步操作避免阻塞
- 实施适当的缓存策略
- 避免在 Hook 中执行重操作
- 使用连接池管理外部连接

## 故障排除

### Plugin 加载失败

确保：
- `cyberclaw_plugin_init` 函数正确导出
- 依赖库版本兼容
- Manifest 格式正确

### Hook 超时

- 检查 timeout_ms 配置
- 优化 Hook 处理逻辑
- 使用异步操作

## 许可证

Apache 2.0 License

## 支持

- 文档：https://docs.cyberclaw.io/plugins
- 论坛：https://forum.cyberclaw.io
- Issues：https://github.com/cyberclaw/example-plugin/issues