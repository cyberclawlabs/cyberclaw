# Security Policy

CyberClaw is built around governable execution, controlled capability access, and auditability. If you discover a security issue, do not disclose it publicly first.

## Contact

Report security issues to:

- `info@cyberclawlabs.ai`

If possible, use a subject line like:

`[CyberClaw Security] <short summary>`

## What To Include

Please include:

1. affected component or path
2. impact and severity estimate
3. reproduction steps
4. environment details
5. proof of concept if available
6. suggested remediation if you have one

## Responsible Disclosure

Please do not:

- open a public GitHub issue first
- publish exploit details before coordination
- assume an architectural design document equals a shipped security guarantee

## What To Expect

After receiving a report, maintainers aim to:

1. acknowledge receipt
2. assess severity
3. reproduce the issue
4. coordinate a fix and disclosure path

Response time depends on severity, complexity, and maintainer availability.

## Scope Notes

CyberClaw contains both:

- current implementation
- forward-looking architecture and design materials

When reporting an issue, please distinguish whether the concern affects:

1. current shipped code paths
2. documentation or public claims
3. design proposals not yet implemented

## Security-Related Documentation

For additional context, see the [Security architecture section](docs/ARCHITECTURE.md#security-architecture) of the architecture document.

## Sandbox Architecture (v1.0+)

CyberClaw v1.0 ships with **container-based OS-level sandbox isolation** as the default execution path for `cmd.run`. Defense-in-depth layers:

1. **Layer 1 — Denylist (fast path)**: `check_cmd_run_safety` blocks 12 sensitive path patterns (/etc/passwd, /etc/shadow, .ssh/id_*, /root/, /proc/self/environ, etc.).
2. **Layer 2 — Container isolation (true boundary)**: `ContainerRuntime` dispatches shell into `python:3.12-slim` with `NetworkMode::None`, `read_only_root`, `auto_remove`, 512MB memory cap. Even if the denylist is bypassed via shell expansion, the LLM reads container fake data — host data is unreachable.

**Verified evidence**: `whoami` returns `nobody`, `/etc/passwd` contains Linux `root:/bin/bash` shell entries (different from host macOS `/bin/sh`), confirming the OS-level boundary.

**Pre-existing rc12 denylist remains** as a fast-path defense and audit signal, but is no longer the sole boundary.

## Supported Surface

CyberClaw is under active development. Security support is best-effort and follows the current main development line in this repository unless stated otherwise in release materials.
