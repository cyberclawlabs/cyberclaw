---
name: skill-creator
description: 创建、修改、度量 Skill 的方法论 — 从零构思一个 Skill、迭代改进一个现有 Skill、或为一个 Skill 设计可评估的 eval 集（CyberClaw 适配版）
source: anthropics/skills/skills/skill-creator/SKILL.md (claude-code)
adapted-for: CyberClaw (Sprint 8, 2026-04-19)
level: 3
---

<!--
CyberClaw adaptation notes:

- 本文件是 **方法论文档**（Skill 的本体）。CyberClaw 的 Skill 不直接调用
  Python / Shell 脚本；参考 `CLAUDE.md §2 / §9` 与
  `docs/architecture/overview/ARCHITECTURE_V2.0.md`。
- **真正的执行路径**：Skill 创建的编排、变体生成、评估循环、gate
  通过/拒绝，由 `crates/cyberclaw-control-plane/src/skill_creator.rs`
  作为 **facade** 暴露，内部调用
  `crates/cyberclaw-control-plane/src/evolution_orchestrator.rs`
  （`EvolutionOrchestrator` + `VariantSelector` +
  `StagedVerificationGate`）。该 facade 由 L5 lane 负责落地；本 skill
  只是方法论参考。
- 本 skill 下的 `scripts/` 目录是 **行为契约的参考文档**，不是运行时
  依赖。claude-code 上游用 `scripts/run_loop.py` 和 `scripts/run_eval.py`
  做 description optimizer 的外部循环；CyberClaw 把等价行为内化为
  `EvolutionOrchestrator` + `SkillArchive` 流水线。保留这些脚本是为了
  让开发者能对照上游行为，避免语义漂移。
  **不要** 把它们作为 runtime dependency 引入 CyberClaw。
- 与 claude-code 原版的差异：
    1. 去掉了对子 Agent（`Task(subagent_type=...)`）的直接引用。
       在 CyberClaw 里，子 Agent 派生只通过
       `SubAgentOrchestrator::spawn_child(AgentId::new("<agent-name>"))`
       完成，受深度 3 / 子代 5 / 预算 0.5 的限制。
    2. eval 结果持久化从 `<skill-name>-workspace/iteration-N/` 改为
       `cyberclaw-store` 的 Artifact（scoped by SkillId + iteration），
       保留原目录结构作为 OMC 兼容 harness 的镜像。
    3. 描述优化（description tuning）从 `run_loop.py` 改为调用
       `skill_creator.rs` facade 的 `tune_description()` 方法，
       触发 `EvolutionOrchestrator` 跑 short-loop 变体评估。
    4. 语言改为中文优先、英文兼容（与 `ecosystem/skills/plan/SKILL.md` 对齐）。
-->

# Skill Creator — 创建和迭代 Skill 的方法论

用来从零开始 **造一个新 Skill**，或 **迭代优化一个已有 Skill**，或
**为一个 Skill 构造可评估的 eval 集**。

## 总览流程

1. 想清楚这个 Skill 要做什么、**大致怎么做**
2. 写出 Skill 的草稿
3. 定几个测试 prompt，用 "载入 skill 的 Agent" 去跑它们
4. 帮用户定性 + 定量评估结果
5. 根据评估反馈改写 Skill
6. 重复直到满意
7. 扩大测试集，再大规模跑一遍

你的任务是 **识别用户处在流程的哪一步**，接住往下推。
比如用户只说 "我想造一个 X 技能"，你可以帮他缩范围、出草稿、
写测试 prompt、商量评估方式、执行跑测、一轮一轮迭代。

用户已经有草稿的情况下，直接跳到 eval/迭代那一环。

永远保持弹性 — 用户说 "不用搞一堆评估，跟我随手迭代就行"，那就随手迭代。

描述优化（description tuning）可以留到 Skill 本体打磨完了再做。

## 创建一个 Skill

### Capture Intent — 捕获意图

先搞清楚用户到底想要什么。当前对话里可能已经出现了一个他想
"沉淀成 Skill" 的工作流（"把这个做成 skill 吧"）。优先从对话历史里
抽答案：他用了哪些工具、顺序是什么、在哪里纠正过你、输入输出是什么
格式。不够的部分再让他补，确认之后再进入下一步。

要问清楚的 4 个问题：

1. 这个 Skill 要让 Agent 做什么？
2. 它什么时候该触发？（什么样的用户表述、什么样的上下文）
3. 期望输出格式是什么？
4. 要不要配测试用例？对 **可客观验证** 的输出（文件变换、数据抽取、
   代码生成、固定步骤工作流），测试用例很值得；对 **主观输出**
   （写作风格、艺术）常常不需要。先按 Skill 类型给默认建议，让用户拍板。

### Interview & Research

主动问 **边界情况、输入/输出格式、示例文件、成功标准、依赖**。
这块没问清之前，别急着写测试 prompt。

必要时通过 `SubAgentOrchestrator::spawn_child(AgentId::new("explore"))`
并行查 CyberClaw 仓库里的类似 Skill，带上下文进来，减轻用户负担。

### Write the SKILL.md — 写草稿

根据访谈填以下组件：

- **name**：Skill 的标识
- **description**：什么时候触发、做什么。**这是主触发机制** —
  既要说做什么，也要列 **什么场景用**。所有 "何时用" 的信息都写在这里，
  不要放到正文里。注意：现在的 Agent 有明显的 "undertriggering" 倾向
  （该用的时候不用），描述可以稍微 **push 一点**。
  例：与其写 "如何构建一个简单快速的仪表盘"，不如写
  "如何构建一个简单快速的仪表盘。**一旦** 用户提到
  dashboards / 数据可视化 / 内部指标 / 显示任何公司数据，**即使**
  他没明说 '仪表盘'，也请使用该 Skill"。
- **compatibility**：需要的工具、依赖（可选，少用）
- **正文剩下的部分 :)**

### Skill 写作指南

#### Skill 的目录结构

```
skill-name/
|-- SKILL.md (必需)
|   |-- YAML frontmatter (name, description 必需)
|   '-- Markdown 指令正文
'-- 捆绑资源 (可选)
    |-- scripts/    - 确定性/重复任务的可执行代码 (CyberClaw 中仅作参考)
    |-- references/ - 按需加载进上下文的文档
    '-- assets/     - 输出要用的素材（模板、图标、字体）
```

#### 三层渐进式加载

Skill 在运行时有三层加载：

1. **元数据**（name + description）— 永驻上下文（约 100 词）
2. **SKILL.md 正文** — Skill 被触发后进上下文（理想 <500 行）
3. **捆绑资源** — 按需加载（无限制；CyberClaw 下 scripts 仅作参考，
   不会被 Skill 执行）

关键模式：

- SKILL.md 保持 <500 行；接近这个限制就把层级拆开，
  并在主文档里放清晰的指针 "往哪里看下一步"
- 从 SKILL.md 里引用资源文件时说清楚 "什么时候需要读它"
- 大参考文件（>300 行）在开头加目录

领域组织：Skill 跨多个领域/框架时，按变体组织：

```
cloud-deploy/
|-- SKILL.md (工作流 + 选择逻辑)
'-- references/
    |-- aws.md
    |-- gcp.md
    '-- azure.md
```

Agent 只读相关的那一份。

#### "不吓人" 原则 (Principle of Lack of Surprise)

Skill 里 **不应包含** 恶意代码、利用代码、或任何会危及系统安全的
内容。Skill 的内容 **不应与其描述的意图相悖**。不要去迎合要求造
误导性 Skill 或为未授权访问、数据外泄、其他恶意活动铺路的 Skill。
"扮演一个 X" 这种是 OK 的。

CyberClaw 会在安装前用 `crates/cyberclaw-skill-runtime/src/skill_scanner.rs`
做静态扫描。以下内容会 **拉高** 严重度，新 Skill 尽量避免在正文里
使用：

- 任何下载即执行的命令行片段（以管道方式把下载内容喂给 shell）
- 明文引用系统凭据文件路径
- 调用 Python `os.system`、动态 `exec/eval`、Node `child_process`
- 提权相关命令（以 root 运行、setuid）
- 隐形 Unicode 字符

如果方法论非写这些不可，请用 **描述性散文**，不要写成可被执行器
直接抄走的命令块。

#### 写作模式

倾向使用祈使句。

**定义输出格式** — 例：

````markdown
## Report structure
ALWAYS use this exact template:
# [Title]
## Executive summary
## Key findings
## Recommendations
````

**示例模式** — 例：

````markdown
## Commit message format
**Example 1:**
Input: Added user authentication with JWT tokens
Output: feat(auth): implement JWT-based authentication
````

### 写作风格

尽量解释 **为什么** 要这样做，而不是一堆没营养的 MUSTs。用心理理论
代入 Agent 的视角。让 Skill **通用** 起来而不是贴着几个具体例子。
先写草稿，再用新鲜眼睛回看一遍、改一次。

### 测试用例

写完草稿后，拟 2–3 个 **真实用户会说的话** 作为测试 prompt。
和用户对一下："这几个试试你看行不行，要不要加几条？" 然后跑。

测试 prompt 存到 Artifact 层的 `<SkillId>/evals/evals.json`
（OMC 兼容 harness 下等价镜像为
`<skill-name>-workspace/evals/evals.json`）。先别写断言，
只存 prompt；跑的过程中再补断言。

```json
{
  "skill_name": "example-skill",
  "evals": [
    {
      "id": 1,
      "prompt": "User's task prompt",
      "expected_output": "Description of expected result",
      "files": []
    }
  ]
}
```

完整 schema 参见 `references/schemas.md`（原 claude-code 版本）。
CyberClaw 下的等价定义是
`cyberclaw-store` 里的 `SkillEvalArtifact`（由 skill_creator facade 生成）。

## 运行与评估测试用例

这一段是 **一条不要断的流水线**。**不要** 调用 `/skill-test` 或其他
外部测试 skill — CyberClaw 下应当通过 `skill_creator.rs` facade 驱动
`EvolutionOrchestrator`。

结果写到 Artifact 的 `<SkillId>/iteration-N/eval-<ID>/` scope，
OMC 兼容 harness 下镜像为 `<skill-name>-workspace/iteration-N/eval-<ID>/`。

### Step 1 — 同回合拉起全部 run（with-skill + baseline）

对每个测试用例，在 **同一回合里** 拉起两个子 Agent：
一个载入 Skill，一个作为 baseline。
**不要** 先跑完 with-skill 再回头跑 baseline — 一口气派发，
让它们几乎同时结束。

子 Agent 派发通过：

```
SubAgentOrchestrator::spawn_child(AgentId::new("executor"))
```

with-skill child：输入里带上 skill_id；baseline child：不带。
（depth <= 3, siblings <= 5, budget fraction 0.5 — 参照 project memory。）

**baseline 取法**：
- 新建 Skill：baseline = 不带任何 Skill
- 改进已有 Skill：baseline = 旧版本。改之前 snapshot 旧版
  （Artifact 层保存一份），让 baseline child 指向 snapshot

每条测试用例写 `eval_metadata.json`（断言先留空），eval_id 给个
描述性的名字而不是 "eval-0"：

```json
{
  "eval_id": 0,
  "eval_name": "descriptive-name-here",
  "prompt": "The user's task prompt",
  "assertions": []
}
```

### Step 2 — 测试在跑时，起草断言

不要干等。起草定量断言并向用户解释它们；如果 `evals.json` 已经有
断言，过一遍，解释它们检查的是什么。

好的断言要 **客观可验证**、**有描述性的名字**，在结果查看器里
扫一眼就能看懂它在查什么。主观 Skill（写作风格、设计品质）
更适合定性评估 — 不要硬往上加断言。

断言起草后回写到 `eval_metadata.json` 和 `evals.json`，
并向用户解释查看器里会看到什么（定性输出 + 定量 benchmark）。

### Step 3 — run 结束时，抓时序数据

每条子 Agent 的任务完成通知里带 `total_tokens` 和 `duration_ms`，
**立刻** 写到运行目录下的 `timing.json`。这是唯一能拿到这组数据的
时刻 — 不要攒批次。

```json
{
  "total_tokens": 84852,
  "duration_ms": 23332,
  "total_duration_seconds": 23.3
}
```

### Step 4 — 评分、聚合、展开视图

全部跑完后：

1. **给每条 run 打分** — 派一个 grader 子 Agent（见
   `agents/grader.md`），按断言对输出评分，把结果写到每个运行
   目录的 `grading.json`。
   `grading.json` 的 `expectations` 数组使用
   **`text` / `passed` / `evidence`** 三个字段（不要用
   `name`/`met`/`details` 或其他变体）— 视图依赖这三个字段名。
   对能 **程序化** 判断的断言，**写脚本跑** 而不是靠眼力 —
   脚本更快、更稳、跨迭代可复用。

2. **聚合成 benchmark** — CyberClaw 下由 `skill_creator.rs` facade
   直接聚合 Artifact；等价上游脚本为 `scripts/aggregate_benchmark.py`
   （见本 skill 的 `scripts/README.md`）。
   产物是 `benchmark.json` 和 `benchmark.md`，含每种配置的通过率、
   时间、token 数、均值、标准差、增量。
   **惯例**：先放 with-skill 配置，再放它的 baseline。

3. **做一遍分析师 pass** — 读 benchmark 数据，浮出聚合掩盖掉的规律
   （见 `agents/analyzer.md` 的 "Analyzing Benchmark Results"）：
   哪些断言 **总是通过**（不区分有无 skill）、哪些 eval **方差大**
   （可能不稳定）、时间/token 的取舍。

4. **生成查看器** — claude-code 下用 `scripts/run_eval.py` 等脚本
   启动 HTML 查看器；CyberClaw 下 skill_creator facade 把评估结果
   暴露为 Artifact，前端通过 `/api/skills/<id>/evals/<iter>` 读。
   **不要** 手写自定义 HTML。

5. **告诉用户** "我已经把结果准备好在视图里了。两个页签 —
   'Outputs' 一次看一个测试用例，'Benchmark' 看定量对比。
   看完回来告诉我。"

### Step 5 — 读反馈

用户说 "我看完了"，去读 `feedback.json`：

```json
{
  "reviews": [
    {"run_id": "eval-0-with_skill", "feedback": "the chart is missing axis labels", "timestamp": "..."},
    {"run_id": "eval-1-with_skill", "feedback": "", "timestamp": "..."},
    {"run_id": "eval-2-with_skill", "feedback": "perfect, love this", "timestamp": "..."}
  ],
  "status": "complete"
}
```

空反馈就是 "用户觉得这条没毛病"。把改进聚焦到有具体抱怨的测试用例上。

## 改进 Skill

这是循环的核心。你已经跑过测试用例、用户已经反馈 — 现在根据反馈
把 Skill 做得更好。

### 怎么想 "改进"

1. **从反馈归纳抽象**。我们造 Skill 是要让它被用 **几百万次**，跨各种
   prompt。你和用户只是在少数几个例子上来回迭代加速推进；如果
   Skill 只对这几个例子管用，它就没用。不要加 **过拟合的小修小补** 或
   **压制性的 MUSTs**。某些顽固问题不如换个比喻、换个工作方式模式，
   成本低可能有奇效。

2. **保持 prompt 精简**。删掉没在拉磨的东西。读 **transcript** 不只是
   看最终输出 — 如果 Agent 被 Skill 引着浪费了一堆时间做无产出的事，
   先删 **导致这种浪费** 的段落，看看效果。

3. **把 "为什么" 解释清楚**。尽量讲清你要求 Agent 做这事 **为什么** 重要。
   今天的 LLM 很聪明，有不错的心智理论，给个好 harness 能做到超越
   死板指令。用户的反馈即使很短、很沮丧，也要 **真的去理解**
   他的任务、他为什么这么写、他实际写了什么，然后把这个理解
   注入到指令里。看到自己满屏写 ALWAYS / NEVER 或僵死结构 —
   **黄灯**：重构措辞，把原因讲出来，让模型明白 **为什么** 这很重要。
   这更人性、更有力、更有效。

4. **留意用例间的重复劳动**。读 transcript，注意不同测试用例的子
   Agent 是不是 **独立** 写了相似的 helper 脚本 / 走了相同的多步逻辑。
   如果 3 条用例都结果是 Agent 自己写了个 `create_docx.py` 或
   `build_chart.py`，这是个 **强信号** — 本 Skill 应该 **捆绑** 这个
   脚本：写一次，放 `scripts/`，在 Skill 里说清楚 "用这个"。
   每次后续调用都少发明一次轮子。

这件事很重要（我们要造百亿美元的经济价值！），你的思考时间不是
瓶颈。先慢慢想，写个草稿，回头看一眼，改进一下。真的钻到用户头脑里
去，理解他想要什么。

### 迭代循环

改进 Skill 后：

1. 把改动应用到 Skill 上
2. 重跑所有测试用例到新的 `iteration-<N+1>/` 目录下，**包括 baseline**
   运行。新建 Skill 的 baseline 始终是 "不带 skill"（跨迭代不变）；
   改进已有 Skill 的情况下，baseline 可按判断取原始版本或上次迭代
3. 启动查看器时把 `--previous-workspace` 指向上一轮
4. 等用户说 "我看完了"
5. 读新反馈，继续改进，循环

直到：

- 用户说他满意了
- 反馈全空（都挺好）
- 你已经做不出有意义的提升

## 进阶：盲评对比 (Blind Comparison)

需要 **严格判断两版哪个更好** 时（用户问 "新版真的比旧版好吗？"），
用盲评：把两份输出给一个独立的 Agent，不告诉它哪份是哪版，让它
判断哪份更好，然后分析为什么赢。

见上游 `agents/comparator.md`、`agents/analyzer.md`。需要子 Agent
支持，大多数用户用不上。人工 review 循环通常够用。

## 描述优化 (Description Optimization)

frontmatter 里的 `description` 字段是 Agent 判断 **要不要触发这个
Skill** 的 **主机制**。Skill 写完 / 改完之后，提议帮用户优化描述。

### Step 1 — 生成触发 eval 集

准备 20 条 eval queries，混合 should-trigger 和 should-not-trigger，
存成 JSON：

```json
[
  {"query": "the user prompt", "should_trigger": true},
  {"query": "another prompt", "should_trigger": false}
]
```

queries 必须是真实用户会打的话。不要抽象、要具体：
文件路径、人设上下文、列名、值、公司名、URL、一点点铺垫。
有的是小写、带缩写或口语，长度不一，**边界 case 比清晰 case 更有价值**
（用户后面会签字）。

Bad: `"Format this data"`, `"Extract text from PDF"`, `"Create a chart"`

Good:
`"ok so my boss just sent me this xlsx file (it's in my downloads,"`
`" called something like 'Q4 sales final FINAL v2.xlsx') and she wants me"`
`" to add a column that shows the profit margin as a percentage. The"`
`" revenue is in column C and costs are in column D i think"`

**should-trigger (8–10)**：同一个意图的不同说法，有正式有口语；
覆盖 "用户没明说 Skill 名字/文件类型但显然需要" 的 case；
加一些不太常见的用法；加一些跟别的 Skill 竞争但本 Skill 该赢的。

**should-not-trigger (8–10)**：最有价值的是 **近似miss** — 词、概念
和 Skill 高度相关但其实需要别的东西。想想邻近领域、模糊措辞、
关键词会诱触发但上下文里其实应该用别的工具。

要避免的核心误区：别让负样本 **一看就不相关**。用 "写个斐波那契"
当 PDF Skill 的反例太容易了 — 什么都没测到。负样本要 **真的诱人**。

### Step 2 — 和用户复核 eval 集

呈现 eval 集给用户复核。在 CyberClaw 下由 skill_creator facade 生成
Artifact，前端展示供用户编辑 / 标注 / 提交。

这一步很关键 — 糟糕的 eval queries 直接毁掉后面的描述。

### Step 3 — 跑优化循环

CyberClaw 下通过 `skill_creator.rs` facade 触发：

```
skill_creator.tune_description({
    eval_set: <artifact-id>,
    skill_path: <skill-id>,
    max_iterations: 5,
})
```

facade 内部：
- 拉 `EvolutionOrchestrator` 生成候选描述变体
- 每个变体跑三次评估（稳态触发率）
- `VariantSelector` 按 test-set 分数（不是 train）选出最好版本
  — 避免过拟合
- `StagedVerificationGate` 做最终合规检查
- 结果回传 Artifact，含 `best_description` + 每轮分数

**等价上游脚本** 见本 skill 的 `scripts/run_loop.py` 和 `scripts/run_eval.py`
（仅作参考；**CyberClaw 不调用它们**）。

### Skill 触发机制

Skill 带 name + description 出现在 Agent 的 `available_skills` 列表里。
Agent 看 description 决定要不要 **进一步查看** 这个 Skill。重要的是：
Agent **只在自己搞不定的任务** 上去查 Skill — 简单的、一步就能完的
请求（"读一下这个 PDF"）即使描述完全匹配也可能不触发，因为 Agent
用基础工具就能直接做。**复杂、多步、专业** 的请求，描述匹配时会稳定触发。

也就是说，你的 eval queries 得 **够分量**，让 Agent 觉得 "值得查 Skill"。
"读文件 X" 这种简单 query 是差测试用例 — 无论描述多好都不会触发。

### Step 4 — 应用结果

从 Artifact 里取 `best_description`，更新 SKILL.md 的 frontmatter。
把前后对比和分数展示给用户。

## Cowork / 远程环境说明

- CyberClaw 支持通过 `SubAgentOrchestrator` 并行 — 主工作流（同回合
  派发测试用例、跑 baseline、评分等）都在
- 无浏览器 / 无显示的环境下，`skill_creator.rs` facade 把查看器
  写到 Artifact + 静态 HTML，前端通过 URL 链接展示；不要起 server

## 更新一个已有 Skill

用户可能是想 **改** 一个已有的 Skill，不是新建：

- **保留原名**。留意 Skill 的目录名和 `name` frontmatter 字段 —
  原样用。别自作主张改成 `skill-name-v2`
- **改之前先拷到可写位置**。已安装的 Skill 路径可能只读。
  拷到临时目录里改，再从拷贝打包
- 手动打包时先 **落到临时区**，再复制到输出目录 — 直接写可能因
  权限失败

## 参考文件

claude-code 原版 `agents/` 目录里有专用子 Agent 的指令，需要时读对应的：

- `agents/grader.md` — 如何对断言评分
- `agents/comparator.md` — 如何做两份输出的盲评
- `agents/analyzer.md` — 如何分析一方赢在哪

`references/schemas.md` — `evals.json` / `grading.json` 等的 JSON 结构。

## 循环本体（复述一遍）

- 搞清楚 Skill 做什么
- 起草 / 修改 Skill
- 用载入该 Skill 的 Agent 跑测试 prompt
- 和用户一起评估输出
  - 生成 `benchmark.json`，启动查看器（通过 facade）给用户看
  - 跑定量 eval
- 循环直到都满意
- 最后打包发给用户

放进 TodoList 里确保别忘了，尤其是 "**先** 生成查看器让用户看结果、
**然后再** 自己改 Skill"。

---

**Source acknowledgement**: 原方法论来自 Anthropic 的
`skills/skill-creator/SKILL.md`（claude-code）。本 SKILL.md 在
CyberClaw 语境下做了重写：把所有 "直接跑 Python 脚本" 的路径
重新映射到 `control-plane/skill_creator.rs` facade +
`EvolutionOrchestrator`；子 Agent 派生统一走
`SubAgentOrchestrator`；eval / benchmark / feedback 的落盘
换成 Artifact 层，OMC 兼容 harness 下保留原 `iteration-N/` 目录
结构作为镜像。`scripts/` 目录只作为 **行为契约参考**，不是
运行时依赖 — 详情见 `scripts/README.md`。
