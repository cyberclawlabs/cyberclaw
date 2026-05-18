# cyberclaw-control-plane

- Status: Active
- Scope: Crate
- Owner: CyberClaw Control Plane Maintainers
- Last Updated: 2026-04-11

`cyberclaw-control-plane` 是 CyberClaw 的控制平面 crate，负责包加载、注册、解析、编排、执行状态流转，以及多节点相关的基础控制能力。

## 当前职责

### 包与生态管理

- `loader.rs`：清单加载与基础安全校验
- `ecosystem_scanner.rs`：扫描 `ecosystem/` 包
- `registry.rs`：包记录与能力索引
- `resolver.rs`：运行时选择与计划生成

### 控制平面主链

- `gateway_router.rs`：入口归一化
- `task_manager.rs`：任务管理
- `case_manager.rs`：案例管理
- `review_queue.rs`：审批队列
- `review_gate.rs`：审查门控触发器 ⭐ 新增
- `subagent_scheduler.rs`：子代理派生与预算控制
- `automation.rs`：自动化协调
- `orchestrator.rs`：主编排入口
- `execution_service.rs`：执行状态与执行链调度

### Autopilot V2 运行时 ⭐ 新增

- `autopilot_types.rs`：Autopilot 状态模型与类型定义
- `autopilot_runtime.rs`：9 步 GovernedLoop 核心循环引擎
- `autopilot_progress.rs`：进度追踪与报告
- `autopilot_iteration.rs`：迭代历史与无进展检测
- `autopilot_state_sync.rs`：ExecutionService ↔ StateStore 状态同步
- `autopilot_security.rs`：Capability 白名单与 Prompt 注入检测
- `autopilot_workspace.rs`：工作空间边界检查

### Auto Mode Gate ⭐ 新增

- `auto_mode_gate.rs`：Autopilot 权限动态收窄（进入时快照权限，退出时恢复）
- `circuit_breaker.rs`：连续失败熔断器（Closed→Open→HalfOpen 状态机）

### 多节点基础能力

- `membership_service.rs`：节点成员管理
- `placement_engine.rs`：执行放置
- `lease_manager.rs`：租约与重分配
- `shared_state_store.rs`：共享状态与 CAS
- `event_bus.rs`：事件总线
- `artifact_store.rs`：工件存储

## 使用说明

本 crate 是工作区内部核心 crate，通常通过：

- `apps/cyberclaw-server`
- `apps/cyberclaw-cli`
- 其他工作区 crate

进行调用。

## 开发与验证

常用命令：

```bash
cargo test -p cyberclaw-control-plane
cargo clippy -p cyberclaw-control-plane --all-targets -- -D warnings
cargo test -p cyberclaw-control-plane --test integration_test
cargo test -p cyberclaw-control-plane --test multi_node_integration

# Autopilot V2 测试
cargo test -p cyberclaw-control-plane --test autopilot_integration_test
cargo test -p cyberclaw-control-plane --test autopilot_e2e_test
cargo test -p cyberclaw-control-plane --test autopilot_performance_test
cargo test -p cyberclaw-control-plane --test autopilot_security_test
```

## 相关文档

- [仓库根 README](../../README.md)
- [文档总索引](../../docs/INDEX.md)
- [架构总览](../../docs/architecture/overview/ARCHITECTURE_V2.0.md)
- [Runtime Blueprint](../../docs/architecture/runtime/RUNTIME_BLUEPRINT_V2.0.md)
- [仓库级 Changelog](../../CHANGELOG.md)
- [crate 级 Changelog](CHANGELOG.md)

## 维护规则

1. 这里说明的是 crate 局部职责，不重复仓库级路线图全文。
2. crate 级显著变更记录写入本目录 `CHANGELOG.md`。
3. 如果 crate 边界变化，需同步更新本文件和相关 `docs/` 文档。
