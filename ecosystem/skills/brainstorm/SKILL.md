---
name: brainstorm
description: 创意前置的头脑风暴方法论 — 在进入任何实现（创建功能、搭建组件、改行为）前先把想法打磨成可对齐的设计（CyberClaw 适配版）
source: superpowers/plugin/skills/brainstorming/SKILL.md
adapted-for: CyberClaw (Sprint 8, 2026-04-19)
level: 2
---

<!--
CyberClaw adaptation notes:

- 本文件是 **方法论文档**（Skill 的本体）。CyberClaw 下的 Skill 不直接执行任何代码、
  不拥有任何平台执行权限，真正的动作仍然走 `Connector -> Capability`
  （参考 `CLAUDE.md §2 / §9` 与 `docs/architecture/overview/ARCHITECTURE_V2.0.md`）。
- 调用方式：
    * 作为 Skill 被 `SkillRuntime` 加载（方法论资源加载）。
      CyberClaw 当前的 Skill 载入由
      `crates/cyberclaw-skill-runtime/src/lib.rs`（`SkillRuntime`/`SkillHandler`）负责；
      开箱即用的示例处理器见 `EchoSkill`（仅回显方法论，不执行动作）。
    * 由 Agent 在规划阶段作为 "准入 gate" 参考：在进入
      `AutopilotRuntime`、`PersistentLoop` 或 `SubAgentOrchestrator` 分派之前，
      先走一轮 brainstorm 对齐。
- 与 superpowers 原版的差异：
    1. 移除 **visual-companion**（浏览器陪看工具）。CyberClaw 没有本地 HTTP
       server 类的 skill 行为；视觉化对齐若需要，走
       `Connector -> Capability` 渲染 Artifact，而不是启动浏览器。
    2. 将 "必须先进入 writing-plans" 的跳板改成 CyberClaw 的 `plan` skill
       （`ecosystem/skills/plan/SKILL.md`）。
    3. 将 "设计文档写到 `docs/superpowers/specs/`" 改成 "设计文档作为 Artifact
       持久化到 `cyberclaw-store`，并在 OMC 兼容 harness 下可镜像到
       `.omc/plans/`"。
    4. 语言改为中文优先、英文兼容（与 `ecosystem/skills/plan/SKILL.md` 对齐）。
- 该 skill 本身 **不** 写代码、不 spawn 子进程、不调用任何外部 HTTP。
-->

# Brainstorm — 把想法变成设计

把一个粗糙的想法，通过自然的一来一回对话，打磨成一份可以对齐、可以评审、
可以交给 `plan` 进一步拆解的设计稿。

## 核心硬门 (HARD-GATE)

在用户明确同意设计之前，**禁止**：

- 调用任何实现类 Skill（例如 `plan` 之后的 `AutopilotRuntime` / `PersistentLoop`）
- 写任何代码、生成任何脚手架
- 通过 `SubAgentOrchestrator::spawn_child(AgentId::new("executor"))` 分派执行

无论任务看起来多简单，这条门都适用。
"这个太简单了不需要设计" 是反模式：简单项目恰恰是 **未经审视的假设**
成本最高的地方。设计可以很短（几句话对于真正简单的事就够了），
但必须把它显式化、并获得用户认可。

## 使用时机

### Use When
- 用户提出一个 **创意任务**："帮我做一个…"、"我想加一个…"、"能不能给这个功能加…"
- 需求模糊、范围不清、目标用户不明确
- 用户只给了动机没给方案，或只给了方案没给动机
- 项目在决定走哪条路之前希望先把备选方案摊开

### Do Not Use When
- 用户已经给出足够细节的明确任务 — 直接进入 `plan`
- 只是修一个 bug、改一个字段、补一个测试 — 直接交给 `executor`
- 用户在反馈某个已经在跑的 pipeline — 这属于调整、不是新构想

## 清单 (Checklist)

必须按顺序完成：

1. **探索项目上下文** — 读项目的 README、`docs/INDEX.md`、最近 commit，先弄清
   当前仓库在做什么，再接住用户的想法。CyberClaw 仓库优先读
   `CLAUDE.md` §1–§5，了解对象模型边界与术语。
2. **一次一个澄清问题** — 目的、约束、成功标准
3. **提 2–3 个备选方案** — 每个都附权衡，并推荐一个
4. **分段呈现设计** — 复杂度决定段落长度；每段之后问 "到这儿是对的吗？"
5. **写设计文档** — 持久化到 `cyberclaw-store` 的 Artifact 层；
   OMC 兼容 harness 下可镜像到 `.omc/plans/YYYY-MM-DD-<topic>-design.md`
6. **设计自评** — 对刚写完的文档用新鲜眼睛再看一遍，就地修
7. **用户复核** — 让用户过一遍文档本体，拿到确认
8. **交棒 `plan`** — 终态是调用 `plan`（`ecosystem/skills/plan/SKILL.md`），
   不是直接开干。**brainstorm 的唯一下游是 `plan`。**

## 过程 (The Process)

### 理解想法
- 先探仓库现状（文件、docs、近期 commit），再开口
- 在问细节之前先评估规模：如果用户描述的像 "一个平台，包含
  对话/存储/账单/分析"，立刻提醒 — **这需要先拆**，不要浪费问题
  去精化一个还没被分解的东西
- 如果规模超过一份设计稿能装下，先帮用户拆子项目：有哪些独立块、
  彼此关系是什么、什么顺序建。每个子项目各走一遍
  brainstorm → plan → 实现
- 对规模合适的项目，一次问一个问题
- 尽量用多选题；开放题也行
- 一条消息一个问题，话题大就拆多条

### 探索路径
- 提 2–3 个不同方案，附权衡
- 口语化呈现、亮出你推荐哪个以及为什么
- 先说推荐方案，再摆别的选项

### 呈现设计
- 在你认为自己理解了 "要做什么"之后，再开始呈现设计
- 段落长度和复杂度匹配：简单问题一两句，复杂问题到 200–300 字
- 每段结束问用户 "到这儿看起来没问题吗？"
- 覆盖：架构、组件、数据流、错误处理、测试策略
- 如果哪一段不对劲，回头澄清

### 面向隔离和清晰度设计
- 把系统拆成每个都 "只干一件事" 的单元；单元之间用明确的接口沟通；
  每个单元可以独立被理解和测试
- 对每个单元，你都应该能回答：它做什么、怎么用、它依赖谁
- 判断题：别人能不能不读实现就知道它做什么？能不能改它的实现
  而不破坏调用方？如果不能，边界还得再琢磨
- 小而边界清晰的单元，对 **你自己也更友好** — 你能一次性装进
  上下文的代码，你改起来更可靠

### 在现有代码库里工作
- 改前先探，跟随已有风格
- 已有代码的缺陷如果 **直接影响当前工作**（文件太大、职责混、边界不清），
  把有针对性的清理写进设计 — 就像一个好的开发者会在他正要动的地方
  顺手改好
- 不提不相关的重构。**始终围绕当前目标。**

## 写完文档之后

### 文档位置
- CyberClaw 原生：持久化到 `cyberclaw-store` 的 Artifact 层，
  通过 Provenance 绑定到本次 brainstorm execution
- OMC 兼容 harness：镜像到 `.omc/plans/YYYY-MM-DD-<topic>-design.md`，
  commit 到 git

### 设计自评 (Spec Self-Review)
写完后用新鲜眼睛看一遍：

1. **占位符扫描**：有没有 "TBD"、"TODO"、空段、模糊的要求？就地修
2. **内部一致性**：段落之间有没有互相打架？架构和功能是不是对得上？
3. **范围检查**：这份设计是否窄到能被一份实现计划（`plan`）接住？
   还是仍需要拆？
4. **歧义检查**：任何要求是不是只有一种解读？若两种都行，挑一种，显式化

就地修。不必再来一轮 review，修完继续。

### 用户复核 Gate
自评通过后，请用户过一遍文档：

> "设计稿已经写到 `<path>`。请你复核一下，有没有想改的地方，
>  没问题的话我们就进入 `plan` 做实施计划。"

等用户回复。如果要改，改完再跑一遍自评。批准了才继续。

### 进入实现
- 调用 `plan`（`ecosystem/skills/plan/SKILL.md`）做详细实现计划
- **不要** 调用其他任何 skill。brainstorm 的下一步只有 `plan`

## 关键原则

- **一次一个问题** — 别一口气砸三个
- **优先多选** — 能选的就别开放
- **YAGNI 到底** — 把非必要功能从所有设计里删掉
- **探索备选** — 永远先看 2–3 个方案再收敛
- **增量确认** — 每段设计拿到同意再继续
- **保持弹性** — 不对劲就回去澄清

## 在 CyberClaw 中如何调用

brainstorm 作为 Skill 包，被 `SkillRuntime` 加载后，一般有两条触发路径：

1. **Agent 自主触发**：Agent 识别到创意类请求（模糊目标、缺少范围），
   通过 `SkillRuntime::get()` 载入本 skill 的方法论，在对话中照着做。
   真正的对话由 Agent 的 `AgenticLoop` 推进，不由本 skill 本体执行。

2. **编排层触发**：`BrainCoordinator` 在预检到 "用户请求为创意/设计"
   时，先派 brainstorm，再接 `plan`，再接实现类 runtime
   （`PersistentLoop` / `AutopilotRuntime`）。

在这两条路径里，本 skill 都只提供 **方法论 + 流程门**，
不承担任何执行角色。

## 反模式 (Anti-Patterns)

| 反模式 | 修正 |
|--------|------|
| 跳过探索直接问用户仓库里的信息（"你们 auth 在哪？"） | 先让 Agent 跑一轮 `explore`，再问用户 **偏好** 类问题 |
| 一口气抛 3 个问题 | 一次一个，下一条基于上一条 |
| 把 4 个设计方案一次铺开 | 先推荐 + 讲理由，再逐一摆选项 |
| 没拿到设计同意就开始写代码 | 撤回，回到呈现设计环节 |
| 顺手重构不相关代码 | 只碰 **直接影响当前任务** 的部分 |

## 终态

brainstorm 的终态是 **调用 `plan`**。不是直接开干。
这条线是硬约束 — 跳过 `plan` 直接进 `executor` 或
`AutopilotRuntime` 的代价是：设计与实现脱钩、评审被跳过、
结果走样后没有可回溯的设计稿。

---

**Source acknowledgement**: 原方法论来自 Superpowers 项目的
`skills/brainstorming/SKILL.md`（MIT / 见原仓 LICENSE）。
本文件在 CyberClaw 语境下做了重写：移除了浏览器陪看工具、
把 skill-to-skill 的跳板从 `writing-plans` 改为 CyberClaw 的 `plan`、
把文档持久化换成了 `cyberclaw-store` 的 Artifact 层。
方法论本体（Socratic 一次一个问题、2–3 个备选方案、分段设计、
设计自评、用户复核门）被完整保留。
