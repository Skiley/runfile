pub mod install;
pub mod server;
pub mod tools;

pub use install::{InstallResult, install_for_agent, mcp_config_snippet};
pub use server::{RunfileMcpServer, run_server};
pub use tools::{ToolDef, build_tool_defs, inspect_json};

#[cfg(test)]
mod tests;
