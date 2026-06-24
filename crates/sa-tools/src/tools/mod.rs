//! Phase 6c network tools — one module per tool (mirroring the per-file connectors). Each goes
//! through the `crate::egress` seam; none touches `reqwest` directly.

pub mod http_request;
pub mod shell;
pub mod web_extract;
pub mod web_search;
