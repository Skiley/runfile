use crate::install::{InstallResult, install_for_agent, mcp_config_snippet};
use crate::tools::{build_tool_defs, inspect_json};
use runfile_parser::{CommandSpec, EnvValue, RUNFILE_NAME, Runfile};
use std::collections::HashMap;

fn make_runfile(targets: Vec<(&str, CommandSpec)>) -> Runfile {
	let mut target_map = HashMap::new();
	for (name, spec) in targets {
		target_map.insert(name.to_string(), spec);
	}
	Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: target_map,
		globals: None,
		namespaces: Vec::new(),
	}
}

fn simple_spec(commands: Vec<&str>, description: Option<&str>) -> CommandSpec {
	let mut spec = CommandSpec::new_shell(commands.into_iter().map(String::from).collect());
	spec.description = description.map(String::from);
	spec
}

mod inspect;
mod install;
mod security;
mod server;
mod tool_defs;
