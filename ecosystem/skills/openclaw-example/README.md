# OpenClaw Example Skill

这是一个 OpenClaw 格式的示例 Skill，使用 `skill.toml` 进行配置。

## 特性

- TOML 格式配置
- 自动化工作流
- 可扩展处理器

## 快速开始

```toml
[config]
enable_logging = true
timeout = 300
```

## 执行

```json
{
  "workflow": "process",
  "params": {...}
}
```

## 文档

详见 OpenClaw 官方文档。
