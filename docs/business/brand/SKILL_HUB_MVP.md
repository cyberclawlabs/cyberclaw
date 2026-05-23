# Skill Hub MVP

Skill Hub 是 CyberClaw 生态层的 Skill 发现入口。

## 定位

Skill Hub 不是简单的文档列表，而是目录型 Hub：

- 可以浏览
- 可以分类
- 可以查看 Skill 详情
- 可以理解安装方式
- 可以理解来源和信任信号

## 建议域名

- `skillhub.cyberclawlabs.ai`

## I18N Requirement

Skill Hub 必须是 locale-aware 的目录型 Hub，而不是只支持单语言的静态页。

### 首发维护语种

- `en`
- `zh-CN`

### 近期扩展语种

- `ja`
- `ko`
- `es`

## MVP 范围

### 1. Landing Page

展示：

- Hub 定位
- Featured Skills
- 入口分类
- 投稿入口

### 2. Category Pages

首批建议分类：

- Web3
- Security
- DevTools
- Productivity
- Research
- Automation

### 3. Skill Detail Pages

每个 Skill 至少展示：

- 名称
- 描述
- 类别与标签
- 兼容格式
- 运行前提
- 安装方式
- 维护者 / 来源
- 风险说明
- 当前语言
- 可用翻译
- 翻译状态

### 4. Install Guidance

MVP 阶段至少支持：

- 仓库安装
- 手动安装
- 后续 CLI 安装预留说明

### 5. Publish Guidance

明确说明：

- Skill 目录规范
- 必需元信息
- 提交流程
- 审核与信任信号策略

## 表达原则

Skill Hub 的文案应强调：

- 生态发现
- 兼容接入
- 来源可信度
- 不是“无约束插件市场”

## 数据字段建议

Skill Hub 索引数据建议具备：

- `locale`
- `source_locale`
- `available_locales`
- `translation_status`

## 与主仓库的关系

主仓库负责平台叙事和 builder 入口。

Skill Hub 负责 Skill 发现、分类和投稿，不替代主仓库中的：

- 平台 README
- 产品文档
- 深度架构文档
