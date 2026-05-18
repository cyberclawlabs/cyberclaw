#!/bin/bash
# CyberClaw Server 启动脚本 - 火山引擎 ARK MiniMax 配置
# 参考文档: https://www.volcengine.com/docs/82379/1928261

set -e

echo "🚀 启动 CyberClaw Server (火山引擎 ARK - MiniMax Provider)..."
echo ""

# 检查是否已构建
if [ ! -f "../../target/release/cyberclaw-server" ]; then
    echo "⚠️  未找到 release 版本，开始构建..."
    cargo build --release -p cyberclaw-server
    echo "✅ 构建完成"
    echo ""
fi

# 设置火山引擎 ARK 配置
export LLM_PROVIDER=ark
export LLM_BASE_URL=https://ark.cn-beijing.volces.com/api/v3
export SERVER_PORT=8080

# 从 .env 文件读取 API Key 和模型配置（如果存在）
if [ -f ".env" ]; then
    echo "📝 从 .env 文件加载配置..."
    export $(grep -v '^#' .env | xargs)
fi

# 检查 API Key
if [ -z "$LLM_API_KEY" ]; then
    echo "❌ 错误: LLM_API_KEY 未设置"
    echo ""
    echo "请设置火山引擎 ARK API Key："
    echo "  export LLM_API_KEY=your-ark-api-key"
    echo ""
    echo "或者复制 .env.example 为 .env 并填入您的 API Key："
    echo "  cp .env.example .env"
    echo "  # 然后编辑 .env 文件，设置 LLM_API_KEY"
    echo ""
    echo "获取 API Key："
    echo "  1. 访问火山引擎控制台: https://console.volcengine.com/ark"
    echo "  2. 创建 MiniMax 推理接入点"
    echo "  3. 获取 API Key 和 Endpoint ID（可选）"
    exit 1
fi

# 设置默认模型（如果未设置）
if [ -z "$LLM_DEFAULT_MODEL" ]; then
    export LLM_DEFAULT_MODEL=minimax-abab6.5
    echo "📝 使用默认模型: $LLM_DEFAULT_MODEL"
fi

# 显示配置信息
echo "📋 服务器配置:"
echo "  Provider: $LLM_PROVIDER (火山引擎 ARK)"
echo "  Model: $LLM_DEFAULT_MODEL"
echo "  Base URL: $LLM_BASE_URL"
echo "  Port: $SERVER_PORT"
echo "  API Key: ${LLM_API_KEY:0:20}..." # 只显示前20个字符
if [ -n "$ARK_ENDPOINT_ID" ]; then
    echo "  Endpoint ID: $ARK_ENDPOINT_ID"
fi
echo ""

# 提示信息
echo "💡 使用提示:"
echo "  - 如果使用 Endpoint ID，请在 .env 中设置 ARK_ENDPOINT_ID"
echo "  - 支持的 MiniMax 模型: minimax-abab6.5, minimax-abab6"
echo "  - 也支持豆包模型: doubao-pro-32k, doubao-lite-32k"
echo ""

# 启动服务器
echo "✅ 启动服务器..."
../../target/release/cyberclaw-server
