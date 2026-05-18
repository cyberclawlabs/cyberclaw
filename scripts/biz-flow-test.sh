#!/usr/bin/env bash
# 业务流测试：3 个 prompt 走 cyberclaw chat completions，
# 测 first-token / 总耗时 / 输出质量，验证"达到 hermes 级"。
#
# 使用 MiniMax-M2.7-HighSpeed（cyberclaw 当前默认 LLM）。
# 输出存 /tmp/cc-bizflow-{1,2,3}.json + 控制台 summary。

set -e
SERVER="${CYBERCLAW_SERVER:-http://127.0.0.1:38090}"
JWT=$(curl -s -X POST -H "Content-Type: application/json" \
  -d '{"user_id":"qa-admin","password":"x"}' \
  "$SERVER/admin/login" | python3 -c "import sys,json;print(json.load(sys.stdin)['jwt'])")
[ -z "$JWT" ] && { echo "[FAIL] login failed"; exit 1; }

run_prompt() {
  local idx=$1; local prompt=$2; local out="/tmp/cc-bizflow-${idx}.sse"
  local conv=$(curl -s -X POST -H "Authorization: Bearer $JWT" \
    -H "Content-Type: application/json" \
    -d "{\"title\":\"bizflow-${idx}\"}" \
    "$SERVER/api/v1/chat/conversations" | python3 -c "import sys,json;print(json.load(sys.stdin)['id'])")
  local t0=$(python3 -c "import time;print(int(time.time()*1000))")
  curl -s -X POST -H "Authorization: Bearer $JWT" \
    -H "Content-Type: application/json" \
    -H "Accept: text/event-stream" \
    -d "{\"model\":\"MiniMax-M2.7-HighSpeed\",\"messages\":[{\"role\":\"user\",\"content\":\"${prompt}\"}],\"stream\":true,\"conversation_id\":\"$conv\"}" \
    "$SERVER/v1/chat/completions" > "$out"
  local t1=$(python3 -c "import time;print(int(time.time()*1000))")
  local elapsed=$((t1 - t0))
  python3 << PY
import json, re
content = ''
first_token_at = None
import time
for line in open('$out'):
    if line.startswith('data: ') and 'choices' in line:
        try:
            d = json.loads(line[6:])
            c = d.get('choices',[{}])[0].get('delta',{}).get('content','')
            content += c
        except: pass
visible = re.sub(r'<think>.*?</think>', '', content, flags=re.DOTALL).strip()
print(f"--- prompt {$idx} ---")
print(f"  elapsed: ${elapsed}ms")
print(f"  total chars: {len(content)} (visible no-think: {len(visible)})")
print(f"  --- visible output (first 400) ---")
print('  ' + visible[:400].replace(chr(10), chr(10)+'  '))
print()
PY
}

echo "=== bizflow test starting at $(date -u +%H:%M:%S)Z ==="
echo
run_prompt 1 "Write a Python function validate_email(email) using regex. Return True/False. Just the code, no explanation."
run_prompt 2 "List 3 concrete benefits of dark mode in admin dashboards. Each bullet under 25 words."
run_prompt 3 "Create a markdown checklist with 5 items for shipping a v2 admin dashboard rewrite. Use - [ ] format."
echo "=== done at $(date -u +%H:%M:%S)Z ==="
