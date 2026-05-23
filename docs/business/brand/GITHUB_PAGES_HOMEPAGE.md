# GitHub Pages Homepage Copy

## Positioning

CyberClaw is a governable Agent platform for high-stakes, real-world systems.

## I18N Requirement

Homepage must be designed as localized content, not a single-language landing page.

### Launch Locales

- `en`
- `zh-CN`

### Expansion Locales

- `ja`
- `ko`
- `es`

### Required UX Elements

- visible language switcher
- locale-aware navigation
- localized CTA labels
- `hreflang` metadata in page output

## Hero

### Title

Governable Agent Infrastructure for Real-World Systems

### Subtitle

Build and operate agent workflows with controlled execution, policy, auditability, and clear system boundaries.

### Supporting Line

CyberClaw is a general platform, with Web3 as the strongest current deployment surface and security operations as a natural second anchor.

### Primary CTA

Read the Docs

### Secondary CTAs

- Explore GitHub
- Browse Skill Hub

## Page Structure

### 1. Why CyberClaw

首页第一屏不应只解释“平台是什么”，而应先说明它让什么团队变得更强：

- 安全团队可以用 AI 组织代码审计、告警分诊、事件处置和审计追踪
- AI 团队可以把 Agent 从 Demo 带到真实流程
- 精简团队可以用更少的人构建接近“一人安全中心”的工作方式

### 2. Why Governance Matters

当 Agent 不只是聊天，而是真实触发系统动作、资产操作和跨系统执行时，治理不再是附属层，而是执行主链的一部分。

### 3. Platform Building Blocks

- Agent
- Skill
- Connector
- Capability
- Platform Plugin

### 4. Execution and Control

页面应强调：

- 受控执行
- 策略与审批
- 审计与追踪
- 清晰扩展边界
- 风险与隔离分层

### 5. Web3 as Current Strong Use Case

Web3 不是 CyberClaw 的唯一定位，但它最能体现平台在高风险自动化环境中的价值。

页面应给出具体场景，而不是只写抽象定位，例如：

- treasury / multisig operation pipeline
- signer-gated on-chain runbook
- protocol governance and operator workflow
- on-chain incident escalation and audit trail

首页文案应至少给出一条完整链路示意，例如：

- observe risk or treasury context -> assemble proposal -> approvals/policy gate -> execute through wallet connector -> record artifacts and audit trail

### 6. Other High-Stakes Scenarios

首页应同时给出非 Web3 的高价值工作流，证明 CyberClaw 不是单一垂直工具，例如：

- code audit / PR risk review
- alert triage and escalation
- incident response and follow-up
- release gate and rollback proposal
- governed database migration

这里也应至少给出一到两条完整链路，而不是只列分类名，例如：

- security alert -> collect traces and context -> classify and escalate -> require approval for risky follow-up -> record actions and artifacts
- release request -> build governed checklist -> open issue or PR through connector -> notify operators in Slack -> preserve execution trace

### 7. Builder Ecosystem

引导访客进入：

- Docs
- Builder Guide
- Skill Hub

### 8. Status Framing

页面中所有能力表达必须按三层状态写：

- Implemented
- In Progress
- Roadmap

## Localization Rule

首页文案应以 English canonical copy 为主源，并维护简体中文对齐版本。新增语种必须继承同一状态口径和术语表。

## Tone

首页不应写成：

- 玩具型 agent demo 页面
- 空泛 AI 营销页
- 只堆技术细节的架构报告

首页应呈现：

- 工业级
- 克制但有力度
- 安全与治理优先
- 对 builder 友好
