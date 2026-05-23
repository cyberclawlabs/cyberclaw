# CyberClaw 性能基准数据

本文档记录 `cyberclaw-store` 关键路径的性能基准，作为 P95 SLA 的参考依据。

- **基准日期**: 2026-04-24
- **平台**: macOS Darwin 25.3.0
- **Rust 版本**: rustc (stable)
- **运行命令**: `cargo bench -p cyberclaw-store`

---

## InMemoryLeveledStore — 写入吞吐量

基准名称: `memory_store/store_leveled/{batch_size}`

测试场景：单线程连续写入 N 条 `LeveledMemoryRecord`（L1Summary 层级，含字符串 content），
使用 `Arc<InMemoryLeveledStore>` 模拟并发共享访问（RwLock 写路径）。

### 基准结果（100 samples，Criterion 0.5）

| 批量大小 | 耗时（mean） | 耗时范围（低—高） | 每条记录耗时 |
|----------|-------------|-------------------|--------------|
| 100      | 13.135 µs   | 13.101 — 13.174 µs | ~131 ns/op  |
| 1,000    | 135.30 µs   | 135.08 — 135.55 µs | ~135 ns/op  |
| 10,000   | 1.4177 ms   | 1.4156 — 1.4202 ms | ~142 ns/op  |

### 吞吐量换算

| 批量大小 | 吞吐量（ops/sec） |
|----------|------------------|
| 100      | ~7,614,000 ops/s |
| 1,000    | ~7,390,000 ops/s |
| 10,000   | ~7,054,000 ops/s |

### 线性度分析

写入耗时与批量大小呈良好线性关系（R² ≈ 1.0），单条记录写入成本约 **130–142 ns**，
说明 `RwLock<HashMap>` 在无并发竞争时性能稳定，无异常拷贝或分配放大。

### 异常值

- size=100：9% 异常值（3 low mild, 5 high mild, 1 high severe）
- size=1000：5% 异常值（4 high mild, 1 high severe）
- size=10000：3% 异常值（3 high severe）

异常值主要源于操作系统调度抖动，属正常范围。

---

## SLA 参考

基于上述基准，`InMemoryLeveledStore` 的写入路径满足：

| 指标 | 值 |
|------|-----|
| P50 单写延迟 | ~135 ns |
| P95 单写延迟 | < 200 ns（含异常值） |
| 10k 批写延迟 | < 1.5 ms |
| 吞吐上限（单线程） | ~7M ops/s |

> **注意**: 以上数据为 `InMemoryLeveledStore` 的内存路径性能。
> SQLite 后端（`SqliteLeveledStore`）受磁盘 I/O 约束，性能差异预计 100–1000x，
> 具体取决于存储介质和 WAL 同步模式。

---

## 复现方法

```bash
# 在项目根目录运行
cargo bench -p cyberclaw-store

# 仅运行特定基准
cargo bench -p cyberclaw-store -- memory_store/store_leveled

# 生成 HTML 报告（需安装 gnuplot）
cargo bench -p cyberclaw-store -- --output-format html
# 报告位于: target/criterion/memory_store/report/index.html
```

---

## 相关文档

- [生产部署指南](DEPLOY.md)
- [存储层源码](../../crates/cyberclaw-store/src/memory_store.rs)
- [基准源码](../../crates/cyberclaw-store/benches/memory_write_bench.rs)
