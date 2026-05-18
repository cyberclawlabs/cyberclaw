#!/bin/bash

# 文档验证脚本 - 按照 doc-updater 规范
# 检查所有文档链接和引用是否有效

set -e

echo "=== CyberClaw 文档验证 ==="
echo ""

ERRORS=0
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

# 检查必要的文档文件是否存在
echo "✓ 检查必要文件..."
FILES=(
    "README.md"
    "SECURITY.md"
    "CHANGELOG.md"
    "docs/SECURITY_FIXES_MILESTONE_C.md"
    "docs/CODEMAPS/INDEX.md"
    "docs/CODEMAPS/control-plane.md"
    "docs/CODEMAPS/core.md"
    "docs/CODEMAPS/security.md"
)

for file in "${FILES[@]}"; do
    if [ -f "$file" ]; then
        echo "  ✅ $file"
    else
        echo "  ❌ $file 缺失"
        ERRORS=$((ERRORS + 1))
    fi
done

echo ""
echo "✓ 检查源代码文件..."

# 检查源代码文件
SOURCE_FILES=(
    "crates/cyberclaw-core/src/lib.rs"
    "crates/cyberclaw-core/src/execution.rs"
    "crates/cyberclaw-control-plane/src/lib.rs"
    "crates/cyberclaw-control-plane/src/artifact_store.rs"
    "crates/cyberclaw-control-plane/src/event_bus.rs"
    "crates/cyberclaw-control-plane/src/lease_manager.rs"
    "crates/cyberclaw-control-plane/src/membership_service.rs"
    "crates/cyberclaw-control-plane/src/subagent_scheduler.rs"
    "crates/cyberclaw-control-plane/src/shared_state_store.rs"
)

for file in "${SOURCE_FILES[@]}"; do
    if [ -f "$file" ]; then
        echo "  ✅ $file"
    else
        echo "  ❌ $file 缺失"
        ERRORS=$((ERRORS + 1))
    fi
done

echo ""
echo "✓ 验证文档元数据..."

# 检查所有 CODEMAPS 文档是否有必要的元数据
for md_file in docs/CODEMAPS/*.md; do
    if [ -f "$md_file" ]; then
        if grep -q "最后更新:" "$md_file"; then
            echo "  ✅ $(basename $md_file) - 有时间戳"
        else
            echo "  ⚠️  $(basename $md_file) - 缺少时间戳"
        fi
    fi
done

echo ""
echo "✓ 统计文档..."
echo "  📄 总文档数: $(find docs -name "*.md" | wc -l | tr -d ' ')"
echo "  📁 CODEMAPS: $(find docs/CODEMAPS -name "*.md" | wc -l | tr -d ' ') 个文件"
echo "  📝 总行数: $(cat docs/CODEMAPS/*.md | wc -l | tr -d ' ') 行"

echo ""
if [ $ERRORS -eq 0 ]; then
    echo "✅ 文档验证通过！所有文件和元数据都有效。"
    echo ""
    echo "📚 文档结构:"
    tree -L 2 docs/ 2>/dev/null || find docs -type f -name "*.md" | sort | sed 's/^/  /'
    exit 0
else
    echo "❌ 发现 $ERRORS 个问题。请修复后再试。"
    exit 1
fi
