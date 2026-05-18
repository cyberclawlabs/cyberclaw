#!/usr/bin/env python3
"""CyberClaw load test — concurrent request driver against critical endpoints.

Usage:
    python3 scripts/load-test.py --base http://127.0.0.1:38090 \\
            --duration 30 --concurrency 10 --endpoints health,memory,chat

Reports per-endpoint p50/p95/p99 latency + error rate. Designed to
produce reproducible numbers for SLO baselining (deploy/monitoring/slo.md).

Endpoints:
  - health  — GET /health, no auth, scrape-class
  - memory  — POST /api/v1/memory + GET /api/v1/memory (round-trip)
  - chat    — POST /v1/agent/chat/completions (real LLM call, slow)
  - todo    — POST /api/v1/agents/<id>/invoke + LLM-driven todo_write
"""
import argparse
import concurrent.futures
import json
import statistics
import sys
import time
import urllib.error
import urllib.request


def login(base, user_id="qa-admin"):
    req = urllib.request.Request(
        f"{base}/admin/login",
        data=json.dumps({"user_id": user_id, "password": "any"}).encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read())["jwt"]


def hit(method, url, jwt=None, body=None, timeout=120):
    """Single request. Returns (status_code, elapsed_ms, error_or_None)."""
    headers = {"Content-Type": "application/json"}
    if jwt:
        headers["Authorization"] = f"Bearer {jwt}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            r.read()
            return r.status, (time.perf_counter() - t0) * 1000, None
    except urllib.error.HTTPError as e:
        return e.code, (time.perf_counter() - t0) * 1000, f"HTTP {e.code}"
    except Exception as e:
        return 0, (time.perf_counter() - t0) * 1000, str(e)


def driver_health(base, jwt):
    return hit("GET", f"{base}/health", timeout=5)


def driver_memory(base, jwt):
    body = {
        "agent_id": "load-test-agent",
        "level": "L1",
        "content": "load test record",
    }
    return hit("POST", f"{base}/api/v1/memory", jwt=jwt, body=body, timeout=10)


def driver_chat(base, jwt):
    body = {
        "messages": [{"role": "user", "content": "Reply with the word OK only."}],
        "model": "MiniMax-M2.7-HighSpeed",
        "stream": False,
        "max_iterations": 1,
    }
    return hit("POST", f"{base}/v1/agent/chat/completions", jwt=jwt, body=body, timeout=180)


DRIVERS = {
    "health": driver_health,
    "memory": driver_memory,
    "chat": driver_chat,
}


def run_endpoint(name, driver, base, jwt, duration, concurrency):
    print(f"\n=== {name} (concurrency={concurrency}, duration={duration}s) ===")
    deadline = time.time() + duration
    results = []  # (status, elapsed_ms, err)

    def worker():
        local = []
        while time.time() < deadline:
            local.append(driver(base, jwt))
        return local

    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
        futures = [pool.submit(worker) for _ in range(concurrency)]
        for f in concurrent.futures.as_completed(futures):
            results.extend(f.result())

    if not results:
        print("  no responses")
        return

    successes = [(s, lat) for s, lat, e in results if e is None and 200 <= s < 300]
    fails = [(s, lat, e) for s, lat, e in results if not (e is None and 200 <= s < 300)]
    latencies = [lat for _, lat in successes]

    n = len(results)
    n_ok = len(successes)
    n_429 = sum(1 for s, _, _ in fails if s == 429)
    n_other = len(fails) - n_429
    rps = n / duration

    print(f"  total: {n}  ok: {n_ok}  429: {n_429}  other_err: {n_other}  rps: {rps:.1f}")
    if latencies:
        latencies.sort()
        p50 = statistics.median(latencies)
        p95 = latencies[int(len(latencies) * 0.95)]
        p99 = latencies[int(len(latencies) * 0.99)]
        mx = max(latencies)
        print(f"  latency_ms: p50={p50:.0f} p95={p95:.0f} p99={p99:.0f} max={mx:.0f}")
    if n_other and n_other <= 5:
        # surface a sample of unexpected errors
        for s, lat, e in fails[:3]:
            print(f"  sample_err: status={s} elapsed_ms={lat:.0f} msg={e}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:38090",
                    help="server base URL (default: %(default)s)")
    ap.add_argument("--user", default="qa-admin",
                    help="login user_id (default: %(default)s)")
    ap.add_argument("--duration", type=int, default=15,
                    help="seconds per endpoint (default: %(default)s)")
    ap.add_argument("--concurrency", type=int, default=8,
                    help="parallel workers per endpoint (default: %(default)s)")
    ap.add_argument("--endpoints", default="health,memory,chat",
                    help="comma-separated subset of: " + ",".join(DRIVERS))
    args = ap.parse_args()

    print(f"Load-testing {args.base} as {args.user}")
    jwt = login(args.base, args.user)
    print(f"JWT acquired, len={len(jwt)}")

    for name in args.endpoints.split(","):
        name = name.strip()
        if name not in DRIVERS:
            print(f"!! unknown endpoint '{name}', skip", file=sys.stderr)
            continue
        run_endpoint(name, DRIVERS[name], args.base, jwt, args.duration, args.concurrency)


if __name__ == "__main__":
    main()
