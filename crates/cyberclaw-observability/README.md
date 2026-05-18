# cyberclaw-observability

- Status: Active
- Scope: Crate
- Owner: CyberClaw Maintainers
- Last Updated: 2026-04-14

`cyberclaw-observability` 提供 CyberClaw 平台的可观测性基础设施。

## 核心模块

### Tracing (`tracing.rs`)

- Execution / Agent / Capability / Skill / Review / StatusTransition span 创建
- SpanGuard RAII 模式防止 span 泄漏
- 8 个测试用例通过

### Metrics (`metrics.rs`)

- 并发安全的指标记录
- 死锁防护测试

### Security Event Store (`security_event_store.rs`)

- 安全事件的内存存储与查询
- 多维过滤：actor / event_type / execution_id
- FIFO 容量淘汰策略
- 14 个测试用例通过

### OTel Exporter (`otel_exporter.rs`)

- OpenTelemetry 兼容的导出 stub

### Token Economics (新增 2026-04-14)

- `token_economics.rs`：执行级 Token 用量追踪与经济分析
  - `TokenRecord`：单次执行的 input/output/filtered token 计数 + 节省率 + 耗时
  - `TokenSummary`：聚合统计（总量、节省率、平均值）
  - `TokenTracker` trait：可插拔存储后端（async + Send + Sync）
  - `InMemoryTokenTracker`：开发/测试用内存实现（Arc<RwLock<Vec>>）
  - `TimedExecution`：计时辅助器，自动填充 duration
  - 5 维聚合：ByAgent / ByConnector / ByCapability / ByProject / ByDay
  - 时间范围过滤 + 过期数据清理
  - 12 个测试用例通过

## 维护规则

1. 本文件说明 crate 局部职责，不重复仓库级路线图全文。
2. 显著变更记录写入仓库级 `CHANGELOG.md` 的相关章节。
3. 如果 crate 边界变化，需同步更新本文件和相关 `docs/` 文档。
