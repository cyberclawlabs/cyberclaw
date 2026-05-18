#!/usr/bin/env bash
# 下载 web/dist/ 下的第三方 vendor 资产（不进 git，因为 dist/ 在 .gitignore）。
# 任何首次 clone 仓库的人执行一次即可。
#
# 当前仅 Tailwind JIT runtime（替代 cdn.tailwindcss.com）。

set -euo pipefail

cd "$(dirname "$0")/.."
OUT=web/dist/tailwind.runtime.js

if [ -s "$OUT" ]; then
  echo "[skip] $OUT already exists ($(wc -c <"$OUT") bytes)"
  exit 0
fi

echo "[fetch] tailwind JIT runtime → $OUT"
mkdir -p web/dist
curl -sL --fail -o "$OUT" "https://cdn.tailwindcss.com?plugins=forms,typography"

# Strip the runtime "should not be used in production" console.warn — 我们已知非
# 最优、已经放进 backlog（PostCSS 静态编译）。当前 polish 阶段不希望该 warn
# 污染 console、误导开发者认为应用没装好。可后续随 build pipeline 一起治本。
PATTERN='console.warn("cdn.tailwindcss.com should not be used in production'
if grep -q "$PATTERN" "$OUT"; then
  perl -i -pe 's{console\.warn\("cdn\.tailwindcss\.com[^"]*"\);}{}g' "$OUT"
  echo "[patch] removed cdn.tailwindcss.com console.warn"
fi

echo "[done] $(wc -c <"$OUT") bytes"
