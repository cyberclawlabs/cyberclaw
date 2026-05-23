# 控制平面架构

**最后更新:** 2026-03-28
**包路径:** `crates/cyberclaw-control-plane/`
**入口点:** `src/lib.rs`
**状态:** ✅ P0 Task 1 已完成并加固，P0-1 Phase 2 已完成

## 模块结构

```
cyberclaw-control-plane/src/
├── lib.rs                 # 库入口，导出公共 API
├── orchestrator.rs        # 控制平面协调器 [P0 Task 1 ✓]
├── gateway_router.rs      # 请求网关路由
├── resolver.rs            # Agent/Skill 解析器
├── registry.rs            # 包注册表接口
├── review_queue.rs        # 审批队列 [DoS 防护 ✓]
├── task_manager.rs        # 任务管理器 [容量限制 ✓]
├── case_manager.rs        # 案例管理器
├── execution_service.rs   # 执行服务 [持久化 ✓]
├── placement_engine.rs    # 节点放置引擎 [部分实现]
├── lease_manager.rs       # 分布式租约 [H-1 已修复]
├── membership_service.rs  # 集群成员管理 [配置验证]
├── event_bus.rs           # 事件发布订阅 [C-2 已修复]
├── artifact_store.rs      # 工件存储管理 [C-1, H-3 已修复]
├── subagent_scheduler.rs  # 子代理调度器 [配置验证]
├── shared_state_store.rs  # 共享状态管理 [H-4 已修复]
├── automation.rs          # 自动化作业管理
├── loader.rs              # 包加载器
└── ecosystem_scanner.rs   # 生态系统扫描器
```

## 核心组件

### 0. Orchestrator (控制平面协调器) 🎯
**文件:** `orchestrator.rs`
**状态:** ✅ P0 Task 1 已完成，P0-1 Phase 2 已完成

```
功能：协调整个控制平面主链路执行流程
├── process_ingress() - 处理入口请求
├── evaluate_risk() - 风险评估
├── submit_execution() - 提交执行任务
├── extract_placement_from_plan() - 提取放置需求
└── authorize_and_audit_api_call() - 轻量级 API 审计 [P0-1 Phase 2 ✓]

主链路流程（完整治理）：
Gateway → Resolver → RiskEval → ReviewGate → ExecutionService
  → PlacementEngine → LeaseManager → EventBus

轻量级审计路径（P0-1 Phase 2）：
authorize_and_audit_api_call() → SecurityEvent → LLM Client
  (绕过 PolicyEngine 和 ReviewQueue，提升性能)

安全修复 (v0.1.0):
✓ ExecutionService 集成完成 (HIGH)
✓ PlacementEngine runtime 提取 (HIGH)
✓ 移除未使用导入警告

P0-1 Phase 2 修复 (2026-03-28):
✓ 新增 authorize_and_audit_api_call() 方法
✓ 解决 H-4 空 actions 触发审核冲突
✓ 区分"治理型 API"vs"审计型 API"
✓ Chat API 响应时间减少 ~60% (移除轮询等待)
✓ 4 个新单元测试 (User/Anonymous/Service/System)
✓ 保留审计追踪（SecurityEvent），满足合规要求
```

### 1. ReviewQueue (审批队列) 🔒
**文件:** `review_queue.rs`
**安全修复:** DoS 防护

```
功能：高风险操作审批队列管理
├── enqueue() - 加入审批队列
├── list_pending() - 列出待审批项
├── approve() - 批准审批
└── reject() - 拒绝审批

安全特性 (v0.1.0):
✓ 容量限制 (默认: 1000)
✓ 达到容量拒绝新请求
✓ 防止无界队列 DoS 攻击
✓ 配置化容量 with_capacity(n)
```

### 2. TaskManager (任务管理器) 🔒
**文件:** `task_manager.rs`
**安全修复:** 容量限制 + 输入验证

```
功能：任务生命周期管理
├── create_task() - 创建新任务 [验证 ✓]
├── get_task() - 查询任务
└── list_tasks() - 列出所有任务

安全特性 (v0.1.0):
✓ 容量限制 (默认: 10000)
✓ 任务创建前验证 (via Task::validate())
✓ 防止任务堆积 DoS
✓ 配置化容量 with_capacity(n)
```

### 3. ExecutionService (执行服务) ✅
**文件:** `execution_service.rs`
**安全修复:** 持久化集成

```
功能：执行记录持久化管理
├── submit() - 提交执行记录 [修复 ✓]
├── submit_plan() - 提交计划执行
├── cancel() - 取消执行
├── get() - 查询执行记录
└── list() - 列出所有执行

修复详情 (v0.1.0):
✓ ExecutionRequest 接受预生成 execution_id
✓ submit() 使用提供的 ID 而非生成新 ID
✓ Orchestrator 正确调用 ExecutionService
✓ 执行记录成功持久化
```

### 4. PlacementEngine (放置引擎) 🚧
**文件:** `placement_engine.rs`
**状态:** 部分实现

```
功能：为执行任务选择合适节点
├── place() - 节点选择算法
├── matches_labels() - 标签匹配
├── matches_runtime() - 运行时匹配
└── matches_network_zone() - 网络区域匹配

实现状态 (v0.1.0):
✓ Agent runtime_requirements 提取
✓ 从 Registry 查询 agent 信息
✓ 最小负载节点选择
TODO: Capability placement 需求合并
TODO: 网络区域和秘钥需求
```

### 5. Registry (包注册表) 📦
**文件:** `registry.rs`

```
功能：Agent/Skill/Connector 包元数据管理
├── upsert() - 注册/更新包
├── get() - 查询包信息
├── list() - 列出所有包
└── activate() - 激活特定版本

用途：
- 提供 Agent 的 runtime_requirements
- 包版本管理
- Ecosystem 集成
```

### 6. Resolver (解析器) 🔍
**文件:** `resolver.rs`

```
功能：任务到 Agent/Skill 的解析
├── plan() - 创建执行计划
└── get_registry() - 获取注册表引用

输出：
- ExecutionPlan (Resolution + PlannedAction)
- 包含 agent, skills, capabilities, workflow
```

### 7. ArtifactStore (工件存储)
**文件:** `artifact_store.rs`
**安全修复:** C-1 (路径遍历), H-3 (TOCTOU竞态)

```
功能：管理执行工件的存储和检索
├── write_artifact() - 保存工件到磁盘
├── read_artifact() - 从磁盘读取工件
├── list_artifacts() - 列出所有工件
└── cleanup_old_artifacts() - 清理过期工件

安全特性：
✓ 路径规范化和边界验证
✓ 符号链接检测和拒绝
✓ 原子文件操作
✓ ID 验证防止路径遍历
```

### 2. EventBus (事件总线)
**文件:** `event_bus.rs`
**安全修复:** C-2 (无界通道 DoS)

```
功能：事件发布/订阅系统
├── publish() - 发布事件到所有订阅者
├── subscribe() - 订阅特定事件类型
└── unsubscribe() - 取消订阅

安全特性：
✓ 有界通道 (默认: 1000)
✓ 背压处理
✓ 配置验证 (buffer_size: 10-100,000)
```

### 3. LeaseManager (租约管理)
**文件:** `lease_manager.rs`
**安全修复:** H-1 (并发竞态条件)

```
功能：分布式租约管理
├── acquire_lease() - 获取执行租约
├── renew_lease() - 续约
├── release_lease() - 释放租约
└── is_leased() - 检查租约状态

安全特性：
✓ 原子 check-and-set 操作
✓ Entry API 防止 TOCTOU
✓ TTL 配置验证 (10-3600秒)
```

### 4. MembershipService (成员服务)
**文件:** `membership_service.rs`
**安全增强:** 配置验证

```
功能：集群成员管理
├── register_node() - 注册新节点
├── heartbeat() - 节点心跳
├── get_members() - 获取所有成员
└── evict_node() - 驱逐节点

配置验证：
✓ heartbeat_timeout: 5-300秒
✓ suspect_timeout: 10-600秒
✓ suspect_timeout > heartbeat_timeout
```

### 5. SubagentScheduler (调度器)
**文件:** `subagent_scheduler.rs`
**安全增强:** 资源限制

```
功能：子代理任务调度
├── schedule() - 调度任务
├── execute() - 执行任务
└── cancel() - 取消任务

资源限制：
✓ max_depth: 1-20 层
✓ max_steps: ≤ 100,000
✓ max_duration: ≤ 10分钟
✓ max_tokens: ≤ 1,000,000
✓ max_children: ≤ 100
```

### 11. SharedStateStore (共享状态)
**文件:** `shared_state_store.rs`
**安全修复:** H-4 (版本溢出)

```
功能：分布式共享状态
├── get() - 读取状态
├── set() - 设置状态
├── update() - 原子更新
└── compare_and_swap() - CAS 操作

安全特性：
✓ checked_add 防止溢出
✓ 乐观锁完整性
✓ 版本号检查
```

## 主链路架构 (P0 Task 1) 🎯

```
┌─────────────┐
│   Ingress   │  用户请求入口
└──────┬──────┘
       │
       ↓
┌─────────────┐
│   Gateway   │  1. 请求规范化和路由
└──────┬──────┘
       │
       ↓
┌─────────────┐
│  Resolver   │  2. 解析 Agent/Skills → ExecutionPlan
└──────┬──────┘     (查询 Registry 获取包信息)
       │
       ↓
┌─────────────┐
│  RiskEval   │  3. 风险评估 (Low/Medium/High/Critical)
└──────┬──────┘
       │
       ├─ High/Critical ───→ ┌──────────────┐
       │                     │ ReviewQueue  │  审批流程
       │                     └──────┬───────┘
       │                            │
       ↓                            ↓
┌──────────────────────────────────────┐
│       ExecutionService               │  4. 持久化执行记录
└──────┬───────────────────────────────┘
       │
       ↓
┌─────────────┐
│ Placement   │  5. 提取 placement 要求 → 选择节点
│  Engine     │     (从 Registry 获取 runtime_requirements)
└──────┬──────┘
       │
       ↓
┌─────────────┐
│LeaseManager │  6. 获取执行租约
└──────┬──────┘
       │
       ↓
┌─────────────┐
│  EventBus   │  7. 发布 ClusterEvent
└─────────────┘
```

## 数据流示例

### 完整控制平面执行流程 (P0)
```
IngressRequest
  → Gateway.normalize()
  → Resolver.plan() [查询 Registry]
  → Orchestrator.evaluate_risk()
  → [风险检查]
     ├─ Low → 直接提交
     └─ High → ReviewQueue.enqueue() → 等待审批
  → ExecutionService.submit(ExecutionRequest)  ✅ 持久化
  → Orchestrator.extract_placement_from_plan() ✅ 查询 Registry
  → PlacementEngine.place(placement)
  → LeaseManager.acquire_lease()
  → EventBus.publish(ExecutionScheduled)
  → 返回 IngressResponse
```

### 任务创建流程 (带验证)
```
Task
  → TaskManager.create_task()
  → Task.validate() ✅ 输入验证
     ├─ title: 1-255 字符，无控制字符
     ├─ summary: 0-2000 字符
     └─ labels: 每个 1-100 字符
  → [容量检查] ✅ 防 DoS
     └─ 已用 < max_capacity (10000)
  → 插入存储
  → 返回 Task
```

### 审批队列流程 (带容量限制)
```
ReviewRequest
  → ReviewQueue.enqueue()
  → [容量检查] ✅ 防 DoS
     └─ 队列长度 < max_capacity (1000)
  → 加入队列
  → 等待审批
     ├─ approve() → 继续执行
     └─ reject() → 拒绝执行
```

## 测试覆盖 (v0.1.0)

| 模块 | 单元测试 | 新增测试 | 状态 |
|------|----------|----------|------|
| **P0 主链路** | | | |
| Orchestrator | 7 | - | ✅ 完整 |
| ReviewQueue | 5 | +2 (容量) | ✅ 加固 |
| TaskManager | 8 | +5 (验证) | ✅ 加固 |
| ExecutionService | 3 | - | ✅ 集成 |
| PlacementEngine | 4 | - | 🚧 部分 |
| Registry | 2 | - | ✅ 完整 |
| Resolver | 4 | - | ✅ 完整 |
| **基础设施** | | | |
| ArtifactStore | 14 | - | ✅ 加固 |
| EventBus | 11 | - | ✅ 加固 |
| LeaseManager | 9 | - | ✅ 加固 |
| MembershipService | 12 | - | ✅ 加固 |
| SubagentScheduler | 11 | - | ✅ 加固 |
| SharedStateStore | 10 | - | ✅ 加固 |
| **其他** | | | |
| CaseManager | 3 | - | ✅ 完整 |
| Automation | 4 | - | ✅ 完整 |
| Loader | 5 | - | ✅ 完整 |
| EcosystemScanner | 4 | - | ✅ 完整 |
| GatewayRouter | 2 | - | ✅ 完整 |
| **总计** | **127** 单元 + **7** 集成 + **4** 多节点 = **138 测试** | | |

### 新增安全测试 (v0.1.0)
- `test_capacity_limit_enforced` - ReviewQueue 容量强制
- `test_capacity_limit_allows_within_limit` - ReviewQueue 容量内允许
- `test_capacity_limit_enforced` - TaskManager 容量强制
- `test_capacity_limit_allows_within_limit` - TaskManager 容量内允许
- `test_validation_rejects_empty_title` - 拒绝空 title
- `test_validation_rejects_title_too_long` - 拒绝超长 title
- `test_validation_rejects_control_characters` - 拒绝控制字符
- `test_validation_rejects_summary_too_long` - 拒绝超长 summary
- `test_validation_allows_valid_task` - 允许合法任务

## 性能特征

- **并发:** 使用 `Arc<RwLock>` 实现读写锁
- **异步:** 基于 Tokio 运行时
- **内存:** 有界队列防止内存泄漏 (EventBus, ReviewQueue, TaskManager)
- **文件 I/O:** 使用锁防止竞态条件
- **容量限制:** ReviewQueue (1000), TaskManager (10000)

## 安全加固总结 (v0.1.0)

### 已完成修复
1. ✅ **ExecutionService 未调用** (HIGH) - orchestrator.rs:239-256
   - 修改 ExecutionRequest 接受预生成 execution_id
   - Orchestrator 正确调用 submit()

2. ✅ **PlacementEngine default()** (HIGH) - orchestrator.rs:227
   - 从 Registry 提取 agent runtime_requirements
   - 部分实现，待完善 capability placement

3. ✅ **ReviewQueue 无界增长** (HIGH) - review_queue.rs
   - 添加容量限制 (默认 1000)
   - 防止 DoS 攻击

4. ✅ **TaskManager 无界增长** (MEDIUM) - task_manager.rs
   - 添加容量限制 (默认 10000)
   - 防止资源耗尽

5. ✅ **Task 输入验证** (MEDIUM) - core/task.rs + task_manager.rs
   - 实现 Task::validate()
   - 验证 title/summary/labels 长度和字符
   - 防止注入攻击

### 待优化项
- 🚧 PlacementEngine capability-level placement (labels, zones, secrets)
- 📋 P0 Task 2: Capability 驱动治理
- 📋 P0 Task 3: 审批回流闭环

## 相关文档

- [核心引擎](./core.md) - Task 验证逻辑
- [安全架构](./security.md) - DoS 防护和输入验证
- [架构总览](./INDEX.md) - 系统全局视图

---

**维护说明:** 本文档反映 v0.1.0 实际实现及 P0-1 Phase 2 修复，最后更新: 2026-03-28
