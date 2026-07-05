//! Behavioral integration tests for the file-writing subcommands (`:generate` vscode/zed/jetbrains,
//! `:init`, `:convert`), driving the actual `run` binary as a subprocess.
//!
//! These cover the CLI wiring that the generators-crate unit tests can't reach: that `--stdout`
//! prints the generated config and writes nothing to disk, that the on-disk paths produce the right
//! files, and that every writer honors `.editorconfig`. Each test runs in an isolated temp dir with
//! `HOME` / `XDG_CONFIG_HOME` / `APPDATA` pointed at an empty dir so user settings and global
//! Runfiles can't leak targets into the output.

use std::path::Path;
use std::process::{Command, Output};

/// Run the compiled `run` binary in `dir` with a hermetic environment.
fn run_in(dir: &Path, args: &[&str]) -> Output {
	let home = dir.join("_home");
	std::fs::create_dir_all(&home).unwrap();
	Command::new(env!("CARGO_BIN_EXE_run"))
		.args(args)
		.current_dir(dir)
		.env("HOME", &home)
		.env("XDG_CONFIG_HOME", home.join(".config"))
		.env("APPDATA", home.join("AppData"))
		.env_remove("RUNFILE_TARGET")
		.env_remove("RUNFILE_ENV_FILE_TARGET")
		.output()
		.expect("run binary executes")
}

fn write(path: &Path, contents: &str) {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).unwrap();
	}
	std::fs::write(path, contents).unwrap();
}

fn stdout_of(out: &Output) -> String {
	String::from_utf8(out.stdout.clone()).expect("stdout is UTF-8")
}

const RUNFILE_TWO_TARGETS: &str = r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo build"] }, "test": { "commands": ["echo test"] } } }"#;
const RUNFILE_ONE_TARGET: &str = r#"{ "$schema": "x", "targets": { "build": { "commands": ["echo build"] } } }"#;

const EDITORCONFIG_2SPACE_FINAL_NL: &str =
	"root = true\n[*]\nindent_style = space\nindent_size = 2\ninsert_final_newline = true\n";

// ── :generate --stdout writes nothing to disk ────────────────────────────

#[test]
fn generate_vscode_stdout_prints_config_and_writes_nothing() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), RUNFILE_TWO_TARGETS);
	write(&root.join(".editorconfig"), EDITORCONFIG_2SPACE_FINAL_NL);

	let out = run_in(root, &[":generate", "vscode-tasks", "--stdout"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let stdout = stdout_of(&out);
	assert!(
		stdout.contains("\"label\": \"run build\""),
		"missing build task:\n{stdout}"
	);
	assert!(
		stdout.contains("\"label\": \"run test\""),
		"missing test task:\n{stdout}"
	);
	// EditorConfig: 2-space indent + final newline applied to stdout output.
	assert!(stdout.contains("\n  \"version\""), "expected 2-space indent:\n{stdout}");
	assert!(
		stdout.ends_with("\n"),
		"expected trailing newline from insert_final_newline"
	);

	// Nothing written to disk.
	assert!(!root.join(".vscode").exists(), "--stdout must not create .vscode/");
}

#[test]
fn generate_zed_stdout_prints_array_and_writes_nothing() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), RUNFILE_TWO_TARGETS);

	let out = run_in(root, &[":generate", "zed-tasks", "--stdout"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let stdout = stdout_of(&out);
	assert!(
		stdout.trim_start().starts_with('['),
		"zed output should be a JSON array:\n{stdout}"
	);
	assert!(
		stdout.contains("\"label\": \"run build\""),
		"missing build task:\n{stdout}"
	);
	assert!(!root.join(".zed").exists(), "--stdout must not create .zed/");
}

#[test]
fn generate_jetbrains_stdout_multi_has_headers() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), RUNFILE_TWO_TARGETS);

	let out = run_in(root, &[":generate", "jetbrains-run-configurations", "--stdout"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let stdout = stdout_of(&out);
	assert!(
		stdout.contains("<!-- .run/Runfile_build.run.xml -->"),
		"expected filename header for build:\n{stdout}"
	);
	assert!(
		stdout.contains("<!-- .run/Runfile_test.run.xml -->"),
		"expected filename header for test:\n{stdout}"
	);
	assert_eq!(
		stdout.matches("<component").count(),
		2,
		"expected two run configs:\n{stdout}"
	);
	assert!(!root.join(".run").exists(), "--stdout must not create .run/");
}

#[test]
fn generate_jetbrains_stdout_single_has_no_header() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), RUNFILE_ONE_TARGET);

	let out = run_in(root, &[":generate", "jetbrains-run-configurations", "--stdout"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let stdout = stdout_of(&out);
	assert!(
		!stdout.contains("<!--"),
		"single config should have no header comment:\n{stdout}"
	);
	assert!(
		stdout.trim_start().starts_with("<component"),
		"single config should start with <component:\n{stdout}"
	);
}

#[test]
fn generate_jetbrains_stdout_honors_output_dir_in_header() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), RUNFILE_TWO_TARGETS);

	let out = run_in(
		root,
		&[
			":generate",
			"jetbrains-run-configurations",
			"-o",
			".idea/runConfigurations",
			"--stdout",
		],
	);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let stdout = stdout_of(&out);
	assert!(
		stdout.contains("<!-- .idea/runConfigurations/Runfile_build.run.xml -->"),
		"header should reflect --output-dir:\n{stdout}"
	);
	assert!(!root.join(".idea").exists(), "--stdout must not create the output dir");
}

// ── :generate --include-namespaces pulls in included/namespaced targets ──

/// A root Runfile that includes a sub-Runfile under the `api` namespace, plus a
/// plain (un-namespaced) include. Written into `dir` alongside the two includes.
fn write_namespaced_project(root: &Path) {
	write(
		&root.join("Runfile.json"),
		r#"{
			"$schema": "x",
			"includes": [
				{ "path": "api/Runfile.json", "namespace": "api" },
				"shared/Runfile.json"
			],
			"targets": { "build": { "commands": ["echo root build"] } }
		}"#,
	);
	write(
		&root.join("api/Runfile.json"),
		r#"{ "$schema": "x", "targets": { "deploy": { "commands": ["echo api deploy"] } } }"#,
	);
	write(
		&root.join("shared/Runfile.json"),
		r#"{ "$schema": "x", "targets": { "clean": { "commands": ["echo shared clean"] } } }"#,
	);
}

#[test]
fn generate_vscode_without_flag_excludes_namespaced_targets() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write_namespaced_project(root);

	let out = run_in(root, &[":generate", "vscode-tasks", "--stdout"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let stdout = stdout_of(&out);
	assert!(
		stdout.contains("\"label\": \"run build\""),
		"expected root build:\n{stdout}"
	);
	assert!(
		!stdout.contains("run api:deploy"),
		"namespaced target must be absent without the flag:\n{stdout}"
	);
	assert!(
		!stdout.contains("run clean"),
		"included target must be absent without the flag:\n{stdout}"
	);
}

#[test]
fn generate_vscode_stdout_include_namespaces_adds_prefixed_targets() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write_namespaced_project(root);

	let out = run_in(root, &[":generate", "vscode-tasks", "--stdout", "--include-namespaces"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let stdout = stdout_of(&out);
	assert!(
		stdout.contains("\"label\": \"run build\""),
		"expected root build:\n{stdout}"
	);
	// Namespaced include: target name carries the `api:` prefix.
	assert!(
		stdout.contains("\"label\": \"run api:deploy\""),
		"expected namespaced api:deploy task:\n{stdout}"
	);
	// The invocation arg is the prefixed name so `run api:deploy` dispatches correctly.
	assert!(
		stdout.contains("\"api:deploy\""),
		"expected api:deploy invocation arg:\n{stdout}"
	);
	// Plain (un-namespaced) include contributes its target verbatim.
	assert!(
		stdout.contains("\"label\": \"run clean\""),
		"expected plain-include clean task:\n{stdout}"
	);
	assert!(!root.join(".vscode").exists(), "--stdout must not create .vscode/");
}

#[test]
fn generate_vscode_writes_file_with_include_namespaces() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write_namespaced_project(root);

	let out = run_in(root, &[":generate", "vscode-tasks", "--include-namespaces"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let tasks = std::fs::read_to_string(root.join(".vscode/tasks.json")).expect(".vscode/tasks.json written");
	assert!(
		tasks.contains("\"label\": \"run build\""),
		"expected root build:\n{tasks}"
	);
	assert!(
		tasks.contains("\"label\": \"run api:deploy\""),
		"expected namespaced api:deploy task:\n{tasks}"
	);
	assert!(
		tasks.contains("\"label\": \"run clean\""),
		"expected plain-include clean task:\n{tasks}"
	);
}

#[test]
fn generate_jetbrains_include_namespaces_sanitizes_colon_in_filename() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write_namespaced_project(root);

	let out = run_in(
		root,
		&[":generate", "jetbrains-run-configurations", "--include-namespaces"],
	);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	// The colon in `api:deploy` is sanitized to `_` in the filename, but the run
	// invocation keeps the prefixed name.
	let cfg = std::fs::read_to_string(root.join(".run/Runfile_api_deploy.run.xml"))
		.expect(".run/Runfile_api_deploy.run.xml written");
	assert!(
		cfg.contains("value=\"run --stdin-args api:deploy\""),
		"expected namespaced invocation:\n{cfg}"
	);
	assert!(
		root.join(".run/Runfile_build.run.xml").is_file(),
		"root target config should also exist"
	);
}

// ── :generate writes files, honoring .editorconfig ───────────────────────

#[test]
fn generate_vscode_writes_file_with_editorconfig_formatting() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(&root.join("Runfile.json"), RUNFILE_TWO_TARGETS);
	write(&root.join(".editorconfig"), EDITORCONFIG_2SPACE_FINAL_NL);

	let out = run_in(root, &[":generate", "vscode-tasks"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let tasks = std::fs::read_to_string(root.join(".vscode/tasks.json")).expect(".vscode/tasks.json written");
	assert!(tasks.contains("\n  \"version\""), "expected 2-space indent:\n{tasks}");
	assert!(tasks.ends_with('\n'), "expected final newline");
	assert!(tasks.contains("\"label\": \"run build\""));
}

// ── :init honors .editorconfig ───────────────────────────────────────────

#[test]
fn init_default_uses_tab_indent_and_lf() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();

	let out = run_in(root, &[":init"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let bytes = std::fs::read(root.join("Runfile.json")).expect("Runfile.json written");
	let text = String::from_utf8(bytes).unwrap();
	// Historical default: tab indentation, LF, trailing newline — unchanged when no .editorconfig.
	assert!(
		text.contains("\n\t\"$schema\""),
		"expected tab indent by default:\n{text}"
	);
	assert!(!text.contains("\r\n"), "expected LF by default");
	assert!(text.ends_with('\n'));
}

#[test]
fn init_respects_editorconfig_spaces_and_crlf() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(
		&root.join(".editorconfig"),
		"root = true\n[*]\nindent_style = space\nindent_size = 2\nend_of_line = crlf\ninsert_final_newline = true\n",
	);

	let out = run_in(root, &[":init"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let bytes = std::fs::read(root.join("Runfile.json")).expect("Runfile.json written");
	let text = String::from_utf8(bytes).unwrap();
	assert!(text.contains("\r\n"), "expected CRLF line endings:\n{text:?}");
	assert!(
		text.contains("\r\n  \"$schema\""),
		"expected 2-space indent with CRLF:\n{text:?}"
	);
	assert!(
		!text.contains('\t'),
		"tabs should have been converted to spaces:\n{text:?}"
	);
	assert!(text.ends_with("\r\n"));
}

// ── :convert honors .editorconfig ────────────────────────────────────────

#[test]
fn convert_package_json_respects_editorconfig() {
	let dir = tempfile::tempdir().unwrap();
	let root = dir.path();
	write(
		&root.join("package.json"),
		r#"{ "scripts": { "build": "webpack", "lint": "eslint ." } }"#,
	);
	write(
		&root.join(".editorconfig"),
		"root = true\n[*]\nindent_style = space\nindent_size = 2\nend_of_line = crlf\ninsert_final_newline = true\n",
	);

	let out = run_in(root, &[":convert", "package-json"]);
	assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

	let bytes = std::fs::read(root.join("Runfile.json")).expect("Runfile.json written");
	let text = String::from_utf8(bytes).unwrap();
	assert!(text.contains("\r\n"), "expected CRLF line endings:\n{text:?}");
	assert!(text.contains("\r\n  \"$schema\""), "expected 2-space indent:\n{text:?}");
	assert!(text.ends_with("\r\n"), "expected final newline");
	// The converted targets are present.
	assert!(text.contains("\"build\""));
	assert!(text.contains("\"lint\""));
}
