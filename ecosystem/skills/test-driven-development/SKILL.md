---
name: test-driven-development
description: Test-driven development 方法论 — 先写失败测试 → 实现最小代码让测试通过 → 重构保证绿灯。触发词：tdd、test driven、先写测试、写测试再实现、red-green-refactor。
source: superpowers/skills/test-driven-development/SKILL.md
adapted-for: CyberClaw (Sprint 11/12 wave, 2026-04-22)
level: 3
---

<!--
CyberClaw adaptation notes:

- 本文件是 **方法论文档**（Skill 的本体）。CyberClaw 的 Skill 不直接执行测试；
  真正的执行路径走 `Connector -> Capability`（参考 `CLAUDE.md §2 / §9`）。
- 与 `persistent_execution.rs` 中 `AcceptanceCriterion` 的关系：
    * 代码 TDD 用 `#[test]` 和 `cargo test`。
    * 非代码任务（文档、设计、架构审查）用 `AcceptanceCriterion` 作为
      "测试"等价物（在 `PersistentLoop` 内）。
- 子 Agent 派遣：通过 `SubAgentOrchestrator::spawn_child(AgentId::new("executor"))`
  运行测试和实现；参考 `verify` skill 的测试命令。
-->

# Test-Driven Development (TDD)

以测试驱动实现 — 先写一个失败的测试，再写最少代码让它通过，然后重构。

## 核心循环

### Red → Green → Refactor

```
1. RED:   写一个失败的测试（描述想要的行为）
2. GREEN: 写最少代码让测试通过
3. REFACTOR: 在保持绿灯的前提下清理代码
   回到 RED（继续添加新测试）
```

这个循环很小，通常 5-15 分钟一个完整周期。

## Step 1: RED — 写一个失败的测试

测试应该：
- **明确描述一个行为**（不是实现细节）
- **现在就能写**（不依赖实现代码）
- **会失败**（因为功能还不存在）

**示例（Rust）：**

```rust
#[test]
fn test_user_can_reset_password_with_valid_token() {
    let user_email = "test@example.com";
    let reset_token = "valid-token-12345";
    
    let result = reset_password(user_email, reset_token, "new-password");
    
    assert!(result.is_ok());
    assert_eq!(result.unwrap().email, user_email);
}
```

**关键原则：**
- **测试应该从用户视角描述行为**，不是从实现视角说"调用这个函数"
- **一个测试一个行为**（不要在一个测试里混合多个 assertion）
- **边界情况也要测试**（密码太短、token 过期、邮箱不存在）

**示例边界情况：**

```rust
#[test]
fn test_reset_password_fails_with_expired_token() {
    let result = reset_password("test@example.com", "expired-token", "new-pwd");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "token_expired");
}

#[test]
fn test_reset_password_fails_with_weak_password() {
    let result = reset_password("test@example.com", "valid-token", "123");
    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "password_too_weak");
}
```

运行这些测试 — 它们应该全红（fail）。

### 非代码任务的 "RED" 阶段

如果这是一个非代码任务（文档、架构审查、系统设计），用 `AcceptanceCriterion`
代替测试：

```rust
AcceptanceCriterion {
    name: "Authentication docs include OAuth2 flow diagram".to_string(),
    description: "Docs must have a visual flow showing: User → App → Provider → Token".to_string(),
    criteria: vec![
        "docs/auth/oauth2.md exists".to_string(),
        "Contains Mermaid/ASCII diagram of OAuth2 flow".to_string(),
        "Covers token refresh scenario".to_string(),
    ],
}
```

验证这些 acceptance criteria 是否被满足（参考 `verify` skill）。

## Step 2: GREEN — 最少代码让测试通过

写 **最小化的实现**。目标是让测试通过，不是完美实现。

**示例实现：**

```rust
fn reset_password(email: &str, token: &str, new_password: &str) -> Result<User, String> {
    // 最少代码：验证 token、验证密码强度、更新密码
    
    if token.is_empty() || token == "expired-token" {
        return Err("token_expired".to_string());
    }
    
    if new_password.len() < 8 {
        return Err("password_too_weak".to_string());
    }
    
    Ok(User {
        email: email.to_string(),
        password_hash: hash_password(new_password),
    })
}
```

**关键原则：**
- **只写让这个测试通过的代码**
- **不要 "优化"、"预防"、"预留"** — 那是稍后重构的工作
- **不要跳到实现全部逻辑** — 一次一个测试

如果你发现自己写了 20 行代码让一个测试通过，问问自己：这测试能再拆成两个吗？

## Step 3: REFACTOR — 清理代码（保持绿灯）

现在所有测试都通过了，可以：
- **提取重复代码** → helper 函数
- **改名** 让意图更清楚
- **拆分函数** 如果太长
- **移动数据结构** 到合理的位置

**重构原则：**
- **每改一次，就跑一遍所有测试** — 确保没破坏已有行为
- **一次改一个地方** — 不要同时重构 3 个函数
- **重构不改行为** — 不是修 bug 的时候，不是加功能的时候

**示例重构：**

```rust
// 重构前
fn reset_password(email: &str, token: &str, new_password: &str) -> Result<User, String> {
    if token.is_empty() || token == "expired-token" {
        return Err("token_expired".to_string());
    }
    if new_password.len() < 8 {
        return Err("password_too_weak".to_string());
    }
    Ok(User {
        email: email.to_string(),
        password_hash: hash_password(new_password),
    })
}

// 重构后 — 提取验证逻辑
fn reset_password(email: &str, token: &str, new_password: &str) -> Result<User, String> {
    validate_reset_token(token)?;
    validate_password_strength(new_password)?;
    Ok(create_user_with_password(email, new_password))
}

fn validate_reset_token(token: &str) -> Result<(), String> {
    if token.is_empty() || token == "expired-token" {
        Err("token_expired".to_string())
    } else {
        Ok(())
    }
}

fn validate_password_strength(pwd: &str) -> Result<(), String> {
    if pwd.len() < 8 {
        Err("password_too_weak".to_string())
    } else {
        Ok(())
    }
}
```

重构后再跑测试 — 应该还是全绿。

## 完整周期示例

假设你要实现一个"购物车计算折扣"的功能。

### 周期 1: 基础折扣

```rust
#[test]
fn test_apply_10_percent_discount() {
    let cart = vec![
        Item { name: "Apple", price: 100 },
        Item { name: "Banana", price: 50 },
    ];
    let total = calculate_total(&cart, 0.1); // 10% 折扣
    assert_eq!(total, 135); // (100 + 50) * 0.9 = 135
}
```

实现：

```rust
fn calculate_total(items: &[Item], discount_rate: f32) -> f32 {
    let subtotal: f32 = items.iter().map(|i| i.price as f32).sum();
    subtotal * (1.0 - discount_rate)
}
```

### 周期 2: 最少金额的折扣

```rust
#[test]
fn test_discount_only_applies_above_minimum() {
    let cart = vec![Item { name: "Apple", price: 50 }];
    let total = calculate_total_with_min(&cart, 0.1, 100); // 需要 ≥100 才打折
    assert_eq!(total, 50); // 50 < 100，不打折
}

#[test]
fn test_discount_applies_when_above_minimum() {
    let cart = vec![
        Item { name: "Apple", price: 80 },
        Item { name: "Banana", price: 40 },
    ];
    let total = calculate_total_with_min(&cart, 0.1, 100);
    assert_eq!(total, 108); // (80 + 40) * 0.9 = 108
}
```

实现：

```rust
fn calculate_total_with_min(items: &[Item], discount_rate: f32, min_amount: f32) -> f32 {
    let subtotal: f32 = items.iter().map(|i| i.price as f32).sum();
    if subtotal >= min_amount {
        subtotal * (1.0 - discount_rate)
    } else {
        subtotal
    }
}
```

### 周期 3: 重构 + 合并

现在两个函数可以合并：

```rust
fn calculate_total(items: &[Item], discount_rate: f32, min_amount: Option<f32>) -> f32 {
    let subtotal: f32 = items.iter().map(|i| i.price as f32).sum();
    let should_discount = min_amount.map_or(true, |min| subtotal >= min);
    if should_discount {
        subtotal * (1.0 - discount_rate)
    } else {
        subtotal
    }
}
```

所有测试还是绿。

## 与 CyberClaw 对象模型的关系

### 代码部分

- **测试本体** → `#[test]` 属于你的代码库（比如 `crates/cyberclaw-core/src/lib.rs`）
- **运行测试** → Agent 通过 `SubAgentOrchestrator::spawn_child(AgentId::new("executor"))`
  派遣执行 `cargo test`（或针对特定 crate）
- **持久化结果** → 测试通过/失败结果记录在 Artifact（通过 `verify` skill）

### 非代码部分

对于架构设计、文档、审查等，用 `AcceptanceCriterion` 代替测试：

```rust
// 在 PersistentLoop 里
let criterion = AcceptanceCriterion {
    name: "API docs complete".to_string(),
    description: "All endpoints documented with examples".to_string(),
    criteria: vec![
        "GET /api/users documented".to_string(),
        "POST /api/users documented".to_string(),
        "Response format includes examples".to_string(),
    ],
};

loop_state.add_criterion(criterion);
// Agent 工作，验证 criterion 满足
```

## TDD 的反模式

| 反模式 | 修正 |
|--------|------|
| 测试后写实现（"先写完再测"） | 这不是 TDD。TDD 必须先写失败的测试 |
| 测试太高级，覆盖整个系统 | 用 unit test 测单个函数；集成测试分开写 |
| 一个测试里混合多个 assertion | 拆成多个测试，每个测试一个行为 |
| 跳过边界情况（"明显不会发生"） | TDD 里没有 "明显"；写测试再说 |
| 测试通过后不重构（"反正能用"） | 重构不改行为，只改可读性。必须做 |
| 写了太多代码才让一个测试通过 | 警信号；测试应该更小、更专注 |

## 何时使用 TDD

**强推荐：**
- 核心业务逻辑（支付、权限、数据一致性）
- 复杂算法
- 容易出 bug 的区域（并发、资源清理）
- 需要长期维护的代码

**可选：**
- UI 代码（UI test 写起来麻烦，但也有 value）
- 简单数据转换（如果逻辑确实很直白，unit test 可能 overkill）

**不适合：**
- 探索性代码（"我不知道这样会不会 work"）— 先写代码探索，再补测试
- 学习阶段 — TDD 需要知道期望结果；完全陌生的领域先看例子

## 关键原则（重复以强化）

1. **测试-实现-清理 的循环很小** — 5-15 分钟一轮，不是一天一轮
2. **测试不是事后诸葛亮** — 它们驱动设计，让你提前思考边界和 error case
3. **重构很重要** — TDD 经常被理解为 "有测试就行"，其实重构是提升代码质量的地方
4. **不要过度测试** — 100% 覆盖率没必要；覆盖重要路径和边界情况就够
5. **好的测试名字值得** — "test_user_can_reset_password_with_valid_token" 比 "test_reset" 好得多

---

**Source acknowledgement**: 原方法论来自 Superpowers 项目的
`skills/test-driven-development/SKILL.md`。本适配版在 CyberClaw 语境下重写：
增加了对 `AcceptanceCriterion`（非代码任务的"测试"等价物）的引用、
说明了测试执行通过 `SubAgentOrchestrator` 和 `verify` skill 的方式、
并强调了与 `PersistentLoop` 的集成。
