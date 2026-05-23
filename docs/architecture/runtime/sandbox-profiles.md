# SandboxProfile — Unified Container Sandbox Abstraction

> **Module**: `crates/cyberclaw-connectors/src/sandbox/profile.rs`
> **Sprint**: v1.x optimization R3 (2026-05-21)
> **Status**: Implemented; first wired through `cmd.run` / `cmd.exec` / `cmd.run_streaming` / `cmd.run_powershell`

`SandboxProfile` is the single source of truth for how a `Capability` runs inside a container. It replaces two formerly independent mechanisms (the `ContainerRuntime` default tmpfs + the per-capability inline `ContainerConfig` blocks in `cmd.rs`) that wrote conflicting mount directives to the same container path.

---

## 1. Why this exists

The v1 business test matrix (`docs/implementation/release/business-test-report-v1-2026-05-21.md`) recorded the C-class shell-correctness category at **50-67% on CyberClaw vs 100% on Hermes**. The server log showed two reproducible failures every time a `cmd.run` was dispatched:

```
docker: Error response from daemon: Duplicate mount point: /tmp
rg unavailable or failed (No such file or directory): falling back to pure-Rust grep
```

Root cause: two unrelated code paths both declared a `/tmp` mount:

1. `crates/cyberclaw-connectors/src/runtime/container.rs` — when `read_only_root: true`, the runtime added `--tmpfs /tmp:rw,noexec,nosuid,size=100m` unconditionally.
2. `crates/cyberclaw-connectors/src/local/cmd.rs::build_shared_container_volumes()` — for every `cmd.run` / `cmd.exec` dispatch, the helper added `-v /tmp/cyberclaw-shared:/tmp:rw`.

Docker rejected the second `/tmp`. Compounding the failure, the base image (`python:3.12-slim`) shipped without ripgrep, jq, or git — so even when a `cmd.run` succeeded, `search.grep` (a separate capability that shells out to `rg`) degraded to its pure-Rust fallback and lost feature parity (e.g. `--type=` filtering).

---

## 2. Design

### Object boundary

`SandboxProfile` is a **Connector-layer detail**. It does **not** introduce a sixth ecosystem object: agents and skills remain unaware of profiles. The five-object discipline holds:

| Object              | Role                              | Sees profile? |
|---------------------|-----------------------------------|---------------|
| Agent               | Who runs the task                 | No            |
| Skill               | How to approach the task          | No            |
| Connector           | Which adapter to dispatch through | **Yes — resolves a profile** |
| Capability          | Smallest action unit              | Declares desired profile id  |
| Platform Plugin     | Platform-level augmentation       | No            |

### Type sketch

```rust
pub struct SandboxProfile {
    pub id: SandboxProfileId,
    pub mounts: Vec<MountSpec>,
    pub tools_required: Vec<&'static str>,
    pub network: NetworkPolicy,
    pub time_budget: Duration,
    pub default_image: &'static str,
    pub mount_workspace: bool,
    pub read_only_root: bool,
    pub memory_limit_mb: Option<u64>,
    pub cpu_limit: Option<f64>,
}

impl SandboxProfile {
    pub fn validate_and_resolve(&self, cap: &CapabilityContract) -> Result<EffectiveSandbox>;
}
```

`validate_and_resolve()` returns an `EffectiveSandbox` view that is **deduplicated** and **conflict-free**. Crucially it carries a `needs_implicit_tmp_tmpfs: bool` that tells the runtime whether to add its automatic `--tmpfs /tmp` — when the profile already owns `/tmp`, this is `false`, and the duplicate-mount class of error is structurally impossible.

### Three builtin profiles

| id          | mounts                                | tools_required                                 | network |
|-------------|----------------------------------------|------------------------------------------------|---------|
| `minimal`   | none (runtime adds tmpfs `/tmp`)      | bash, coreutils                                | None    |
| `dev`       | host `/tmp/cyberclaw-shared` → `/tmp` | bash, coreutils, ripgrep, jq, python3          | Bridge  |
| `isolated`  | host `/tmp/cyberclaw-shared` → `/tmp` | bash, coreutils, ripgrep, jq, python3          | None    |

Operator overrides:

- `CYBERCLAW_CONTAINER_IMAGE` — replaces `default_image`
- `CYBERCLAW_CONTAINER_SHARED_TMP` — replaces the host `/tmp/cyberclaw-shared` source path used by `dev` / `isolated`

---

## 3. Capability mapping

| Capability             | Profile     | Rationale                                                              |
|------------------------|-------------|------------------------------------------------------------------------|
| `cmd.exec` (whitelist) | `isolated`  | Historically `NetworkMode::None`; whitelist excludes anything needing net |
| `cmd.run` (bash facade)| `dev`       | Agent-facing shell — needs ripgrep / jq / python3 and bridge net      |
| `cmd.run_streaming`    | `dev`       | Same surface as `cmd.run`; buffered output inside one-shot `docker run`|
| `cmd.run_powershell`   | `minimal` (advisory) | pwsh not yet packaged in the sandbox image — falls back to native shell, profile is consulted only for the time budget contract |

`cmd.run_powershell` is currently the one outlier: the `python:3.12-slim` base does not ship pwsh, so its dispatch stays native. A future profile addition (`powershell` id, `mcr.microsoft.com/dotnet/runtime` base) would let it land in a container too.

---

## 4. Image baking

The image referenced by `SandboxProfile::dev().default_image` (and by the `isolated` profile) is built from the new `Dockerfile.sandbox` at the repo root:

```bash
docker build -t cyberclaw/sandbox:latest -f Dockerfile.sandbox .
export CYBERCLAW_CONTAINER_IMAGE=cyberclaw/sandbox:latest
```

The image bakes:

- `bash`, `coreutils` — POSIX shell + standard utilities
- `ripgrep` — `rg` for `search.grep` (resolves the "rg unavailable" log line)
- `jq` — JSON filter for shell pipelines
- `python3` (already in the base)
- `ca-certificates`, `curl`, `git` — typical agent-emitted tool surface

The Dockerfile's `RUN bash --version && rg --version && jq --version && …` line is intentional: it fails the build loudly if any tool is missing so the image promise stays honest.

---

## 5. Governance visibility

`SandboxProfile` is **declarable in governance.yaml** so operators can see what surface area each capability has been given. Example (informational; the resolver currently consults the profile ids set inside `cmd.rs`, not the YAML — full YAML-driven profile selection is a follow-up):

```yaml
# governance.yaml — sandbox profile section (R3 advisory)
sandbox_profiles:
  - id: dev
    description: "Default agent bash facade — bridge net + ripgrep/jq/python"
    bound_capabilities:
      - cmd.run
      - cmd.run_streaming
  - id: isolated
    description: "No-network shell for whitelisted cmd.exec"
    bound_capabilities:
      - cmd.exec
  - id: minimal
    description: "Empty mounts, no network — used for offline numeric verifiers"
```

---

## 6. Failure modes after R3

1. **Duplicate mount on `/tmp`** — structurally impossible. The runtime branch that emits `--tmpfs /tmp` reads `skip_implicit_tmp_tmpfs` from the `ContainerConfig`, which is set from the profile's `needs_implicit_tmp_tmpfs`. Any profile that owns `/tmp` (both `dev` and `isolated`) suppresses the runtime tmpfs.

2. **Missing `rg` / `jq`** — operator boots the server with `CYBERCLAW_CONTAINER_IMAGE` pointing at an image that wasn't built from `Dockerfile.sandbox`. Mitigation: the profile logs `tools_required` at debug level when resolving; the Dockerfile fails its own build if a tool is absent. Future hardening: a startup probe that runs `rg --version` inside the configured image and surfaces a server-startup warning when missing.

3. **Profile id collision** — two profile mounts target the same container path with different bindings. `validate_and_resolve()` returns `Err` with `conflicting mounts` so the bad profile cannot reach the runtime.

---

## 7. Verification

The R3 acceptance suite expects:

- `cargo test -p cyberclaw-connectors --lib` — all green, including the new `sandbox::profile::tests` suite (10 tests).
- `grep -i "duplicate mount" <server-log>` — empty for the full duration of a `cmd.run`-heavy session.
- Business matrix C class re-run — ≥ 95% correctness (was 50-67% pre-R3).

---

## 8. Follow-ups (out of R3 scope)

- Wire all other connectors (`browser`, `mcp`, `cluster`, `retrieval`) through the profile abstraction. Today only `local/cmd.rs` consumes it.
- YAML-driven profile resolution: read `sandbox_profiles:` block from governance.yaml and let capability manifests declare `desired_profile: <id>` rather than hardcoding the choice inside the connector.
- pwsh-bearing sandbox image so `cmd.run_powershell` joins the container path.
- `streaming` container dispatch via `docker exec` log tailing (today the container path on `cmd.run_streaming` synthesises lines from the buffered output, which is correct but loses real-time semantics).
