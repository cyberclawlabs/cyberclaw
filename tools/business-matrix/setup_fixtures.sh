#!/usr/bin/env bash
# Build the /Users/max/project/cyberclaw/.matrix-fixtures/ test fixture tree referenced by prompts.yml.
# Idempotent: nukes and recreates every time so each matrix run starts clean.

set -e
ROOT=/Users/max/project/cyberclaw/.matrix-fixtures

echo "=== rebuilding fixture tree at $ROOT ==="
rm -rf "$ROOT"
mkdir -p "$ROOT"/{listdir-target,backups,docs,configs,logs,src,tests,context-shared}

# A — file_io
echo "hello world" > "$ROOT/hello.txt"
touch "$ROOT/exists-yes.txt"
touch "$ROOT/listdir-target/a.txt" "$ROOT/listdir-target/b.txt"
# touch-test.txt expected to be created by agent
# A-L1-5: 32-byte file (exactly)
printf 'x%.0s' {1..32} > "$ROOT/sized-32bytes.txt"
cat > "$ROOT/csv-3lines.csv" <<'EOF'
name,value
alice,100
bob,200
EOF
echo "apple banana date" > "$ROOT/edit-target.txt"
cat > "$ROOT/notes.txt" <<'EOF'
# heading one
intro line
# heading two
body line a
# heading three
body line b
EOF
cat > "$ROOT/log.txt" <<'EOF'
line one
line two
line three
line four
line five
EOF
echo "same content here" > "$ROOT/file-a.txt"
echo "same content HERE" > "$ROOT/file-b.txt"   # one word differs
cat > "$ROOT/docs/intro.md" <<'EOF'
# Intro
This document introduces the platform.
EOF
cat > "$ROOT/docs/setup.md" <<'EOF'
# Setup
Install dependencies and run the bootstrap script.
EOF
cat > "$ROOT/docs/deploy.md" <<'EOF'
# Deploy
Deploy via the CI/CD pipeline after passing all checks.
EOF
cat > "$ROOT/configs/prod.json" <<'EOF'
{"name":"prod","enabled":true,"region":"us-east-1"}
EOF
cat > "$ROOT/configs/active.json" <<'EOF'
{"name":"active","enabled":true,"region":"eu-west-1"}
EOF
cat > "$ROOT/configs/legacy.json" <<'EOF'
{"name":"legacy","enabled":false,"region":"us-west-2"}
EOF
# 42-word report stub
cat > "$ROOT/report-stub.md" <<'EOF'
This report stub contains exactly forty two words and is used by the cyberclaw business
matrix test to validate that read write and count operations end to end work without
any tool error or governance interrupt happening in the middle here.
EOF
echo "foo bar baz foo bar baz" > "$ROOT/multiedit.txt"
cat > "$ROOT/data.csv" <<'EOF'
group,value
alpha,30
alpha,20
alpha,10
beta,40
beta,50
EOF

# B — search
cat > "$ROOT/logs/app.log" <<'EOF'
2026-05-19T10:00:00 INFO app started
2026-05-19T10:00:01 WARN slow query detected E1001
2026-05-19T10:00:02 ERROR cache miss E2042
2026-05-19T10:00:03 INFO retry succeeded
2026-05-19T10:00:04 CRITICAL out of disk
2026-05-19T10:00:05 ERROR db timeout E1001
2026-05-19T10:00:06 WARN deprecated api E3007
EOF
cat > "$ROOT/src.py" <<'EOF'
# TODO: implement caching
def main():
    return 0

# TODO: add tests
EOF
cat > "$ROOT/events.txt" <<'EOF'
2026-05-19 normal event
2026-05-19 ERROR happened
2026-05-19 another normal
2026-05-19 Error in module
2026-05-19 error detected
2026-05-19 ERROR cascade
EOF
mkdir -p "$ROOT/src"
cat > "$ROOT/src/checkout.py" <<'EOF'
import json
def handle_payment(amount):
    return {"status": "ok"}

import logging
EOF
cat > "$ROOT/src/util.py" <<'EOF'
import os
import sys
def parse_args():
    pass
def load_config():
    pass
def save_config():
    pass
def main():
    pass
EOF
cat > "$ROOT/src/auth.py" <<'EOF'
import jwt
import os
STRIPE_SECRET = os.getenv("STRIPE_SECRET")
def verify(token):
    return jwt.decode(token, "x")
EOF
cat > "$ROOT/src/billing.py" <<'EOF'
import stripe
stripe.api_key = os.environ.get("STRIPE_SECRET")
def charge(amount):
    return stripe.Charge.create(amount=amount)
EOF
cat > "$ROOT/data.yaml" <<'EOF'
name: example
EOF
cat > "$ROOT/extra.yml" <<'EOF'
mode: test
EOF
touch "$ROOT/README" "$ROOT/README.md"
# tests dir
cat > "$ROOT/tests/test_util.py" <<'EOF'
def test_a(): assert 1 == 1
def test_b(): assert 2 == 2
def test_c(): assert 3 == 3
EOF
cat > "$ROOT/tests/test_auth.py" <<'EOF'
def test_a(): assert True
def test_b(): assert 2 > 1
def test_c(): assert 1 < 2
def test_d(): assert 1 != 2
def test_e(): assert "x" in "xy"
EOF
cat > "$ROOT/tests/test_payment.py" <<'EOF'
def test_1(): assert 1
def test_2(): assert 2
def test_3(): assert 3
def test_4(): assert 4
def test_5(): assert 5
def test_6(): assert 6
def test_7(): assert 7
def test_8(): assert 8
EOF
cat > "$ROOT/configs/staging.json" <<'EOF'
{"name":"staging","enabled":false,"region":"us-east-2","tier":"medium","quota":1000}
EOF
cat > "$ROOT/configs/dev.json" <<'EOF'
{"name":"dev","enabled":false,"region":"local"}
EOF
# overwrite prod.json with 7 keys for B-L3-5
cat > "$ROOT/configs/prod.json" <<'EOF'
{"name":"prod","enabled":true,"region":"us-east-1","tier":"high","quota":100000,"timeout_ms":30000,"audit_enabled":true}
EOF

# E — multi-step
echo "this file should persist as is" > "$ROOT/important.txt"
cat > "$ROOT/numbers.txt" <<'EOF'
10
20
30
40
50
EOF
cat > "$ROOT/checklist.md" <<'EOF'
- [x] item one
- [ ] item two
- [x] item three
- [ ] item four
- [ ] item five
EOF
cat > "$ROOT/v1.txt" <<'EOF'
line a
line b
line c
EOF
cat > "$ROOT/v2.txt" <<'EOF'
line a
line c
line d
EOF
cat > "$ROOT/inventory.json" <<'EOF'
[
  {"sku":"A1","qty":10,"price":2.50},
  {"sku":"B2","qty":5,"price":7.00},
  {"sku":"C3","qty":20,"price":1.25},
  {"sku":"D4","qty":3,"price":15.00}
]
EOF

# H — context-shared
cat > "$ROOT/context-1.txt" <<'EOF'
project=cyberclaw
sprint=v1.2.16
status=GA prep
EOF
cat > "$ROOT/context-shared/sec.md" <<'EOF'
# Security Policy
version: v2.3
EOF
cat > "$ROOT/context-shared/changelog.md" <<'EOF'
# Changelog
v2.3 shipped 2026-04-12
v2.2 shipped 2026-02-08
EOF

# G fixtures (G has no fs deps, pure reasoning)
# I fixtures (I uses external httpbin.org; no local setup)
# J fixtures (J is reasoning + occasional connector mock; no local setup)

echo "=== fixture build done ==="
find "$ROOT" -type f | wc -l | awk '{print $1" files created"}'
