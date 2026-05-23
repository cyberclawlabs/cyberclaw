---
marp: true
theme: default
paginate: true
---

# `slides.render` Connector — Design Proposal
**CyberClaw Feature Design · Sprint ~12 · 2026-05-03**

---

## Problem Statement

CyberClaw agents produce rich Markdown artifacts daily, but sharing them requires external tools (Marp CLI, online converters). Users need to render Marp Markdown → PPTX **inside the container** — without leaving the CyberClaw runtime.

> **Goal:** A native `slides.render` Connector that turns Marp markdown into a `.pptx` Artifact, fully traceable in the CyberClaw control plane.

---

## Requirements & Constraints

**Must satisfy:**
- Marp markdown → valid PPTX with correct slide structure
- Native Connector → Capability → Artifact pipeline integration
- Executes inside the container (no external service calls)
- Output is a CyberClaw Artifact (bytes + MIME type + trace ID)
- Standard Marp themes (built-in) supported

**Hard constraints:**
- No subprocess spawning for rendering (governance requirement)
- No Node.js / external runtime dependency
- Must wire into `cyberclaw-connectors` and `cyberclaw-capabilities`

---

## Design Options Considered

| # | Option | Approach | Key Tradeoff |
|---|--------|----------|--------------|
| **A** | CLI Wrapper | `@marp-team/marp-cli` subprocess | ❌ Node.js dep, subprocess governance risk |
| **B** ✅ | **Pure Rust** | `pulldown-cmark` + Marp parser + `pptx` crate | **Winner: governance-native, no ext deps** |
| **C** | WASM Hybrid | `@marp-team/marp-core` via `wasmtime` | WASM complexity, larger image |
| **D** | HTTP Sidecar | Local HTTP server + Rust Connector | Sidecar lifecycle mgmt, extra runtime |

**Weighted scoring** (governance 20%, deps 20%, sandbox 20%, fidelity 25%, maintainability 15%):
→ **Option B wins at 4.55 / 5.00**

---

## Architecture — Option B

```
Agent / Skill
     │
     ▼
SlidesRenderTask  (markdown: String, theme: Option<String>)
     │
     ▼
slides.render Connector  ──governance──▶  MarpDirectiveParser
     │                                         │
     │                                   MarpDeck { slides, frontmatter }
     │                                         │
     │                                   MarpToPptxMapper
     │                                         │
     │                                   pptx::Presentation
     │                                         │
     ▼                                         ▼
Artifact  ◀──────────────────────────────  pptx::Presentation.save()
(bytes + MIME + trace_id)
```

**Key files:**
- `crates/cyberclaw-connectors/src/slides/render/marp_parser.rs` — Marp directive parsing
- `crates/cyberclaw-connectors/src/slides/render/mapper.rs` — Marp AST → PPTX mapping
- `crates/cyberclaw-connectors/src/slides/render/connector.rs` — Connector trait impl
- `crates/cyberclaw-capabilities/src/slides.rs` — Capability registration

---

## Marp-to-PPTX Mapping

| Marp Element | PPTX Output |
|-------------|------------|
| `---` (slide break) | `slide.add_slide()` |
| `# H1` / `## H2` | `PlaceholderFormat::Title / Outline` |
| Paragraph text | `TextFrame` + `Paragraph` + `Run` |
| Code block `` ``` `` | Text box, monospace font (`Fira Code` fallback) |
| `![alt](src)` | `pptx::media::Image` (base64 inline or URL fetch) |
| Markdown table | `pptx::table::Table` |
| `<!-- class: something -->` | Slide class → `FillFormat` / `SlideTransition` |
| `theme:` frontmatter | `ThemeColorScheme` via `parse_theme_color_scheme` |

> **Known gap:** Full Marp CSS themes require CSS → DML mapping. MVP targets background color + font family only.

---

## Implementation Roadmap

| Phase | Task | File(s) | Risk |
|-------|------|---------|------|
| **1 — Spike** | Marp directive parser (frontmatter, `---` breaks, HTML comments) | `marp_parser.rs` | **HIGH** — core unknown |
| **2 — Core** | MarpDeck → pptx::Presentation mapper | `mapper.rs` | MEDIUM |
| **3 — Connector** | `Connector::execute()` + governance | `connector.rs`, `mod.rs` | LOW |
| **4 — Capability** | SlidesRenderCapability registration | `slides.rs` | LOW |
| **5 — Tests** | Unit tests + LibreOffice validation | `tests/slides_render.rs` | LOW |
| **6 — Artifact** | Trace writing to `cyberclaw-store` | control plane | LOW |

**Estimated total:** ~3–5 days for working MVP (spike is the critical path)

---

## Risks, Mitigations & Verification

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `pptx` crate API instability (v0.1.0, solo author) | Medium | Pin version + integration tests |
| Marp CSS themes partially unsupported | Medium | MVP: bg color + font family only |
| Large deck OOM | Low | Govern input size + slide count cap |
| Image URL fetch in container | Medium | Inline base64 MVP; URL fetch follow-up |
| Speaker notes not in pptx crate | Medium | Upstream issue + document as gap |

**Verification:**
- `cargo test -p cyberclaw-connectors slides` → all green
- Generated PPTX opens in LibreOffice headless (no errors)
- `PptxValidator` from pptx crate confirms valid structure
- End-to-end: Connector task → Artifact in `cyberclaw-store`

---

## Acceptance Criteria

- [ ] Marp with H1, H2, paragraphs, code, images, tables → valid PPTX
- [ ] PPTX opens in LibreOffice without errors
- [ ] `slides.render` Connector registered and callable via task pipeline
- [ ] Artifact trace: `bytes` + `mime: application/vnd.openxmlformats-officedocument.presentationml.presentation` + `trace_id`
- [ ] Unit test coverage ≥ 90% on `marp_parser` + `mapper`
- [ ] Governance: input size limit + timeout enforced
- [ ] Plan persisted to `cyberclaw-store` Artifact

---

## Appendix: ADR — Why Pure Rust?

**Decision:** Use Pure Rust (Option B) — `pulldown-cmark` + custom Marp directive parser + `pptx` crate.

**Drivers:**
1. **Governance & sandbox** — in-process rendering with full CyberClaw governance visibility
2. **Container purity** — no Node.js, no sidecar, no WASM runtime added to image
3. **Rendering fidelity** — `pptx` crate v0.1.0 covers slides, text, tables, charts, media, transitions, themes, OPC

**Alternatives rejected:**
- **A (CLI):** Subprocess escape hatch undermines CyberClaw's governance model
- **C (WASM):** `wasmtime` adds complexity; Marp WASM ecosystem not yet stable
- **D (Sidecar):** Operational overhead, HTTP IPC latency, lifecycle management burden

**Consequences:**
- Marp CSS → DML mapping is partial (MVP: colors + fonts only)
- `pptx` crate v0.1.0 stability risk — monitor releases closely
- Speaker notes support is a known gap (upstream issue filed)

**Follow-ups:**
- Spike `pptx` crate speaker notes support; file upstream if missing
- Add URL-based image fetching as follow-up feature
- Evaluate WASM path if `marp-team/marp-core` stabilizes its WASM build
