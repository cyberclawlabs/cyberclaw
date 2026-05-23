# 可观测性架构

**最后更新:** 2024-03-18
**包路径:** `crates/cyberclaw-observability/`
**状态:** 🚧 规划中

## 可观测性三大支柱

```
Observability Layer
├── Logging (日志)      - 离散事件记录
├── Tracing (追踪)      - 分布式请求追踪
└── Metrics (指标)      - 时序聚合数据
```

## 架构定位

```
┌─────────────────────────────────────────────┐
│        All CyberClaw Components              │
│  • Control Plane • Runtime • Governance     │
└────────────────┬────────────────────────────┘
                 │
                 │ 插桩 (Instrumentation)
                 │
┌────────────────▼────────────────────────────┐
│     Observability Layer (本层)              │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  │
│  │ Logging  │  │ Tracing  │  │ Metrics  │  │
│  │ 日志收集  │  │ 链路追踪  │  │ 指标采集  │  │
│  └─────┬────┘  └─────┬────┘  └─────┬────┘  │
└────────┼─────────────┼─────────────┼────────┘
         │             │             │
┌────────▼─────────────▼─────────────▼────────┐
│          Storage & Processing                │
│  • Elasticsearch  • Jaeger  • Prometheus    │
└────────┬─────────────┬─────────────┬────────┘
         │             │             │
┌────────▼─────────────▼─────────────▼────────┐
│          Visualization & Alerting            │
│  • Kibana  • Jaeger UI  • Grafana           │
└─────────────────────────────────────────────┘
```

## 1. Logging (日志系统)

### 设计目标

```
功能职责：
├── 结构化日志
│   ├── JSON 格式
│   ├── 统一字段 (timestamp, level, target, message)
│   └── 上下文字段 (executionId, agentId, userId)
│
├── 日志级别
│   ├── TRACE  - 详细调试信息
│   ├── DEBUG  - 调试信息
│   ├── INFO   - 一般信息
│   ├── WARN   - 警告信息
│   └── ERROR  - 错误信息
│
├── 日志路由
│   ├── 控制台输出 (开发环境)
│   ├── 文件输出 (生产环境)
│   └── 远程收集 (Elasticsearch, Loki)
│
└── 日志采样
    ├── 全量采集 (ERROR)
    ├── 采样采集 (INFO, DEBUG)
    └── 动态采样率
```

### 日志格式

```json
{
  "timestamp": "2024-03-18T10:00:00.123Z",
  "level": "info",
  "target": "cyberclaw_control_plane::task_manager",
  "message": "Task created successfully",
  "span": {
    "name": "create_task",
    "trace_id": "abc123",
    "span_id": "def456"
  },
  "fields": {
    "task.id": "task-123",
    "task.agent": "security-scanner",
    "task.priority": "high",
    "execution.id": "exec-456",
    "user.id": "user@example.com"
  }
}
```

### 日志实现（规划）

```rust
use tracing::{info, warn, error, instrument};
use serde_json::json;

// 基础日志宏
#[instrument(skip(task_manager))]
pub async fn create_task(task_manager: &TaskManager, input: CreateTaskInput) -> Result<TaskId> {
    info!(
        task.agent = %input.agent,
        task.priority = ?input.priority,
        "Creating task"
    );

    let task_id = task_manager.create(input).await?;

    info!(
        task.id = %task_id,
        "Task created successfully"
    );

    Ok(task_id)
}

// 错误日志 + 上下文
pub async fn invoke_capability(connector: &Connector, input: Value) -> Result<Value> {
    match connector.invoke(input).await {
        Ok(output) => {
            info!(
                connector.id = %connector.id,
                capability = %input["capability"],
                "Capability invoked successfully"
            );
            Ok(output)
        }
        Err(e) => {
            error!(
                error = %e,
                connector.id = %connector.id,
                capability = %input["capability"],
                "Capability invocation failed"
            );
            Err(e)
        }
    }
}
```

### 日志层次结构

```
应用日志
├── Control Plane
│   ├── TaskManager: 任务创建/更新/完成
│   ├── CaseManager: 案例管理
│   ├── ReviewQueue: 审批请求/决策
│   ├── Registry: 包加载/注册
│   └── Resolver: Agent/Skill 选择
│
├── Runtime
│   ├── AgentRuntime: Agent 初始化/执行
│   ├── SkillRuntime: Skill 加载/执行
│   ├── WorkflowEngine: 工作流步骤
│   └── Connector: 能力调用
│
├── Governance
│   ├── PermissionCheck: 权限验证
│   ├── PolicyEngine: 策略评估
│   ├── RiskAssessment: 风险评估
│   └── ApprovalFlow: 审批流程
│
└── Infrastructure
    ├── EventBus: 事件发布/订阅
    ├── LeaseManager: 租约获取/释放
    ├── ArtifactStore: 工件读写
    └── SharedState: 状态更新
```

## 2. Tracing (分布式追踪)

### 设计目标

```
功能职责：
├── Span 管理
│   ├── 创建 root span
│   ├── 创建 child span
│   └── 传播 trace context
│
├── 上下文传播
│   ├── 进程内传播 (tracing crate)
│   ├── 跨服务传播 (HTTP headers)
│   └── 异步上下文传播 (tokio)
│
├── 采样策略
│   ├── 全量采样 (错误 trace)
│   ├── 概率采样 (1%, 10%, 100%)
│   └── 基于规则采样 (高风险操作)
│
└── Trace 导出
    ├── Jaeger (推荐)
    ├── Zipkin
    └── OpenTelemetry Collector
```

### Trace 层次结构

```
HTTP Request: POST /api/v1/tasks
  │
  ├─ Span: create_task (control_plane)
  │   │
  │   ├─ Span: resolver.resolve (选择 Agent)
  │   │   ├─ Span: registry.get_agent
  │   │   ├─ Span: registry.get_skills
  │   │   └─ Span: registry.get_connectors
  │   │
  │   ├─ Span: governance.check_permission
  │   │   ├─ Span: load_identity
  │   │   └─ Span: check_rbac
  │   │
  │   ├─ Span: task_manager.create
  │   │   ├─ Span: generate_task_id
  │   │   ├─ Span: store.save_task
  │   │   └─ Span: event_bus.publish (task.created)
  │   │
  │   └─ Span: execution_service.execute
  │       │
  │       ├─ Span: agent_runtime.initialize
  │       │   ├─ Span: load_agent_spec
  │       │   ├─ Span: load_persona
  │       │   └─ Span: inject_skills
  │       │
  │       ├─ Span: agent_runtime.execute_loop
  │       │   ├─ Span: llm.generate (OpenAI API)
  │       │   ├─ Span: skill.execute (static-analysis)
  │       │   │   └─ Span: script.run
  │       │   │
  │       │   └─ Span: connector.invoke (github:create-issue)
  │       │       ├─ Span: governance.check_permission
  │       │       ├─ Span: governance.risk_assessment
  │       │       ├─ Span: approval.wait (HIGH RISK)
  │       │       └─ Span: http.post (GitHub API)
  │       │
  │       └─ Span: artifact_store.save_results
```

### Trace 实现（规划）

```rust
use tracing::{instrument, Span};
use opentelemetry::trace::TraceContextExt;

// 自动追踪函数
#[instrument(
    name = "task_manager.create",
    skip(self),
    fields(
        task.agent = %input.agent,
        task.id = tracing::field::Empty
    )
)]
pub async fn create_task(&self, input: CreateTaskInput) -> Result<TaskId> {
    let task_id = TaskId::new();

    // 填充 span 字段
    Span::current().record("task.id", &task_id.to_string());

    // 创建子 span
    let resolved = self.resolver.resolve(&input).instrument(
        tracing::info_span!("resolver.resolve")
    ).await?;

    // ... 其他逻辑

    Ok(task_id)
}

// 跨服务传播
pub async fn call_remote_service(client: &HttpClient, url: &str) -> Result<Response> {
    let mut request = Request::builder().uri(url);

    // 注入 trace context 到 HTTP headers
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(
            &Context::current(),
            &mut HeaderInjector::new(&mut request)
        );
    });

    client.send(request.body(()).unwrap()).await
}
```

### Trace 可视化

```
Jaeger UI 示例：

Timeline View:
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
  create_task                      [=============================] 2.5s
    resolver.resolve               [====]                          0.3s
    governance.check_permission    [==]                            0.1s
    task_manager.create            [===]                           0.2s
    execution_service.execute      [====================]          1.8s
      agent_runtime.initialize     [===]                           0.2s
      agent_runtime.execute_loop   [================]              1.5s
        llm.generate               [======]                        0.5s
        skill.execute              [====]                          0.3s
        connector.invoke           [======]                        0.5s
          approval.wait            [====]                          0.3s
          http.post                [==]                            0.1s
      artifact_store.save_results  [=]                             0.1s
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Time: 0s                                          2s        2.5s
```

## 3. Metrics (指标系统)

### 设计目标

```
功能职责：
├── Counter (计数器)
│   ├── 总任务数 (tasks_total)
│   ├── 总请求数 (requests_total)
│   └── 错误计数 (errors_total)
│
├── Gauge (仪表盘)
│   ├── 活跃 Agent 数 (agents_active)
│   ├── 待审批数 (reviews_pending)
│   └── 队列长度 (queue_length)
│
├── Histogram (直方图)
│   ├── 请求延迟 (request_duration_seconds)
│   ├── 任务执行时间 (task_duration_seconds)
│   └── Token 使用量 (tokens_used)
│
└── Summary (摘要)
    ├── API 响应时间分位数
    └── 任务执行时间分位数
```

### 指标定义

```rust
use prometheus::{
    IntCounter, IntGauge, Histogram, HistogramOpts,
    register_int_counter, register_int_gauge, register_histogram
};

lazy_static! {
    // Counter: 任务总数
    pub static ref TASKS_TOTAL: IntCounter = register_int_counter!(
        "cyberclaw_tasks_total",
        "Total number of tasks created"
    ).unwrap();

    // Counter (带标签): 按状态分类的任务
    pub static ref TASKS_BY_STATUS: IntCounterVec = register_int_counter_vec!(
        "cyberclaw_tasks_by_status_total",
        "Tasks by status",
        &["status"]  // completed, failed, pending
    ).unwrap();

    // Gauge: 活跃 Agent 数量
    pub static ref AGENTS_ACTIVE: IntGauge = register_int_gauge!(
        "cyberclaw_agents_active",
        "Number of currently active agents"
    ).unwrap();

    // Histogram: 任务执行时间
    pub static ref TASK_DURATION: Histogram = register_histogram!(
        "cyberclaw_task_duration_seconds",
        "Task execution duration in seconds",
        vec![1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0]
    ).unwrap();

    // Histogram (带标签): API 请求延迟
    pub static ref API_REQUEST_DURATION: HistogramVec = register_histogram_vec!(
        "cyberclaw_api_request_duration_seconds",
        "API request duration in seconds",
        &["method", "endpoint", "status"],
        vec![0.001, 0.01, 0.1, 0.5, 1.0, 5.0]
    ).unwrap();
}
```

### 指标采集

```rust
// 记录任务创建
pub async fn create_task(&self, input: CreateTaskInput) -> Result<TaskId> {
    TASKS_TOTAL.inc();

    let timer = TASK_DURATION.start_timer();

    let result = self.create_internal(input).await;

    timer.observe_duration();

    match &result {
        Ok(_) => TASKS_BY_STATUS.with_label_values(&["completed"]).inc(),
        Err(_) => TASKS_BY_STATUS.with_label_values(&["failed"]).inc(),
    }

    result
}

// 记录 Agent 生命周期
pub async fn start_agent(&self, agent_id: &str) -> Result<()> {
    AGENTS_ACTIVE.inc();
    // ... agent 逻辑
    Ok(())
}

pub async fn stop_agent(&self, agent_id: &str) -> Result<()> {
    AGENTS_ACTIVE.dec();
    // ... 清理逻辑
    Ok(())
}

// API 中间件：自动记录请求指标
pub async fn metrics_middleware(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let timer = API_REQUEST_DURATION
        .with_label_values(&[method.as_str(), &path, ""])
        .start_timer();

    let response = next.run(req).await;

    API_REQUEST_DURATION
        .with_label_values(&[method.as_str(), &path, response.status().as_str()])
        .observe(timer.stop_and_record());

    response
}
```

### 关键指标列表

#### 任务指标
```
cyberclaw_tasks_total                          - 总任务数
cyberclaw_tasks_by_status_total{status}        - 按状态分类
cyberclaw_task_duration_seconds                - 执行时长
cyberclaw_task_depth{agent}                    - 执行树深度
cyberclaw_tasks_pending                        - 待处理任务
```

#### Agent 指标
```
cyberclaw_agents_active                        - 活跃 Agent
cyberclaw_agent_invocations_total{agent}       - Agent 调用次数
cyberclaw_agent_duration_seconds{agent}        - Agent 执行时长
cyberclaw_agent_failures_total{agent}          - Agent 失败次数
```

#### Capability 指标
```
cyberclaw_capabilities_invoked_total{capability, risk} - 能力调用
cyberclaw_capability_duration_seconds{capability}      - 调用时长
cyberclaw_capability_failures_total{capability}        - 调用失败
```

#### 审批指标
```
cyberclaw_reviews_pending                       - 待审批数
cyberclaw_reviews_total{decision}               - 总审批数
cyberclaw_review_wait_time_seconds              - 审批等待时间
cyberclaw_reviews_timeout_total                 - 超时审批数
```

#### 系统指标
```
cyberclaw_event_bus_messages_total              - 事件总数
cyberclaw_event_bus_subscribers                 - 订阅者数
cyberclaw_artifact_store_size_bytes             - 存储大小
cyberclaw_leases_active                         - 活跃租约数
```

### Prometheus 导出

```
GET /metrics

# HELP cyberclaw_tasks_total Total number of tasks created
# TYPE cyberclaw_tasks_total counter
cyberclaw_tasks_total 142

# HELP cyberclaw_tasks_by_status_total Tasks by status
# TYPE cyberclaw_tasks_by_status_total counter
cyberclaw_tasks_by_status_total{status="completed"} 120
cyberclaw_tasks_by_status_total{status="failed"} 12
cyberclaw_tasks_by_status_total{status="pending"} 10

# HELP cyberclaw_agents_active Number of currently active agents
# TYPE cyberclaw_agents_active gauge
cyberclaw_agents_active 5

# HELP cyberclaw_task_duration_seconds Task execution duration in seconds
# TYPE cyberclaw_task_duration_seconds histogram
cyberclaw_task_duration_seconds_bucket{le="1.0"} 10
cyberclaw_task_duration_seconds_bucket{le="5.0"} 45
cyberclaw_task_duration_seconds_bucket{le="10.0"} 80
cyberclaw_task_duration_seconds_bucket{le="30.0"} 110
cyberclaw_task_duration_seconds_bucket{le="60.0"} 130
cyberclaw_task_duration_seconds_bucket{le="300.0"} 140
cyberclaw_task_duration_seconds_bucket{le="+Inf"} 142
cyberclaw_task_duration_seconds_sum 8452.3
cyberclaw_task_duration_seconds_count 142
```

## 4. 告警规则

### Prometheus 告警

```yaml
groups:
  - name: cyberclaw_alerts
    interval: 30s
    rules:
      # 错误率告警
      - alert: HighTaskFailureRate
        expr: |
          rate(cyberclaw_tasks_by_status_total{status="failed"}[5m])
          / rate(cyberclaw_tasks_total[5m]) > 0.1
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High task failure rate (> 10%)"
          description: "{{ $value }}% of tasks are failing"

      # 延迟告警
      - alert: HighTaskDuration
        expr: |
          histogram_quantile(0.95,
            rate(cyberclaw_task_duration_seconds_bucket[5m])
          ) > 300
        for: 10m
        labels:
          severity: warning
        annotations:
          summary: "Task execution time is high (P95 > 5min)"

      # 审批积压告警
      - alert: ReviewBacklog
        expr: cyberclaw_reviews_pending > 50
        for: 30m
        labels:
          severity: warning
        annotations:
          summary: "High number of pending reviews"
          description: "{{ $value }} reviews pending approval"

      # 系统健康告警
      - alert: EventBusDown
        expr: up{job="cyberclaw-control-plane"} == 0
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "Control Plane is down"
```

## 5. Dashboard 示例

### Grafana Dashboard

#### 任务概览面板
```
┌─────────────────────────────────────────────┐
│ Tasks Overview                               │
├─────────────────────────────────────────────┤
│                                              │
│  Total Tasks: 142       Active: 10          │
│  Completed: 120         Failed: 12          │
│                                              │
│  Success Rate: 90.9%    Avg Duration: 59s   │
│                                              │
├─────────────────────────────────────────────┤
│  Task Creation Rate (per minute)            │
│  ▁▂▃▅▆▇██▇▆▅▄▃▂▁▂▃▄▅▆▇█▇▆▅▄▃▂              │
│                                              │
│  Task Duration (P50, P95, P99)              │
│  ▁▂▂▃▃▄▄▅▅▆▆▇▇▇▇▆▆▅▅▄▄▃▃▂▂▁              │
└─────────────────────────────────────────────┘
```

#### Agent 性能面板
```
┌─────────────────────────────────────────────┐
│ Agent Performance                            │
├─────────────────────────────────────────────┤
│                                              │
│  Agent               Invocations  Avg Time  │
│  ───────────────────────────────────────────│
│  security-scanner          45       1.2min  │
│  code-reviewer             32       0.8min  │
│  report-agent              18       0.5min  │
│                                              │
├─────────────────────────────────────────────┤
│  Agent Failure Rate                         │
│  ▁▁▁▁▂▂▂▃▃▃▂▂▂▁▁▁▁▁▁▁▁▁▁▁▁▁▁              │
└─────────────────────────────────────────────┘
```

## 未来扩展

### v2.1 规划
- [ ] tracing 基础设施 (tracing-subscriber)
- [ ] Prometheus 指标导出
- [ ] 结构化日志 (JSON)

### v2.2 规划
- [ ] Jaeger 集成
- [ ] Grafana Dashboard 模板
- [ ] 告警规则

### v2.3 规划
- [ ] 分布式追踪优化
- [ ] 自定义指标
- [ ] 异常检测

## 相关文档

- [控制平面](./control-plane.md) - 服务组件
- [治理层](./governance.md) - 审计日志
- [应用层](./applications.md) - API 指标

---

**维护说明:** 可观测性层目前处于脚手架阶段，基础 tracing 已集成。
