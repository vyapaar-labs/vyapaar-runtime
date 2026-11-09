# `vyapaar-runtime` — Persistence Reference

Documents `crates/vyapaar-runtime-persistence/src/`: SQLite and PostgreSQL
(`sqlx`) implementations of `vyapaar-ledger` repository traits.

## Modules

| Module | Purpose |
|--------|---------|
| `lib.rs` | Persistence module entry point |
| `migrations.rs` | Database migration definitions |
| `domain_stores.rs` | Domain event stores |
| `orders.rs` | `OrderRepository` implementation |
| `positions.rs` | `PositionRepository` implementation |
| `fills.rs` | Fill event storage |
| `sessions.rs` | Session management |
| `audit.rs` | Audit log storage |
| `decisions.rs` | Strategy decision storage |
| `traders.rs` | Trader state storage |
| `messages.rs` | Message store |

## Repository Trait Implementations

| Trait | SQLite | PostgreSQL |
|-------|--------|------------|
| `OrderRepository` | ✅ | ✅ |
| `AccountRepository` | ✅ | ✅ |
| `PositionRepository` | ✅ | ✅ |
| `LedgerRepository` | ✅ | ✅ |

## Migrations

Database schema versioned via migrations in `migrations/` directory. Applied at
startup via `sqlx` migration runner.

## Related

- `vyapaar-ledger/src/repository.rs` — Repository trait definitions (`OrderRepository`, `AccountRepository`, `PositionRepository`, `LedgerRepository`)
- `vyapaar-ledger-journal` — `JournalTransaction` type
- `vyapaar-ledger-portfolio` — `PositionRecord`, `TaxLot`
