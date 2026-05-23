# API reference

The HTTP surface of `cyberclaw-server`. All endpoints are namespaced
under `/api/v1/`. Authentication is JWT bearer (operator JWT obtained
via `POST /admin/login`); webhook routes use platform-specific
HMAC-SHA256 signatures instead.

## Files in this directory

| File | What it is |
|---|---|
| [`ROUTES.md`](ROUTES.md) | Per-route reference: method, path, body shape, response shape, status codes, error variants |
| [`openapi.yaml`](openapi.yaml) | Machine-readable OpenAPI 3.1 specification |
| [`AUTOPILOT_V2_API.md`](AUTOPILOT_V2_API.md) | Autopilot subsystem endpoints |
| [`USER_MANUAL.md`](USER_MANUAL.md) | End-to-end usage walkthrough |

## Where else to look

- [`docs/reference/api.md`](../reference/api.md) — quick lookup table
- [`docs/configuration/README.md`](../configuration/README.md) — env vars that change API behavior
- [`schemas/v2/`](../../schemas/v2/) — JSON Schemas for request/response payloads
