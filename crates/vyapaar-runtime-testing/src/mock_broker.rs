//! Mock broker executor — consolidated onto `vyapaar_exec_core`.
//!
//! The execution-gateway crate owns the canonical thread-safe mock
//! (`vyapaar_exec_core::MockBrokerExecutor`) with auto-fill, rejection mode
//! (`reject_all` / `with_reject_all`), and submission recording
//! (`placed_orders`). This module only re-exports it so the runtime never
//! forks a second `MockBrokerExecutor` (AGENTS §4.1 consolidation-over-
//! duplication).
//!
//! Prefer the explicit modes over the legacy `new(bool)`:
//! - `MockBrokerExecutor::accept_all()` — accepts every order, leaves it `New`.
//! - `MockBrokerExecutor::reject_all()` — refuses every order/cancel.

pub use vyapaar_exec_core::MockBrokerExecutor;
