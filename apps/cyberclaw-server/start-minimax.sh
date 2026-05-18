#!/bin/bash
# CyberClaw Server 启动脚本 - MiniMax 配置

set -e

echo "🚀 启动 CyberClaw Server (MiniMax Provider)..."
echo ""

# 检查是否已构建
if [ ! -f "../../target/release/cyberclaw-server" ]; then
    echo "⚠️  未找到 release 版本，开始构建..."
    cargo build --release -p cyberclaw-server
    echo "✅ 构建完成"
    echo ""
fi

# 设置 MiniMax 配置
export LLM_PROVIDER=minimax
export LLM_BASE_URL=https://api.minimax.chat/v1
export LLM_DEFAULT_MODEL=minimax-m2.5
export SERVER_PORT=8080

# 从 .env 文件读取 API Key（如果存在）
if [ -f ".env" ]; then
    echo "📝 从 .env 文件加载配置..."
    export $(grep -v '^#' .env | xargs)
fi

# 检查 API Key
if [ -z "$LLM_API_KEY" ]; then
    echo "❌ 错误: LLM_API_KEY 未设置"
    echo ""
    echo "请设置 LLM_API_KEY 环境变量或创建 .env 文件："
    echo "  export LLM_API_KEY=your-minimax-api-key"
    echo ""
    echo "或者复制 .env.example 为 .env 并填入您的 API Key："
    echo "  cp .env.example .env"
    echo "  # 然后编辑 .env 文件"
    exit 1
fi

# 显示配置信息
echo "📋 服务器配置:"
echo "  Provider: $LLM_PROVIDER"
echo "  Model: $LLM_DEFAULT_MODEL"
echo "  Base URL: $LLM_BASE_URL"
echo "  Port: $SERVER_PORT"
echo "  API Key: ${LLM_API_KEY:0:10}..." # 只显示前10个字符
echo ""

# 启动服务器
echo "✅ 启动服务器..."
../../target/release/cyberclaw-server
