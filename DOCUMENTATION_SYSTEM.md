# CyberClaw 文档管理体系

- Status: Active
- Scope: Repository
- Owner: CyberClaw Maintainers
- Last Updated: 2026-03-20

本文档定义 CyberClaw 仓库中根目录 Markdown、`docs/` 正式文档体系和 crate 本地文档的统一管理规则。

目标：

1. 保持只有一套文档治理规则
2. 明确根目录文件、`docs/` 与 crate 本地文档的职责边界
3. 避免同一主题在多个位置重复演化
4. 让 AI 代理和人工开发者都能快速判断信息应写在哪里

## 1. 总原则

CyberClaw 的文档体系分为三层：

1. 根目录 Markdown
2. `docs/` 正式文档体系
3. crate 本地文档

三者不是并列竞争关系，而是：

- 根目录 Markdown 负责仓库入口、协作规则、项目政策、发布记录
- `docs/` 负责架构、实施、业务三大正式知识域
- crate 本地文档负责单个 crate 的职责、局部开发说明和 crate 级变更记录

一句话：

> 根目录文件负责“入口和政策”，`docs/` 负责“体系化知识”，crate 文档负责“局部实现说明”。

## 2. 根目录 Markdown 的职责

当前根目录文件：

- `README.md`
- `CHANGELOG.md`
- `CONTRIBUTING.md`
- `DEVELOPMENT.md`
- `SECURITY.md`
- `FIXES.md`
- `CLAUDE.md`
- `AGENTS.md`
- `DOCUMENTATION_SYSTEM.md`
- `DOCUMENT_METADATA_TEMPLATE.md`

### 2.1 `README.md`

定位：

- 项目入口
- 对外概览
- 快速导航

### 2.2 `CHANGELOG.md`

定位：

- 仓库级 changelog
- 跨 crate、跨阶段、跨主题的重要变更记录

### 2.3 `CONTRIBUTING.md`

定位：

- 贡献规则
- PR / review / 文档同步要求

### 2.4 `DEVELOPMENT.md`

定位：

- 本地开发环境
- 构建、测试、调试
- 常见开发操作

### 2.5 `SECURITY.md`

定位：

- 安全政策
- 漏洞报告流程
- 支持版本
- 安全能力概览

### 2.6 `FIXES.md`

定位：

- 根目录修复索引
- 指向 `docs/implementation/fixes/` 中的详细修复文档

### 2.7 `CLAUDE.md` / `AGENTS.md`

定位：

- AI 代理项目级约束文件

### 2.8 `DOCUMENTATION_SYSTEM.md`

定位：

- 仓库级文档治理规则
- 根目录 Markdown、`docs/` 与 crate 文档的职责定义

### 2.9 `DOCUMENT_METADATA_TEMPLATE.md`

定位：

- 文档页头元信息模板
- 用于统一入口文档、目录索引文档和 crate 本地文档的状态字段

## 3. `docs/` 的职责

`docs/` 是正式知识体系，分为三层：

- `docs/architecture`
- `docs/implementation`
- `docs/business`

### 3.1 `docs/architecture`

负责：

- 平台设计
- 对象模型
- 运行时
- 记忆与上下文工程
- 知识检索
- code maps

### 3.2 `docs/implementation`

负责：

- 路线图
- 执行 prompt
- 实现报告
- 修复记录
- 评审记录
- 发布记录

### 3.3 `docs/business`

负责：

- GTM
- 品牌与推广
- 商业执行计划

## 4. crate 本地文档的职责

crate 本地文档只服务于单个 crate。

### 4.1 crate `README.md`

定位：

- 说明 crate 目标、模块边界、公共接口、局部验证方式
- 链接到相关仓库级和架构级文档

应包含：

- crate 定位
- 当前主要模块
- 开发与验证入口
- 相关文档链接

不应包含：

- 仓库级路线图全文
- 平台总架构重复说明
- 容易快速过时的硬编码测试数字 badge

### 4.2 crate `CHANGELOG.md`

定位：

- 单个 crate 的显著变更记录

应包含：

- crate 级 Added / Changed / Fixed / Removed
- crate 内 breaking changes
- 必要的迁移提示

不应包含：

- 仓库级路线图全文
- 大段阶段报告
- 与根 `CHANGELOG.md` 重复的仓库级说明

## 5. 信息写入规则

按内容类型判断：

1. 仓库入口信息 → 根 `README.md`
2. 仓库级版本变更 → 根 `CHANGELOG.md`
3. 贡献协作规范 → 根 `CONTRIBUTING.md`
4. 本地开发说明 → 根 `DEVELOPMENT.md`
5. 安全政策与漏洞流程 → 根 `SECURITY.md`
6. 文档治理规则 → 根 `DOCUMENTATION_SYSTEM.md`
7. 元信息模板 → 根 `DOCUMENT_METADATA_TEMPLATE.md`
8. 详细架构设计 → `docs/architecture/*`
9. 实现记录 / 修复 / prompt / review → `docs/implementation/*`
10. 业务推广 → `docs/business/*`
11. 单个 crate 的说明和 changelog → crate 目录内文档

## 6. 一个主题只能有一个主文档

例如：

- 架构设计主文档在 `docs/architecture/`
- 修复详情主文档在 `docs/implementation/fixes/`
- crate 局部说明主文档在对应 crate 目录
- 根目录只保留摘要和索引，不复制全文

## 7. 一致性规则

### 7.1 发生以下变化时必须同步

1. 文档路径变化
2. 目录结构变化
3. 核心对象模型变化
4. 路线图阶段变化
5. 修复文档归档位置变化
6. crate 边界或 crate 对外说明变化

### 7.2 最少同步清单

至少更新：

1. 对应文档正文
2. 所属目录 `README.md`
3. [docs/INDEX.md](docs/INDEX.md)
4. 必要时更新根目录入口文件
5. 若为 crate 局部变化，更新 crate `README.md` 或 crate `CHANGELOG.md`

## 8. 元信息模板规则

入口页、规则页、crate README、crate CHANGELOG 建议统一使用以下页头字段：

- `Status`
- `Scope`
- `Owner`
- `Last Updated`

模板见 [DOCUMENT_METADATA_TEMPLATE.md](DOCUMENT_METADATA_TEMPLATE.md)。

## 9. 当前定稿

当前仓库按以下规则执行：

1. 根 `CHANGELOG.md` = 仓库级变更记录
2. crate 内 `CHANGELOG.md` = crate 级变更记录
3. 根 `FIXES.md` = 修复索引
4. 修复详情 = `docs/implementation/fixes/*`
5. 正式架构知识 = `docs/architecture/*`
6. 实施材料 = `docs/implementation/*`
7. 业务材料 = `docs/business/*`
8. crate README = crate 局部职责说明

## 10. 入口

- [项目根 README](README.md)
- [文档总索引](docs/INDEX.md)
- [文档中心 README](docs/README.md)
- [文档元信息模板](DOCUMENT_METADATA_TEMPLATE.md)
