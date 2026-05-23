# CyberClaw One Pager v2.0

## 一句话定义

**CyberClaw 是一个面向开发者和工程组织的受控智能体平台，用于组织 Agent、Skill、Connector 与 Platform Plugin 生态，并提供统一执行、治理控制以及内建的安全与可观测能力。**

---

## 平台定位

CyberClaw 提供一套轻量级、可扩展、可治理的智能体运行底座，面向：

- `R&D`
- `DevOps / Platform Engineering`
- `Security / AppSec / SecOps / GRC`
- `Compliance / Audit`
- 高治理要求的企业自动化场景
- 更可控的个人智能助手

安全不是唯一业务边界，而是平台内生能力。

---

## 核心价值

CyberClaw 关注四件事：

1. **多 Agent 协同**
把角色分工和工作流组织成可组合的执行体系。

2. **统一执行**
所有任务进入统一执行入口，形成可管理的执行树。

3. **治理优先**
所有高风险动作都经过权限、策略、审批与审计。

4. **生态可扩展**
通过 Agent、Skill、Connector、Platform Plugin 四类对象建设生态。

---

## 核心对象

### Agent
谁来做。

负责角色主体、默认行为边界、默认 skill/connector 集合、风险上限、workspace 和记忆视角。

### Skill
怎么做。

兼容 `Claude/Codex Skill`，承载方法、知识、模板和 playbook。

### Connector
用什么做。

统一承载外部系统、工具、模型、渠道和外部 Agent Runtime 接入。

### Platform Plugin
平台怎么被增强。

负责事件监听、前后置拦截、审计增强、自动化挂点和平台级扩展。

---

## 平台对象

### 业务对象
- `Task`
- `Case`
- `Workflow`
- `Artifact`

### 运行对象
- `Identity / Actor / Tenant`
- `Workspace / IsolationProfile`
- `Session`
- `Execution / RunState / ExecutionTree`
- `Capability`
- `Trigger / Event`
- `Registry / Package / Manifest / Trust`
- `Trace / Correlation`
- `Review / ApprovalStep`
- `Provenance / Lineage`

---

## 运行模型

CyberClaw 的运行模型不是单 Agent 对话，而是**受控执行树**。

```text
Trigger/Event
-> Task / Case
-> Resolver 选择 Agent + Skill + Connector + Workflow
-> Execution Service
-> Governance Gate
-> Capability 调用
-> Artifact / Review / Trace / Provenance
```

关键约束：

1. `MasterAgent` 负责全局编排
2. `BusinessAgent` 可以申请派生 `Subagent`
3. 所有 Subagent 进入 `ExecutionTree`
4. 内部不采用自由 Agent 通信网络
5. 所有高风险动作都经过治理门禁

---

## 安全与治理

CyberClaw 默认内置两层安全能力。

### 平台安全强制层
- Prompt 注入检测
- Skill / Package 信任扫描
- Runtime 异常检测
- 权限强制执行
- Policy 决策
- Security Event 汇聚

### 安全监督 Agent
- `SecuritySupervisorAgent`
- 负责全局风险分析、巡检、review 建议和安全报告
- 不替代平台强制控制

---

## 可观察、可审计、可溯源

CyberClaw 默认提供：

1. **可观察**
记录 Trigger、Task、Execution、Connector 调用和 Workflow 流转。

2. **可审计**
记录发起者、执行者、审批者、Capability 调用和最终结果。

3. **可溯源**
记录 Artifact 与 Execution、Skill、Connector、Review 的血缘关系。

平台输出要求是：

- 可解释
- 可追踪
- 可验证
- 可控制

---

## 为什么适合高治理场景

CyberClaw 不是简单的“聊天机器人 + 工具调用”。

它适合：

- 需要审批和 review 的动作
- 需要审计和追踪的自动化流程
- 需要多角色协作的复杂任务
- 需要明确工作区和权限边界的执行场景
- 需要把知识、流程、工具长期沉淀成生态对象的平台

---

## 典型场景

1. 代码审查与工程协作
2. DevOps 与平台自动化
3. 告警研判、事件响应、GRC、合规
4. 报告生成与多阶段审核
5. 企业内部知识与流程自动化
6. 更可控的个人专业助手

---

## 最终定义

> **CyberClaw 是一个面向开发者和工程组织的轻量级受控智能体平台。它将角色分工、工作流、工具接入与知识沉淀抽象为可组合的多 Agent 协同体系，并围绕 Agent、Skill、Connector、Platform Plugin 四类核心对象建设生态。平台内置 MasterAgent 负责全局编排，支持业务 Agent 派生 Subagent 进行并行作业；所有执行统一由 Execution Service 调度，所有高风险动作统一由 Governance Gate 治理，并由 Observability、Audit、Trace 与 Provenance 提供可观察、可审计、可溯源的底座能力。安全是平台的内生能力，不是唯一业务边界。**
