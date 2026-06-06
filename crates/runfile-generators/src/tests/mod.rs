use crate::jetbrains::*;
use crate::vscode::*;
use crate::zed::*;
use runfile_parser::{CommandSpec, Metadata, Runfile};
use std::collections::HashMap;

fn make_runfile(targets: Vec<(&str, Vec<&str>)>) -> Runfile {
	let mut target_map = HashMap::new();
	for (name, commands) in targets {
		target_map.insert(
			name.to_string(),
			CommandSpec::new_shell(commands.into_iter().map(String::from).collect()),
		);
	}
	Runfile {
		schema: "https://github.com/Skiley/runfile/releases/latest/download/v0.schema.json".into(),
		includes: None,
		targets: target_map,
		globals: None,
		namespaces: Vec::new(),
	}
}

/// Build a runfile where one target has `excludeFromGenerateCommand: true` baked
/// into its `metadata`. Other targets are left as-is. Used to verify each
/// generator skips the flagged target.
fn make_runfile_with_excluded(targets: Vec<(&str, Vec<&str>)>, excluded: &[&str]) -> Runfile {
	let mut rf = make_runfile(targets);
	for name in excluded {
		let spec = rf.targets.get_mut(*name).expect("target exists");
		spec.metadata = Some(Metadata {
			exclude_from_generate_command: Some(true),
			extra: Default::default(),
		});
	}
	rf
}

mod coverage;
mod exclude_metadata;
mod internal_targets;
mod jetbrains;
mod stdin_args;
mod vscode;
mod zed;
