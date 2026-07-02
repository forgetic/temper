//! Minimal stdio MCP/JSON-RPC client for agent-local tool bridges.
//!
//! This module intentionally exposes only the low-level pieces the coding agent
//! needs for a local codebase-memory MCP server: spawn a configured command over
//! stdio, initialize, list tools, call a tool, and kill the child when the last
//! client/tool handle is dropped. It does not try to be a full MCP SDK.

mod client;
mod connection;
mod protocol;

pub use client::{McpError, StdioMcpClient, StdioMcpServerConfig};
pub use protocol::{McpToolCallResult, McpToolDescriptor};

#[cfg(test)]
mod tests;
