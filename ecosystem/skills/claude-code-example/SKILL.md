---
name: Claude Code Example Skill
version: 1.0.0
description: Example skill demonstrating Claude Code format
author: CyberClaw Team
homepage: https://cyberclaw.io
tags:
  - example
  - claude-code
  - demonstration
---

# Claude Code Example Skill

这是一个 Claude Code 格式的示例 Skill，用于演示 CyberClaw Skill Loader 的功能。

## 📋 功能描述

此 Skill 提供以下功能：

### 1. 文本处理

- 文本大小写转换
- 字符统计
- 单词分析

### 2. 数据验证

- 格式验证
- 类型检查
- 约束验证

## 🚀 使用方法

### 基本示例

```json
{
  "action": "uppercase",
  "text": "hello world"
}
```

### 响应示例

```json
{
  "result": "HELLO WORLD",
  "status": "success"
}
```

## 🔧 配置选项

| 选项 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `case_sensitive` | boolean | true | 是否区分大小写 |
| `trim_whitespace` | boolean | true | 是否删除前后空格 |

## 📊 支持的操作

- `uppercase` - 转换为大写
- `lowercase` - 转换为小写
- `count` - 字符计数
- `validate` - 数据验证

## 🧪 测试

```bash
# 运行测试
./scripts/test.sh
```

## 📝 更新日志

### v1.0.0 (2026-03-23)
- 🎉 初始版本发布
- ✨ 支持基本文本处理
- ✨ 支持数据验证
