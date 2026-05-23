# 核心引擎架构

**最后更新:** 2024-03-19
**包路径:** `crates/cyberclaw-core/`
**入口点:** `src/lib.rs`
**状态:** ✅ 安全加固完成

## 模块结构

```
cyberclaw-core/src/
├── lib.rs          # 库入口，导出公共 API
├── ids.rs          # ID 类型和验证 [H-2 已修复]
├── task.rs         # 任务类型和验证 [新增 ✓]
├── case.rs         # 案例类型定义
├── capability.rs   # 能力系统类型
├── cluster.rs      # 集群类型定义
├── enums.rs        # 枚举类型
├── execution.rs    # 执行类型定义
├── identity.rs     # 身份类型定义
├── manifests.rs    # 清单类型定义
├── provenance.rs   # 溯源类型定义
└── prelude.rs      # 常用类型预导入
```

## 核心类型

### Task (任务) 🔒
**文件:** `task.rs`
**安全加固:** v0.1.0 输入验证

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub case_id: Option<CaseId>,
    pub title: String,          // ← 验证
    pub summary: String,        // ← 验证
    pub kind: TaskKind,
    pub priority: Priority,
    pub requested_by: ActorRef,
    pub requested_at: DateTime<Utc>,
    pub trigger: TriggerRef,
    pub input: TaskInput,
    pub desired_outputs: Vec<OutputContractRef>,
    pub labels: Vec<String>,    // ← 验证
}

impl Task {
    pub fn validate(&self) -> anyhow::Result<()>;
}

验证规则 (v0.1.0):
✓ title: 1-255 字符，无控制字符 (除 \n \t \r)
✓ summary: 0-2000 字符，无控制字符 (除 \n \t \r)
✓ labels: 每个 1-100 字符，无控制字符
✓ 防止注入攻击 (XSS, SQL 注入)
✓ 防止数据完整性破坏
```

### ExecutionId (执行标识符)

**文件:** `execution.rs`
**安全增强:** H-2 (输入验证)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ExecutionId(pub String);

验证规则：
✓ 非空字符串
✓ 长度 ≤ 128 字符
✓ 无控制字符
✓ 无路径遍历序列 (../, /, \)
✓ 无绝对路径标记
```

### NodeId (节点标识符)

**文件:** `execution.rs`
**安全增强:** H-2 (格式验证)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

验证规则：
✓ 与 ExecutionId 相同的基本规则
✓ 额外的节点特定验证
```

## 验证流程

### Task 验证流程 (v0.1.0 新增)

```
Task
  ↓
Task.validate()
  ↓
validate_string_field("title", 1, 255)
  ├─ 长度检查: 1 ≤ len ≤ 255
  ├─ 控制字符检查 (允许 \n \t \r)
  └─ 失败 → bail!("title 错误")
  ↓
validate_string_field("summary", 0, 2000)
  ├─ 长度检查: len ≤ 2000
  ├─ 控制字符检查
  └─ 失败 → bail!("summary 错误")
  ↓
for label in labels:
  validate_string_field(label, 1, 100)
    ├─ 长度检查: 1 ≤ len ≤ 100
    ├─ 控制字符检查
    └─ 失败 → bail!("label 错误")
  ↓
通过 → Ok(())
  ↓
TaskManager.create_task() 继续处理
```

### ID 验证流程

```
输入 String
    ↓
长度检查 (1-128)
    ↓
字符检查 (无控制字符)
    ↓
路径检查 (无 ../, /, \)
    ↓
  通过？
   ├─是→ 创建 ExecutionId/NodeId
   │        ↓
   │    传递给控制平面
   │
   └─否→ 返回验证错误
```

## 数据流

```
用户请求 → Core::ExecutionId → ControlPlane::ArtifactStore
                             → ControlPlane::EventBus
                             → ControlPlane::SharedState

用户请求 → Core::NodeId → ControlPlane::MembershipService
                        → ControlPlane::LeaseManager
                        → ControlPlane::SubagentScheduler
```

## 验证函数

### validate() 方法

```rust
impl ExecutionId {
    pub fn validate(&self) -> anyhow::Result<()> {
        // 1. 长度检查
        if self.0.is_empty() || self.0.len() > Self::MAX_LEN {
            anyhow::bail!("Invalid length");
        }

        // 2. 字符检查
        for ch in self.0.chars() {
            if ch.is_control() {
                anyhow::bail!("Control character detected");
            }
        }

        // 3. 路径遍历检查
        if self.0.contains("..") ||
           self.0.contains('/') ||
           self.0.contains('\\') {
            anyhow::bail!("Path traversal detected");
        }

        Ok(())
    }
}
```

## 使用示例

```rust
// ✅ 有效的 ID
let exec_id = ExecutionId("exec-123-abc".to_string());
exec_id.validate()?;

// ❌ 无效的 ID - 路径遍历
let bad_id = ExecutionId("../../etc/passwd".to_string());
bad_id.validate()?; // 返回错误

// ❌ 无效的 ID - 控制字符
let bad_id = ExecutionId("exec\x00\x01".to_string());
bad_id.validate()?; // 返回错误

// ❌ 无效的 ID - 太长
let bad_id = ExecutionId("x".repeat(200));
bad_id.validate()?; // 返回错误
```

## 测试覆盖 (v0.1.0)

### Task 验证测试 (新增)

| 测试类型 | 数量 | 测试名称 |
|----------|------|----------|
| 长度验证 | 3 | empty_title, title_too_long, summary_too_long |
| 控制字符 | 1 | control_characters (null byte) |
| 正常输入 | 1 | allows_valid_task (含 \n \t) |

**测试位置:** `crates/cyberclaw-control-plane/src/task_manager.rs::tests`
- ✅ `test_validation_rejects_empty_title` - 拒绝空 title
- ✅ `test_validation_rejects_title_too_long` - 拒绝超长 title (>255)
- ✅ `test_validation_rejects_control_characters` - 拒绝 null byte
- ✅ `test_validation_rejects_summary_too_long` - 拒绝超长 summary (>2000)
- ✅ `test_validation_allows_valid_task` - 允许合法任务（含 \n \t）

### ID 验证测试

| 测试类型 | 数量 | 描述 |
|----------|------|------|
| 有效输入 | 3 | 正常格式的 ID |
| 空字符串 | 2 | 空 ID 拒绝 |
| 长度限制 | 2 | 超长 ID 拒绝 |
| 控制字符 | 3 | 控制字符拒绝 |
| 路径遍历 | 4 | ../、/、\ 拒绝 |
| 边界值 | 3 | 恰好 128 字符 |

**测试位置:** `crates/cyberclaw-core/src/ids.rs::tests` (17 个测试)

### 测试示例

```rust
#[test]
fn test_execution_id_validation_valid() {
    let id = ExecutionId("valid-exec-123".to_string());
    assert!(id.validate().is_ok());
}

#[test]
fn test_execution_id_validation_path_traversal() {
    let id = ExecutionId("../../etc/passwd".to_string());
    assert!(id.validate().is_err());
}

#[test]
fn test_execution_id_validation_control_chars() {
    let id = ExecutionId("exec\x00\x01".to_string());
    assert!(id.validate().is_err());
}
```

## 集成点

```
Core Layer                    Control Plane Layer
────────────────────────────  ──────────────────────────────
ExecutionId::validate()   →   ArtifactStore::sanitize_path()
                          →   EventBus::publish()
                          →   SharedState::get()

NodeId::validate()        →   MembershipService::register()
                          →   LeaseManager::acquire()
                          →   SubagentScheduler::schedule()
```

## 性能特征

- **验证开销:** O(n)，其中 n 是 ID 长度 (最大 128)
- **内存占用:** 每个 ID 一个 String 分配
- **线程安全:** 实现 Send + Sync

## 相关文档

- [控制平面架构](./control-plane.md)
- [安全架构](./security.md)
- [项目总览](./INDEX.md)

---

**维护说明:** 本文档从源代码自动生成，反映实际实现。
