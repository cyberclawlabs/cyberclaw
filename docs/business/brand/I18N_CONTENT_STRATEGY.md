# I18N Content Strategy

CyberClaw 的公开内容应按 i18n 方式设计，而不是发布后再补翻译。

## 目标

让以下公开表面具备稳定的多语种扩展能力：

- GitHub README
- GitHub Pages Homepage
- Docs 门户
- Skill Hub

## 语种分层

### Tier 1: 首发维护语种

- `en`
- `zh-CN`

### Tier 2: 近期扩展语种

- `ja`
- `ko`
- `es`

### Tier 3: 社区扩展语种

- `fr`
- `de`
- `pt-BR`

## 仓库与站点策略

### GitHub 仓库

GitHub 主仓库推荐：

- `README.md` 作为 English canonical entry
- `README.zh-CN.md` 作为中文维护版本
- 后续语种使用独立 locale 文件，不在同一个 README 中堆叠

### 官网与 Docs

官网和 Docs 推荐使用显式 locale 路由：

- `/en/`
- `/zh-cn/`
- `/ja/`
- `/ko/`
- `/es/`

至少应支持：

- 语言切换器
- 默认语言回退
- `hreflang` 标记

### Skill Hub

Skill Hub 的浏览和详情页需要 locale-aware 数据结构，至少支持：

- 当前页面语言
- Skill 默认语言
- 可用翻译列表
- 翻译状态

## 内容源策略

推荐做法：

1. 英文作为公开 canonical source
2. 简体中文作为第一维护翻译
3. 其他语种按优先级扩展

对于面向中国开发者的运营素材，可以先中文起草，再回写英文 canonical 版本，但对外发布资产应保持英文主源清晰。

## 术语治理

跨语种内容必须保持术语一致：

- `Agent`
- `Skill`
- `Connector`
- `Capability`
- `Platform Plugin`

不要因为翻译需要改变对象边界。

## 内容模型建议

对 Homepage、Docs、Skill Hub 的内容数据，建议至少携带：

- `locale`
- `source_locale`
- `translation_status`
- `last_reviewed_at`
- `owner`

## 发布原则

对外内容中，所有语种都必须同步区分：

- Implemented
- In Progress
- Roadmap

不能出现英文版本更保守、中文版本更激进，或反过来的情况。
