# CyberClaw 文档索引

- Status: Active
- Scope: Docs
- Owner: CyberClaw Maintainers
- Last Updated: 2026-05-23

这是 CyberClaw 文档体系的总索引。

## ★ v1.2 业务对等胜出 (2026-05-23)

- [v1.1 Final Ship](implementation/release/v1.1-final-ship-2026-05-23.md) — cb 反超 hm，4 GAP 全闭合，v1.1 发布判定
- [v1.2 Final Ship](implementation/release/v1.2-final-ship-2026-05-23.md) — 5 个 P 项全落地，cb 80% > hm 75.5%（+4.5pp），v1.2 发布判定
- [v1.2 Backlog](implementation/release/v1.2-backlog-2026-05-23.md) — v1.2 Sprint 待办清单与 P1–P5 执行计划

## ★ v1.0.0 GA（2026-05-15）

- [GA Final Readiness](implementation/reports/v1.0-ga-final-readiness-2026-05-15.md) — 14 个发布门 / 133/137 = 97.1% PASS / 0 阻断
- [Hermes Parity Matrix](implementation/reports/v1.0-ga-parity-matrix.md) — 业务对等 87% + 治理 4 维超越
- [Safety Test Matrix](implementation/reports/v1.0-ga-safety-matrix.md) — 25 vectors 全景
- [**v1.1 Backlog**](implementation/reports/v1.1-backlog.md) — 下一版本待办（24 项 + 16 项已完成）
- [Release Records](implementation/releases/README.md) — v1.0.0 release entry

## 0. 仓库级规则与入口

1. [项目根 README](../README.md)
2. [文档中心 README](README.md)
3. [文档管理体系](../DOCUMENTATION_SYSTEM.md)
4. [文档元信息模板](templates/DOCUMENT_METADATA_TEMPLATE.md)
5. [Claude 项目记忆](../CLAUDE.md)
6. [Agent 执行规范](../AGENTS.md)
7. [测试脚本说明](../scripts/testing/README.md)

## 1. 首次访问者入口

1. [项目根 README](../README.md)
2. [文档门户](README.md)
3. [Getting Started](getting-started/README.md)
4. [User Guide](user-guide/README.md)
5. [Builder Guide](builders/README.md)
6. [Web3 Guide](web3/README.md)
7. [Security & Governance](security/README.md)
8. [Reference](reference/README.md)

## 2. 按角色阅读路径

### 2.1 架构师

1. [架构目录 README](architecture/README.md)
2. [架构总览](architecture/overview/ARCHITECTURE_V2.0.md)
3. [核心类型](architecture/overview/CORE_TYPES_V2.0.md)
4. [Runtime Blueprint](architecture/runtime/RUNTIME_BLUEPRINT_V2.0.md)
5. [CyberClaw Autopilot Architecture](architecture/runtime/CYBERCLAW_AUTOPILOT_ARCHITECTURE_V1.md)
6. [Web3 Connector Pack Architecture](architecture/runtime/WEB3_CONNECTOR_PACK_V1.md)
7. [CyberClaw HFT Control Plane Architecture](architecture/runtime/CYBERCLAW_HFT_CONTROL_PLANE_ARCHITECTURE_V1.md)
8. [治理与控制 README](architecture/governance/README.md)
9. [自我学习治理架构方案](architecture/governance/SELF_LEARNING_GOVERNANCE_ARCHITECTURE_V1.md)
10. [记忆与上下文工程 README](architecture/memory/README.md)
11. [记忆架构复核决议（Beta 口径）](architecture/memory/MEMORY_ARCH_REVIEW_DECISION_V1.md)
12. [知识检索 README](architecture/retrieval/README.md)
13. [Letta / Zep / PageIndex Connector 策略](architecture/retrieval/LETTA_ZEP_PAGEINDEX_CONNECTOR_STRATEGY_V1.md)
14. [OpenViking 默认外接记忆架构方案](architecture/retrieval/OPENVIKING_DEFAULT_EXTERNAL_MEMORY_ARCHITECTURE_V1.md)
15. **[Skill/Tool 兼容性架构设计 v1](architecture/overview/SKILL_TOOL_COMPATIBILITY_V1.md)** -- 外部接入面规范

### 2.2 平台开发者

1. [实施目录 README](implementation/README.md)
2. [开发路线图](implementation/roadmap/DEVELOPMENT_ROADMAP_V2.0.md)
3. [Beta 缺口计划](implementation/roadmap/BETA_GAP_PLAN_V1.md)
4. [下一阶段开发路线图 V2](implementation/roadmap/NEXT_STAGE_ROADMAP_V2.md)
5. **[Development Plan V3（6-Sprint / 38-task）](implementation/roadmap/DEVELOPMENT_PLAN_V3.md)** ⭐ 当前计划（已完成）
6. [Autopilot 完整实施计划 V2（16-20周）](implementation/roadmap/AUTOPILOT_IMPLEMENTATION_PLAN_V2.md)
6. [~~Autopilot PoC 计划（已废弃）~~](../.omc/plans/AUTOPILOT_POC_PLAN.md)
7. [~~Autopilot 实施计划 V1（已废弃）~~](implementation/roadmap/AUTOPILOT_IMPLEMENTATION_PLAN_V1.md)
8. [自我学习治理 Issue 清单](implementation/roadmap/SELF_LEARNING_GOVERNANCE_ISSUES_V1.md)
9. [执行 Prompt README](implementation/prompts/README.md)
10. [自我学习治理实现 Prompt](implementation/prompts/2026-03-21-self-learning-governance-implementation-prompt.md)
11. [技术调研 README](implementation/research/README.md)
12. **[业务能力对标 Hermes-agent (2026-05-04)](implementation/reports/business-test-list-vs-hermes-2026-05-04.md)** ⭐ 40/40 ✅ (100%)
13. **[发布门禁报告](implementation/reports/release-gate-report.md)** ⭐ 当前门禁基线
14. **[缺口地图](implementation/reports/gap-catalog.md)** ⭐ 当前缺口基线
15. **[Phase 3.9/4 续跑复核](implementation/reports/2026-03-30-phase39-phase4-continuation.md)** ⭐ 最新续跑证据
16. **[架构深度验证](implementation/reports/2026-03-30-architecture-deep-verification.md)** ⭐
17. **[生产就绪评估](implementation/reports/production-readiness-assessment-2026-03-30.md)** ⭐
18. [测试状态验证](implementation/reports/2026-03-29-test-status-verification.md)
19. [Host 工具能力收敛（去 echo + 真实 skill invoke）](implementation/reports/2026-04-06-host-tool-runtime-closure.md)
20. [Claude-first 工具带 Phase 3 清单与执行证据](implementation/reports/2026-04-07-claude-toolbelt-phase3-checklist.md)
21. [Autopilot Milestone 0 架构对齐评审](implementation/reports/2026-03-22-autopilot-milestone0-architecture-review.md)
22. [Autopilot Milestone 1 完成报告](implementation/reports/2026-03-22-autopilot-milestone1-completion-report.md)
23. [实现报告 README](implementation/reports/README.md)
23. [评审文档 README](implementation/reviews/README.md)
24. [修复记录 README](implementation/fixes/README.md)
    - [修复记录](implementation/fixes/2026-03-21-fixes.md)
25. [安全文档 README](implementation/security/README.md)
    - [安全修复状态](implementation/security/2026-03-21-security-fixes-status.md)
26. [Code Maps 索引](architecture/codemaps/INDEX.md)
27. [Control Plane Crate README](../crates/cyberclaw-control-plane/README.md)
28. **[Ralph Closed-Loop Design Spec](superpowers/specs/2026-04-14-ralph-closed-loop-design.md)** ⭐ 新增
29. **[ACP External Agent Runtime Connector Design](superpowers/specs/2026-04-14-acp-external-agent-runtime-design.md)** ⭐ 新增
30. **[IM Voice Control Surface Design](superpowers/specs/2026-04-14-im-voice-control-surface-design.md)** ⭐ 新增

### 2.3 安全与治理负责人

1. [架构总览](architecture/overview/ARCHITECTURE_V2.0.md)
2. [Runtime Blueprint](architecture/runtime/RUNTIME_BLUEPRINT_V2.0.md)
3. [M2 Governance Core 架构设计](architecture/governance/M2_GOVERNANCE_ARCHITECTURE.md)
4. [自我学习治理架构方案](architecture/governance/SELF_LEARNING_GOVERNANCE_ARCHITECTURE_V1.md)
5. [记忆压缩与上下文收敛策略](architecture/memory/MEMORY_COMPACTION_STRATEGY_V1.md)
6. [Milestone C 安全修复](implementation/fixes/SECURITY_FIXES_MILESTONE_C.md)
7. **[Beta Closure 安全审查完成报告](../.omc/SECURITY_REVIEW_COMPLETION_REPORT.md)** ⭐ 新增
8. **[Beta Closure 完成报告](../.omc/BETA_CLOSURE_COMPLETION_REPORT.md)** ⭐ 新增
9. [评审文档 README](implementation/reviews/README.md)

### 2.4 运维 / DevOps

1. **[部署指南](deployment/README.md)** ⭐ 新增
   - [本地开发部署](#本地开发部署)
   - [生产环境部署](#生产环境部署)
   - [Docker 部署指南](deployment/docker.md)
2. [Docker 部署详细指南](deployment/docker.md)
3. [故障排查手册](deployment/troubleshooting.md)
4. [安全检查清单](deployment/security-checklist.md)
5. [示例配置文件](deployment/examples/)
   - [.env.production 示例](deployment/examples/.env.production.example)
   - [docker-compose.yml 示例](deployment/examples/docker-compose.example.yml)
   - [nginx.conf 示例](deployment/examples/nginx.conf.example)
6. [环境变量配置](ENVIRONMENT_VARIABLES.md)

### 2.5 GTM / 运营

1. [业务目录 README](business/README.md)
2. [GTM README](business/gtm/README.md)
3. [GTM 执行计划](business/gtm/GTM_EXECUTION_PLAN_V1.md)
4. [GTM 成本执行清单](business/gtm/GTM_COST_EXECUTION_CHECKLIST_V1.md)
