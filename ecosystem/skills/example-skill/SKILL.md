---
name: Example Skill
version: 1.0.0
description: A comprehensive example skill demonstrating CyberClaw Skill capabilities
author: CyberClaw Team
tags: [example, data-processing, automation, productivity]
license: Apache-2.0
homepage: https://cyberclaw.io/skills/example
repository: https://github.com/cyberclaw/example-skill

capabilities:
  - id: process_data
    description: Process and transform data in various formats
    parameters:
      input:
        type: string
        description: Input file path or data string
        required: true
      format:
        type: string
        enum: [csv, json, xml, yaml]
        default: json
        description: Input data format
      output:
        type: string
        description: Output file path
        required: true
      transformations:
        type: array
        description: List of transformations to apply
        items:
          type: object
          properties:
            type:
              type: string
              enum: [filter, map, aggregate, sort]
            config:
              type: object

  - id: generate_report
    description: Generate detailed reports from processed data
    parameters:
      data:
        type: object
        description: Processed data object
        required: true
      template:
        type: string
        enum: [summary, detailed, executive]
        default: summary
        description: Report template type
      format:
        type: string
        enum: [markdown, html, pdf]
        default: markdown
        description: Output format

  - id: validate_schema
    description: Validate data against a JSON schema
    parameters:
      data:
        type: object
        description: Data to validate
        required: true
      schema:
        type: object
        description: JSON schema for validation
        required: true

dependencies:
  python: ">=3.8"
  packages:
    - pandas>=1.3.0
    - numpy>=1.21.0
    - pyyaml>=5.4.0
    - jsonschema>=3.2.0
    - jinja2>=3.0.0
---

# Example Skill

这是一个综合示例 Skill，展示了 CyberClaw Skill 的各种能力。

## 🎯 核心功能

### 1. 数据处理 (process_data)

支持多种数据格式的读取、转换和处理：

- **CSV**: 表格数据处理
- **JSON**: 结构化数据处理
- **XML**: 标记语言数据处理
- **YAML**: 配置文件处理

### 2. 报告生成 (generate_report)

从处理后的数据生成专业报告：

- **摘要报告**: 快速概览
- **详细报告**: 完整分析
- **执行报告**: 决策支持

### 3. 模式验证 (validate_schema)

使用 JSON Schema 验证数据结构，确保数据质量。

## 📋 使用方法

### 快速开始

```bash
# 安装依赖
pip install -r requirements.txt

# 运行主脚本
./scripts/main.py --help
```

### 数据处理示例

```python
from example_skill import process_data

# 处理 CSV 文件
result = process_data(
    input="data.csv",
    format="csv",
    output="processed.json",
    transformations=[
        {
            "type": "filter",
            "config": {
                "column": "age",
                "operator": ">",
                "value": 18
            }
        },
        {
            "type": "aggregate",
            "config": {
                "group_by": "department",
                "aggregations": {
                    "salary": "mean",
                    "count": "count"
                }
            }
        }
    ]
)

print(f"Processed {result['records']} records")
```

### 报告生成示例

```python
from example_skill import generate_report

# 生成 Markdown 报告
report = generate_report(
    data=processed_data,
    template="detailed",
    format="markdown"
)

# 保存报告
with open("report.md", "w") as f:
    f.write(report)
```

### 模式验证示例

```python
from example_skill import validate_schema

# 定义模式
schema = {
    "type": "object",
    "properties": {
        "name": {"type": "string"},
        "age": {"type": "integer", "minimum": 0},
        "email": {"type": "string", "format": "email"}
    },
    "required": ["name", "email"]
}

# 验证数据
data = {
    "name": "John Doe",
    "age": 30,
    "email": "john@example.com"
}

result = validate_schema(data, schema)
if result["valid"]:
    print("✅ Data is valid")
else:
    print(f"❌ Validation errors: {result['errors']}")
```

## 🔧 配置选项

### 环境变量

| 变量名 | 默认值 | 说明 |
|--------|--------|------|
| `SKILL_DEBUG` | `false` | 启用调试模式 |
| `SKILL_CACHE` | `true` | 启用结果缓存 |
| `SKILL_TIMEOUT` | `30` | 执行超时（秒） |
| `SKILL_MAX_MEMORY` | `512` | 最大内存使用（MB） |

### 配置文件

创建 `config.yaml`:

```yaml
processing:
  batch_size: 1000
  parallel: true
  workers: 4

reporting:
  include_metadata: true
  timestamp_format: ISO8601

validation:
  strict_mode: false
  allow_additional_properties: true
```

## 📊 支持的转换

### Filter (过滤)

```json
{
  "type": "filter",
  "config": {
    "column": "status",
    "operator": "in",
    "value": ["active", "pending"]
  }
}
```

### Map (映射)

```json
{
  "type": "map",
  "config": {
    "transformations": {
      "full_name": "{first_name} {last_name}",
      "age_group": "lambda x: 'adult' if x['age'] >= 18 else 'minor'"
    }
  }
}
```

### Aggregate (聚合)

```json
{
  "type": "aggregate",
  "config": {
    "group_by": ["department", "location"],
    "aggregations": {
      "total_salary": "sum",
      "avg_age": "mean",
      "employee_count": "count"
    }
  }
}
```

### Sort (排序)

```json
{
  "type": "sort",
  "config": {
    "by": ["score", "name"],
    "ascending": [false, true]
  }
}
```

## 🧪 测试

### 运行单元测试

```bash
python -m pytest tests/unit/
```

### 运行集成测试

```bash
python -m pytest tests/integration/
```

### 测试覆盖率

```bash
pytest --cov=example_skill --cov-report=html
```

## 📈 性能优化

- **批处理**: 使用 `batch_size` 参数处理大数据集
- **并行处理**: 启用 `parallel` 选项利用多核
- **内存管理**: 设置 `SKILL_MAX_MEMORY` 限制内存使用
- **缓存**: 启用 `SKILL_CACHE` 避免重复计算

## 🔍 故障排除

### 常见问题

#### 内存不足

**问题**: `MemoryError: Unable to allocate array`

**解决方案**:
- 减小批处理大小
- 增加 `SKILL_MAX_MEMORY`
- 使用流式处理

#### 依赖冲突

**问题**: `ImportError: No module named 'pandas'`

**解决方案**:
```bash
pip install --upgrade -r requirements.txt
```

#### 格式不支持

**问题**: `ValueError: Unsupported format: parquet`

**解决方案**:
- 安装额外依赖: `pip install pyarrow`
- 转换为支持的格式

## 📚 API 参考

### process_data

```python
def process_data(
    input: Union[str, Dict, List],
    format: str = "json",
    output: Optional[str] = None,
    transformations: List[Dict] = None
) -> Dict[str, Any]:
    """
    处理和转换数据

    Args:
        input: 输入数据或文件路径
        format: 数据格式
        output: 输出文件路径
        transformations: 转换列表

    Returns:
        处理结果字典
    """
```

### generate_report

```python
def generate_report(
    data: Dict[str, Any],
    template: str = "summary",
    format: str = "markdown"
) -> str:
    """
    生成报告

    Args:
        data: 输入数据
        template: 模板类型
        format: 输出格式

    Returns:
        报告内容字符串
    """
```

### validate_schema

```python
def validate_schema(
    data: Dict[str, Any],
    schema: Dict[str, Any]
) -> Dict[str, Any]:
    """
    验证数据模式

    Args:
        data: 待验证数据
        schema: JSON Schema

    Returns:
        验证结果
    """
```

## 🤝 贡献指南

欢迎贡献代码和文档！

1. Fork 项目
2. 创建功能分支
3. 提交更改
4. 推送到分支
5. 创建 Pull Request

## 📄 许可证

Apache 2.0 License - 详见 [LICENSE](LICENSE) 文件

## 🆘 支持

- 📖 文档: https://docs.cyberclaw.io/skills/example
- 💬 论坛: https://forum.cyberclaw.io
- 🐛 Issues: https://github.com/cyberclaw/example-skill/issues
- 📧 Email: support@cyberclaw.io

## 更新日志

### v1.0.0 (2024-03-23)
- 🎉 初始版本发布
- ✨ 支持 CSV, JSON, XML, YAML 格式
- 📊 实现数据转换功能
- 📝 添加报告生成
- ✅ 添加模式验证
- 🧪 完整测试覆盖