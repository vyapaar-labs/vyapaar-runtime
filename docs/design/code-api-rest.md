# `vyapaar-runtime` — Axum REST & WebSocket Gateway Reference

Documents `crates/vyapaar-runtime-api-rest/src/`: production OpenAPI 3.1 REST
endpoints and zero-polling `/ws/live` streaming WebSocket gateway.

## REST Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/health` | GET | System health and diagnostics |
| `/version` | GET | API version |
| `/orders` | GET/POST | Query and place orders |
| `/orders/{id}` | DELETE | Cancel order |
| `/positions` | GET | Query open positions |
| `/positions/close` | POST | Emergency close-all |
| `/portfolio` | GET | Balance, equity, margin, FIFO history |
| `/risk` | GET | OTR counters, circuit breaker status |
| `/risk/kill-switch` | POST | Emergency trading halt |
| `/market-data/quote/{id}` | GET | Latest quote and price band |
| `/market-data/depth/{id}` | GET | L2/L3 order book depth |
| `/screener/scan` | POST | Custom AST-filtered stock scans |
| `/runs` | GET/POST | Backtest runs management |
| `/journal` | GET | Audit journal queries |

## WebSocket

| Endpoint | Purpose |
|----------|---------|
| `/ws/live` | Real-time bidirectional streaming: order updates, fills, positions, market depth |

## Key Modules

| Module | Purpose |
|--------|---------|
| `v1.rs`, `v2.rs` | API version routing |
| `openapi.rs` | OpenAPI 3.1 spec generation |
| `ws.rs` | WebSocket broadcaster |
| `service.rs` | Application service layer |
| `registry.rs` | Service registry |
| `journal.rs` | Audit journal endpoint |
| `run_store.rs` | Backtest run store |
| `streams.rs` | Stream management |
| `dispatch.rs` | Order dispatch |
| `exec.rs` | Execution gateway integration |
| `approve.rs` / `operator_journal.rs` | Approvals queue and operator audit |

## Related

- `vyapaar-runtime-api` — High-level API service traits and DTOs
- `vyapaar-exec` — Execution gateway for order placement
- `vyapaar-risk` — Pre-trade risk gate for order validation
