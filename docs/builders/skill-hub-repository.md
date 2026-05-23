# Skill Hub Repository Guide

这份文档不讨论 Skill 的运行时实现，而是讨论一个独立的 `Skill Hub` GitHub 仓库和站点应该如何呈现，才能既利于生态发现，又不破坏 CyberClaw 的治理定位。

## Skill Hub 是什么

Skill Hub 是 CyberClaw 生态中的 Skill 发现与分发入口。

它应该承担：

- Skill 目录浏览
- 分类和搜索
- Skill 详情展示
- 安装和发布说明
- 来源、维护者与风险信号展示

它不应该承担：

- 直接替代主仓库 README 或产品文档
- 直接执行 Skill
- 绕过治理变成“无约束插件市场”

## 仓库与域名建议

### 公开域名

- 主站首页：`cyberclawlabs.ai`
- 主文档：`cyberclawlabs.ai/docs`
- Skill Hub：`skillhub.cyberclawlabs.ai`

### GitHub 仓库职责

| 仓库 | 作用 |
| --- | --- |
| `cyberclaw` | 主代码仓库、主 README、产品 docs、架构与实现文档 |
| `cyberclawlabs.ai` | GitHub Pages 站点或发布产物仓库 |
| `skillhub` | Skill 目录、投稿、详情页、分类页、安装与信任信号展示 |

## Skill Hub 仓库首页应该怎么写

Skill Hub 仓库根 `README` 应该只做四件事：

1. 一句话说明它是 CyberClaw 的 Skill 发现仓库
2. 给出站点入口和快速浏览入口
3. 告诉提交者如何发布新 Skill
4. 明确说明它不替代主仓库的产品文档和运行时

推荐结构：

1. 标题与一句话定位
2. `Browse Skills`、`Submit a Skill`、`Read Install Guide` 三个入口
3. Featured categories
4. 安装方式总览
5. 投稿流程和审核说明
6. 与主仓库、主 docs 的关系

## 仓库目录建议

下面的结构足够支撑首版，不需要过度设计：

| 路径 | 作用 |
| --- | --- |
| `README.md` | 仓库首页与入口导航 |
| `docs/` | 投稿说明、审核规则、元信息字段说明 |
| `catalog/skills/` | Skill 条目元数据，适合机器读取 |
| `content/skills/` | Skill 详情页内容 |
| `content/categories/` | 分类页内容 |
| `schemas/` | Skill Hub 条目 schema |
| `assets/` | 封面图、图标、示例截图 |

如果首版只做静态站点，`catalog/skills/` 可以使用 `json` 或 `yaml` 存索引，保持简单即可。

## 站点信息架构

### 1. Landing Page

首页至少应包含：

- Hub 定位
- Featured Skills
- 分类入口
- 投稿入口
- 安装入口

### 2. Category Pages

首批建议分类：

- Web3
- Security
- DevTools
- Productivity
- Research
- Automation

### 3. Skill Detail Pages

每个 Skill 详情页至少展示：

- 名称
- 一句话描述
- 类别与标签
- 兼容格式
- 运行前提
- 安装方式
- 维护者 / 来源
- 风险说明
- 当前语言与可用翻译

### 4. Submit Page

投稿页需要明确：

- 必需元信息
- PR 提交流程
- 审核规则
- 信任信号和风险标记

## Skill 卡片应该如何呈现

Skill 列表卡片建议统一展示这些字段：

| 字段 | 说明 |
| --- | --- |
| `name` | Skill 名称 |
| `summary` | 一句话价值说明 |
| `category` | 主分类 |
| `tags` | 补充标签 |
| `compatibility` | 支持的 Skill 格式或运行时 |
| `maintainer` | 维护者或组织 |
| `source` | 来源仓库或发布渠道 |
| `risk_level` | 风险标记 |
| `available_locales` | 已提供语言 |
| `install_method` | 安装方式摘要 |

这些字段已经足够支撑 MVP，不需要一开始就引入复杂评分系统。

## 安装说明应该如何写

Skill Hub 的安装说明建议只保留三类：

1. 仓库安装
2. 手动安装
3. 后续 CLI 安装预留说明

不要在首版引入模糊的“一键市场安装”说法，除非对应实现已经存在。

## 信任与风险展示

因为 CyberClaw 强调受控执行，Skill Hub 页面必须显式展示信任上下文：

- 维护者身份
- 来源仓库
- 最近更新时间
- 兼容格式
- 风险说明
- 审核状态或信任标记

这样可以避免 Skill Hub 被理解成纯营销式插件市场。

## 与主仓库的关系

主仓库负责：

- 平台定位
- 产品 docs
- 架构和实现文档
- Builder Guide

Skill Hub 负责：

- Skill 发现
- Skill 分类
- Skill 投稿
- Skill 详情展示

如果一个内容是在解释 CyberClaw 平台是什么，应该留在主仓库；如果一个内容是在展示某个 Skill 怎么被发现和安装，应该放在 Skill Hub。

## 首版落地建议

如果你要做 MVP，优先顺序建议是：

1. 先做仓库 `README`
2. 再做首页、分类页、详情页三层
3. 再补投稿与审核文档
4. 最后补更复杂的搜索、评分和自动化发布

这能保持 KISS，也符合当前开源发布阶段。
