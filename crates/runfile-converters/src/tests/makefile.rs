use super::*;

// ── Makefile tests ─────────────────────────────────────────────────

#[test]
fn convert_simple_makefile() {
	let makefile = "\
.PHONY: build test clean

build:
\tcargo build --release

test:
\tcargo test

clean:
\trm -rf target/
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets.len(), 3);
	assert!(result.targets.contains_key("build"));
	assert!(result.targets.contains_key("test"));
	assert!(result.targets.contains_key("clean"));
	assert_eq!(result.targets["build"].commands, vec!["cargo build --release"]);
}

#[test]
fn convert_makefile_with_deps() {
	let makefile = "\
.PHONY: all build test

all: build test
\techo done

build:
\tcargo build

test:
\tcargo test
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let all = &result.targets["all"];
	// Make deps now appear as `@target` invocations at the start of `commands`.
	let mut targets: Vec<&str> = Vec::new();
	for cmd in &all.commands {
		if let runfile_parser::CommandStep::TargetCall(call) = cmd {
			targets.push(&call.target);
		}
	}
	assert_eq!(targets, &["build", "test"]);
}

#[test]
fn convert_makefile_strips_silent_prefix() {
	let makefile = "\
.PHONY: quiet

quiet:
\t@echo hello
\t@echo world
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["quiet"].commands, vec!["echo hello", "echo world"]);
}

#[test]
fn convert_makefile_skips_special_targets() {
	let makefile = "\
.PHONY: build
.SUFFIXES:
.DEFAULT:
\techo default

build:
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("build"));
}

#[test]
fn convert_makefile_expands_variables() {
	let makefile = "\
CC = gcc
CFLAGS = -Wall -O2

.PHONY: build

build:
\t$(CC) $(CFLAGS) main.c -o main
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["gcc -Wall -O2 main.c -o main"]);
}

#[test]
fn convert_makefile_skips_on_collision() {
	let mut existing = HashSet::new();
	existing.insert("build".to_string());

	let makefile = "\
.PHONY: build test

build:
\tcargo build

test:
\tcargo test
";
	let result = crate::convert_makefile(makefile, &existing);
	assert!(!result.targets.contains_key("build"));
	assert!(result.targets.contains_key("test"));
	assert_eq!(result.skipped, vec!["build"]);
}

#[test]
fn convert_makefile_multi_command_target() {
	let makefile = "\
.PHONY: deploy

deploy:
\techo Building...
\tcargo build --release
\techo Deploying...
\t./deploy.sh
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["deploy"].commands.len(), 4);
}

#[test]
fn convert_makefile_skips_file_targets() {
	let makefile = "\
.PHONY: build

build:
\tcargo build

target/release/myapp: src/main.rs
\tcargo build --release
";
	// target/release/myapp has a dot and slash, non-phony with file deps — should still be included
	// since it has commands. But its dep (src/main.rs) should not become dependsOn since it has a slash.
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("build"));
}

#[test]
fn convert_makefile_skips_comments_and_blanks() {
	let makefile = "\
# This is a comment
.PHONY: build

# Another comment
build:
\t# inline comment gets kept (it's a valid shell comment)
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands.len(), 2);
}

#[test]
fn convert_makefile_export_as_env() {
	let makefile = "\
.PHONY: serve

serve:
\texport PORT=3000
\tnode server.js
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["serve"];
	assert!(spec.env.as_ref().unwrap().contains_key("PORT"));
	assert_eq!(spec.commands, vec!["node server.js"]);
}

#[test]
fn convert_makefile_line_continuation() {
	let makefile = ".PHONY: build\n\nbuild:\n\tgcc -Wall \\\n\t\t-O2 \\\n\t\t-o main main.c\n";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["build"];
	assert_eq!(spec.commands.len(), 1);
	assert_eq!(spec.commands[0], "gcc -Wall  \t\t-O2  \t\t-o main main.c");
}

#[test]
fn convert_makefile_line_continuation_in_variable() {
	let makefile = "\
SOURCES = foo.c \\\n  bar.c \\\n  baz.c

.PHONY: build

build:
\tgcc $(SOURCES) -o app
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["build"];
	// The continuation joins lines with a space, preserving the indentation from the original
	assert!(spec.commands[0].contains("gcc"));
	assert!(spec.commands[0].contains("foo.c"));
	assert!(spec.commands[0].contains("bar.c"));
	assert!(spec.commands[0].contains("baz.c"));
	assert!(spec.commands[0].contains("-o app"));
	assert_eq!(spec.commands.len(), 1);
}

#[test]
fn convert_makefile_multiname_target() {
	let makefile = "\
.PHONY: kill-docker cleanup

kill-docker cleanup:
\tdocker kill foo
\tdocker rm bar
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["kill-docker"];
	assert_eq!(spec.commands, vec!["docker kill foo", "docker rm bar"]);
	assert_eq!(spec.aliases.as_ref().unwrap(), &["cleanup"]);
}

#[test]
fn convert_makefile_multiname_does_not_leak_recipes() {
	let makefile = "\
.PHONY: aaa bbb ccc

aaa:
\techo aaa

bbb ccc:
\techo bbb

";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["aaa"].commands, vec!["echo aaa"]);
	assert_eq!(result.targets["bbb"].commands, vec!["echo bbb"]);
	assert!(
		result.targets["bbb"]
			.aliases
			.as_ref()
			.unwrap()
			.contains(&"ccc".to_string())
	);
}

#[test]
fn convert_makefile_inline_env_not_extracted() {
	// Bare VAR=value at start of command is shell inline-env, not an export
	let makefile = "\
.PHONY: test

test:
\tENV=test NODE_ENV=test node app.js
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["test"];
	assert!(
		spec.env.is_none(),
		"inline env vars should stay as command, not be extracted"
	);
	assert_eq!(spec.commands.len(), 1);
	assert!(spec.commands[0].contains("ENV=test"));
	assert!(spec.commands[0].contains("node app.js"));
}

#[test]
fn convert_makefile_continuation_with_inline_env() {
	// Real-world pattern: inline env vars with continuation lines
	let makefile = "\
.PHONY: func-test

func-test:
\t@ENV=test \\\n\tNODE_ENV=test \\\n\tnpx cucumber-js \\\n\t--backtrace --exit
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["func-test"];
	assert_eq!(spec.commands.len(), 1);
	assert!(spec.commands[0].contains("ENV=test"));
	assert!(spec.commands[0].contains("NODE_ENV=test"));
	assert!(spec.commands[0].contains("npx cucumber-js"));
	assert!(spec.commands[0].contains("--backtrace --exit"));
	assert!(spec.env.is_none());
}

// ══════════════════════════════════════════════════════════════════════
// Additional Makefile converter coverage
// ══════════════════════════════════════════════════════════════════════

#[test]
fn convert_makefile_pattern_rule_skipped() {
	let makefile = "\
.PHONY: build

build:
\tgcc -o app main.c

%.o: %.c
\tgcc -c $< -o $@
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets.len(), 1);
	assert!(result.targets.contains_key("build"));
}

#[test]
fn convert_makefile_variable_conditional_assignment() {
	let makefile = "\
CC ?= gcc

.PHONY: build

build:
\t$(CC) main.c
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["gcc main.c"]);
}

#[test]
fn convert_makefile_variable_append_assignment() {
	let makefile = "\
FLAGS = -Wall
FLAGS += -O2

.PHONY: build

build:
\tgcc $(FLAGS) main.c
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	// Note: += just overwrites in the simple parser
	let cmd = &result.targets["build"].commands[0];
	assert!(cmd.contains("gcc"));
	assert!(cmd.contains("main.c"));
}

#[test]
fn convert_makefile_curly_brace_variable() {
	let makefile = "\
CC = gcc

.PHONY: build

build:
\t${CC} main.c
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["gcc main.c"]);
}

#[test]
fn convert_makefile_unknown_variable_kept_as_is() {
	let makefile = "\
.PHONY: build

build:
\techo $(UNKNOWN_VAR)
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["echo $(UNKNOWN_VAR)"]);
}

#[test]
fn convert_makefile_function_call_kept_as_is() {
	let makefile = "\
.PHONY: build

build:
\techo $(shell uname -s)
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["build"].commands, vec!["echo $(shell uname -s)"]);
}

#[test]
fn convert_makefile_no_targets() {
	let makefile = "# just a comment\n\n";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.is_empty());
}

#[test]
fn convert_makefile_target_with_file_deps_filtered() {
	let makefile = "\
.PHONY: build

build: src/main.rs Cargo.toml
\tcargo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["build"];
	// File-like deps (with / or .) should be filtered out — no leading `@target`.
	assert!(
		!matches!(&spec.commands[0], runfile_parser::CommandStep::TargetCall(_)),
		"File deps should not become @target invocations"
	);
}

#[test]
fn convert_makefile_target_with_phony_deps_kept() {
	let makefile = "\
.PHONY: all build test

all: build test
\techo done

build:
\tcargo build

test:
\tcargo test
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["all"];
	let mut targets: Vec<&str> = Vec::new();
	for cmd in &spec.commands {
		if let runfile_parser::CommandStep::TargetCall(call) = cmd {
			targets.push(&call.target);
		}
	}
	assert_eq!(targets, &["build", "test"]);
}

#[test]
fn convert_makefile_export_with_quoted_value() {
	let makefile = "\
.PHONY: test

test:
\texport PORT=\"3000\"
\tnode server.js
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	let spec = &result.targets["test"];
	assert!(spec.env.as_ref().unwrap().contains_key("PORT"));
	assert_eq!(spec.commands, vec!["node server.js"]);
}

#[test]
fn convert_makefile_non_phony_with_commands_included() {
	// Non-phony targets with commands should still be included
	let makefile = "\
myapp:
\tgcc main.c -o myapp
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("myapp"));
}

#[test]
fn convert_makefile_non_phony_empty_commands_skipped() {
	// Non-phony targets with no commands should be skipped (file targets)
	let makefile = "\
data.json: input.csv
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.is_empty());
}

#[test]
fn convert_makefile_strips_both_at_and_dash_prefixes() {
	let makefile = "\
.PHONY: test

test:
\t@echo silent
\t-echo ignore_error
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(
		result.targets["test"].commands,
		vec!["echo silent", "echo ignore_error"]
	);
}

#[test]
fn convert_makefile_colon_in_first_position_skipped() {
	// A line starting with : is not a valid target
	let makefile = "\
.PHONY: build

:weird:
\techo weird

build:
\techo build
";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert!(result.targets.contains_key("build"));
	assert!(!result.targets.contains_key(""));
}

// ── Audit L16: non-ASCII preserved in variable expansion ──

#[test]
fn expand_variables_preserves_non_ascii() {
	use std::collections::HashMap;
	let mut vars = HashMap::new();
	vars.insert("NAME".to_string(), "café ☕".to_string());
	let out = crate::makefile::expand_variables("echo $(NAME) — héllo", &vars);
	assert_eq!(out, "echo café ☕ — héllo");
	assert!(!out.contains('Ã'), "no mojibake: {out}");
}

#[test]
fn convert_makefile_preserves_non_ascii_command() {
	let makefile = "greet:\n\techo héllo wörld ☕\n";
	let result = crate::convert_makefile(makefile, &HashSet::new());
	assert_eq!(result.targets["greet"].commands, vec!["echo héllo wörld ☕"]);
}

// ── Audit L17: echo-fallback target name sanitized (no shell injection) ──

#[test]
fn sanitize_echo_text_strips_shell_metacharacters() {
	let malicious = "x\";rm -rf ~;echo \"";
	let safe = crate::makefile::sanitize_echo_text(malicious);
	for c in ['"', ';', '`', '$', '\\', '|', '&', '<', '>', '\n'] {
		assert!(!safe.contains(c), "sanitized text must not contain {c:?}: {safe}");
	}
}

#[test]
fn sanitize_echo_text_keeps_normal_names() {
	assert_eq!(crate::makefile::sanitize_echo_text("build:release"), "build:release");
	assert_eq!(crate::makefile::sanitize_echo_text("a-b_c.d/e f"), "a-b_c.d/e f");
}
