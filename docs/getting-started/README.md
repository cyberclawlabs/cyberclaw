# Getting Started

如果你第一次接触 CyberClaw，从这里开始。

## 这组文档帮你完成什么

它主要解决三件事：

1. 安装运行所需的最小前提
2. 用最短路径确认 CyberClaw 已经能跑
3. 跑通之后进入正确的下一步文档

## 阅读顺序

1. [Installation](installation.md) — 拿到代码 + 编译二进制 + 编译 Web UI
2. [Quickstart](quickstart.md) — 5 分钟跑起 server + CLI 端到端调用
3. [Deployment](deployment.md) — 生产部署（systemd / Podman / K8s + 环境变量契约）
4. [User Guide](../user-guide/README.md)

## 最短路径

如果你只想用最短时间确认仓库入口正常：

```bash
git clone https://github.com/cyberclawlabs/cyberclaw.git
cd cyberclaw
cargo build
cargo run -p cyberclaw-cli -- status
```

跑通后继续读 [Quickstart](quickstart.md)。

## 核心认知

CyberClaw 是一个可治理的 Agent 平台，强调：

- 受控执行
- 治理与审批边界
- 审计与追踪
- Skill / Connector / Platform Plugin 的扩展能力

Web3 是当前最强的落地场景，但 CyberClaw 不只服务于 Web3。

## 先这样理解它的使用方式

CyberClaw 不是让模型直接拿到一堆外部工具去执行，而是把真实动作组织成治理链。

- Web3：先分析金库、多签或链上事件，再走审批、策略和执行留痕
- DevOps：先形成发布或排障提案，再通过 GitHub、Slack 等受控接入面执行
- 安全：先收集 trace、日志和上下文，再决定是否允许后续响应动作
