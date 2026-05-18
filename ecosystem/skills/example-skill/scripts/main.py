#!/usr/bin/env python3
"""
Example Skill - 主执行脚本

这是 CyberClaw Example Skill 的主入口脚本。
"""

import json
import argparse
import sys
from pathlib import Path
from typing import Dict, Any, List, Optional, Union
import pandas as pd
import yaml
import logging
from datetime import datetime

# 添加父目录到路径
sys.path.insert(0, str(Path(__file__).parent.parent))

# 设置日志
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# ==================== 核心功能实现 ====================

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
        format: 数据格式 (csv, json, xml, yaml)
        output: 输出文件路径
        transformations: 转换操作列表

    Returns:
        处理结果字典
    """
    logger.info(f"Processing data with format: {format}")

    # 1. 加载数据
    data = load_data(input, format)
    logger.info(f"Loaded {len(data)} records")

    # 2. 应用转换
    if transformations:
        for transform in transformations:
            logger.info(f"Applying transformation: {transform['type']}")
            data = apply_transformation(data, transform)

    # 3. 保存输出
    if output:
        save_data(data, output)
        logger.info(f"Saved results to: {output}")

    return {
        "success": True,
        "records": len(data) if isinstance(data, list) else 1,
        "output_path": output,
        "transformations_applied": len(transformations) if transformations else 0
    }


def generate_report(
    data: Dict[str, Any],
    template: str = "summary",
    format: str = "markdown"
) -> str:
    """
    生成报告

    Args:
        data: 输入数据
        template: 模板类型 (summary, detailed, executive)
        format: 输出格式 (markdown, html, pdf)

    Returns:
        报告内容
    """
    logger.info(f"Generating {template} report in {format} format")

    # 选择模板
    if template == "summary":
        report = generate_summary_report(data)
    elif template == "detailed":
        report = generate_detailed_report(data)
    elif template == "executive":
        report = generate_executive_report(data)
    else:
        raise ValueError(f"Unknown template: {template}")

    # 转换格式
    if format == "html":
        report = convert_to_html(report)
    elif format == "pdf":
        report = convert_to_pdf(report)

    return report


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
    import jsonschema

    logger.info("Validating data against schema")

    try:
        jsonschema.validate(instance=data, schema=schema)
        return {
            "valid": True,
            "errors": []
        }
    except jsonschema.exceptions.ValidationError as e:
        return {
            "valid": False,
            "errors": [str(e)]
        }
    except jsonschema.exceptions.SchemaError as e:
        return {
            "valid": False,
            "errors": [f"Schema error: {str(e)}"]
        }


# ==================== 辅助函数 ====================

def load_data(input: Union[str, Dict, List], format: str) -> Any:
    """加载数据"""
    if isinstance(input, str) and Path(input).exists():
        # 从文件加载
        file_path = Path(input)
        if format == "csv":
            return pd.read_csv(file_path).to_dict('records')
        elif format == "json":
            with open(file_path) as f:
                return json.load(f)
        elif format == "yaml":
            with open(file_path) as f:
                return yaml.safe_load(f)
        elif format == "xml":
            return pd.read_xml(file_path).to_dict('records')
    else:
        # 直接使用输入数据
        return input


def save_data(data: Any, output_path: str):
    """保存数据"""
    path = Path(output_path)
    path.parent.mkdir(parents=True, exist_ok=True)

    if path.suffix == '.csv':
        df = pd.DataFrame(data)
        df.to_csv(path, index=False)
    elif path.suffix == '.json':
        with open(path, 'w') as f:
            json.dump(data, f, indent=2, default=str)
    elif path.suffix in ['.yaml', '.yml']:
        with open(path, 'w') as f:
            yaml.dump(data, f, default_flow_style=False)
    else:
        # 默认保存为 JSON
        with open(path, 'w') as f:
            json.dump(data, f, indent=2, default=str)


def apply_transformation(data: Any, transform: Dict) -> Any:
    """应用单个转换"""
    transform_type = transform['type']
    config = transform.get('config', {})

    if transform_type == 'filter':
        return apply_filter(data, config)
    elif transform_type == 'map':
        return apply_map(data, config)
    elif transform_type == 'aggregate':
        return apply_aggregate(data, config)
    elif transform_type == 'sort':
        return apply_sort(data, config)
    else:
        logger.warning(f"Unknown transformation type: {transform_type}")
        return data


def apply_filter(data: List[Dict], config: Dict) -> List[Dict]:
    """应用过滤转换"""
    if not isinstance(data, list):
        return data

    column = config.get('column')
    operator = config.get('operator', '==')
    value = config.get('value')

    filtered = []
    for item in data:
        if column not in item:
            continue

        item_value = item[column]

        if operator == '==':
            if item_value == value:
                filtered.append(item)
        elif operator == '!=':
            if item_value != value:
                filtered.append(item)
        elif operator == '>':
            if item_value > value:
                filtered.append(item)
        elif operator == '<':
            if item_value < value:
                filtered.append(item)
        elif operator == 'in':
            if item_value in value:
                filtered.append(item)
        elif operator == 'contains':
            if value in str(item_value):
                filtered.append(item)

    return filtered


def apply_map(data: List[Dict], config: Dict) -> List[Dict]:
    """应用映射转换"""
    if not isinstance(data, list):
        return data

    transformations = config.get('transformations', {})

    mapped = []
    for item in data:
        new_item = item.copy()
        for key, transformation in transformations.items():
            if isinstance(transformation, str):
                # 简单字符串模板
                new_item[key] = transformation.format(**item)
            elif callable(transformation):
                # 函数转换
                new_item[key] = transformation(item)

        mapped.append(new_item)

    return mapped


def apply_aggregate(data: List[Dict], config: Dict) -> List[Dict]:
    """应用聚合转换"""
    if not isinstance(data, list) or not data:
        return data

    df = pd.DataFrame(data)
    group_by = config.get('group_by', [])
    aggregations = config.get('aggregations', {})

    if not group_by:
        # 全局聚合
        result = {}
        for col, agg_func in aggregations.items():
            if col in df.columns:
                if agg_func == 'count':
                    result[f"{col}_{agg_func}"] = len(df)
                elif agg_func == 'sum':
                    result[f"{col}_{agg_func}"] = df[col].sum()
                elif agg_func == 'mean':
                    result[f"{col}_{agg_func}"] = df[col].mean()
                elif agg_func == 'min':
                    result[f"{col}_{agg_func}"] = df[col].min()
                elif agg_func == 'max':
                    result[f"{col}_{agg_func}"] = df[col].max()
        return [result]
    else:
        # 分组聚合
        grouped = df.groupby(group_by).agg(aggregations)
        return grouped.reset_index().to_dict('records')


def apply_sort(data: List[Dict], config: Dict) -> List[Dict]:
    """应用排序转换"""
    if not isinstance(data, list):
        return data

    by = config.get('by', [])
    ascending = config.get('ascending', True)

    if not by:
        return data

    df = pd.DataFrame(data)
    sorted_df = df.sort_values(by=by, ascending=ascending)
    return sorted_df.to_dict('records')


# ==================== 报告生成函数 ====================

def generate_summary_report(data: Dict) -> str:
    """生成摘要报告"""
    report = f"""# Summary Report

Generated: {datetime.now().isoformat()}

## Overview
- Total Records: {data.get('records', 0)}
- Processing Status: {data.get('status', 'Complete')}

## Key Metrics
{generate_metrics_section(data)}

## Summary
{generate_summary_section(data)}
"""
    return report


def generate_detailed_report(data: Dict) -> str:
    """生成详细报告"""
    report = f"""# Detailed Report

Generated: {datetime.now().isoformat()}

## Executive Summary
{generate_executive_summary(data)}

## Detailed Analysis
{generate_analysis_section(data)}

## Data Quality
{generate_quality_section(data)}

## Recommendations
{generate_recommendations(data)}

## Appendix
{generate_appendix(data)}
"""
    return report


def generate_executive_report(data: Dict) -> str:
    """生成执行报告"""
    report = f"""# Executive Report

Date: {datetime.now().strftime('%Y-%m-%d')}

## Executive Summary

{generate_executive_summary(data)}

## Key Findings

{generate_key_findings(data)}

## Strategic Recommendations

{generate_strategic_recommendations(data)}

## Next Steps

{generate_next_steps(data)}
"""
    return report


def generate_metrics_section(data: Dict) -> str:
    """生成指标部分"""
    metrics = data.get('metrics', {})
    lines = []
    for key, value in metrics.items():
        lines.append(f"- {key}: {value}")
    return '\n'.join(lines) if lines else "No metrics available"


def generate_summary_section(data: Dict) -> str:
    """生成摘要部分"""
    return data.get('summary', 'No summary available')


def generate_executive_summary(data: Dict) -> str:
    """生成执行摘要"""
    return data.get('executive_summary', 'Executive summary to be generated based on data analysis.')


def generate_analysis_section(data: Dict) -> str:
    """生成分析部分"""
    return "Detailed analysis would be performed here based on the data patterns and trends."


def generate_quality_section(data: Dict) -> str:
    """生成质量部分"""
    return "Data quality assessment including completeness, accuracy, and consistency checks."


def generate_recommendations(data: Dict) -> str:
    """生成建议"""
    return "1. Recommendation based on analysis\n2. Another recommendation\n3. Further actions"


def generate_appendix(data: Dict) -> str:
    """生成附录"""
    return "Additional supporting information and raw data references."


def generate_key_findings(data: Dict) -> str:
    """生成关键发现"""
    return "1. Key finding from the data\n2. Important trend identified\n3. Critical insight"


def generate_strategic_recommendations(data: Dict) -> str:
    """生成战略建议"""
    return "1. Strategic action item\n2. Long-term improvement\n3. Risk mitigation strategy"


def generate_next_steps(data: Dict) -> str:
    """生成下一步"""
    return "1. Immediate action required\n2. Short-term goal\n3. Follow-up analysis needed"


def convert_to_html(markdown_content: str) -> str:
    """转换 Markdown 为 HTML"""
    # 简单实现，实际应使用 markdown 库
    html = f"""<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>Report</title>
    <style>
        body {{ font-family: Arial, sans-serif; margin: 40px; }}
        h1 {{ color: #333; }}
        h2 {{ color: #666; }}
    </style>
</head>
<body>
{markdown_content.replace('#', '').replace('\n', '<br>\n')}
</body>
</html>"""
    return html


def convert_to_pdf(content: str) -> str:
    """转换为 PDF (模拟)"""
    # 实际实现需要使用 reportlab 或 weasyprint
    logger.warning("PDF conversion not implemented, returning content as-is")
    return content


# ==================== CLI 接口 ====================

def main():
    """主函数"""
    parser = argparse.ArgumentParser(
        description='CyberClaw Example Skill - Data Processing and Reporting'
    )

    subparsers = parser.add_subparsers(dest='command', help='Available commands')

    # process 命令
    process_parser = subparsers.add_parser('process', help='Process data')
    process_parser.add_argument('--input', required=True, help='Input file or data')
    process_parser.add_argument('--format', default='json',
                               choices=['csv', 'json', 'xml', 'yaml'],
                               help='Input format')
    process_parser.add_argument('--output', required=True, help='Output file')
    process_parser.add_argument('--transformations', type=str,
                               help='JSON string of transformations')

    # report 命令
    report_parser = subparsers.add_parser('report', help='Generate report')
    report_parser.add_argument('--data', required=True, help='Data file')
    report_parser.add_argument('--template', default='summary',
                              choices=['summary', 'detailed', 'executive'],
                              help='Report template')
    report_parser.add_argument('--format', default='markdown',
                              choices=['markdown', 'html', 'pdf'],
                              help='Output format')
    report_parser.add_argument('--output', help='Output file')

    # validate 命令
    validate_parser = subparsers.add_parser('validate', help='Validate schema')
    validate_parser.add_argument('--data', required=True, help='Data file')
    validate_parser.add_argument('--schema', required=True, help='Schema file')

    args = parser.parse_args()

    if not args.command:
        parser.print_help()
        return

    try:
        if args.command == 'process':
            transformations = None
            if args.transformations:
                transformations = json.loads(args.transformations)

            result = process_data(
                input=args.input,
                format=args.format,
                output=args.output,
                transformations=transformations
            )
            print(json.dumps(result, indent=2))

        elif args.command == 'report':
            with open(args.data) as f:
                data = json.load(f)

            report = generate_report(
                data=data,
                template=args.template,
                format=args.format
            )

            if args.output:
                with open(args.output, 'w') as f:
                    f.write(report)
                print(f"Report saved to: {args.output}")
            else:
                print(report)

        elif args.command == 'validate':
            with open(args.data) as f:
                data = json.load(f)
            with open(args.schema) as f:
                schema = json.load(f)

            result = validate_schema(data, schema)
            print(json.dumps(result, indent=2))

    except Exception as e:
        logger.error(f"Error: {e}")
        sys.exit(1)


if __name__ == '__main__':
    main()