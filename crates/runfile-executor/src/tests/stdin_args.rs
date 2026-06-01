use super::*;
use crate::args::StdinPrompter;
use std::sync::{Arc, Mutex};

/// Test prompter that returns scripted answers and records every prompt.
#[derive(Debug, Default)]
struct MockPrompter {
	value_answers: Mutex<HashMap<String, Option<String>>>,
	flag_answers: Mutex<HashMap<String, bool>>,
	value_calls: Mutex<Vec<String>>,
	flag_calls: Mutex<Vec<String>>,
}

impl MockPrompter {
	fn with_value(self, key: &str, answer: Option<&str>) -> Self {
		self.value_answers
			.lock()
			.unwrap()
			.insert(key.to_string(), answer.map(|s| s.to_string()));
		self
	}
	fn with_flag(self, key: &str, present: bool) -> Self {
		self.flag_answers.lock().unwrap().insert(key.to_string(), present);
		self
	}
}

impl StdinPrompter for MockPrompter {
	fn prompt_value(&self, key: &str) -> Option<String> {
		self.value_calls.lock().unwrap().push(key.to_string());
		self.value_answers.lock().unwrap().get(key).cloned().unwrap_or(None)
	}
	fn prompt_flag(&self, key: &str) -> bool {
		self.flag_calls.lock().unwrap().push(key.to_string());
		self.flag_answers.lock().unwrap().get(key).copied().unwrap_or(false)
	}
}

fn args_with(prompter: Arc<dyn StdinPrompter>) -> RunArgs {
	RunArgs::parse(&[]).with_stdin_prompter(Some(prompter))
}

#[test]
fn missing_args_prompts_and_uses_answer() {
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.name", Some("alice")));
	let args = args_with(prompter.clone());
	let result = args.substitute("hello {{ ARG.name }}", &HashMap::new()).unwrap();
	assert_eq!(result, "hello alice");
	let calls = prompter.value_calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0], "ARG.name");
}

#[test]
fn arg_with_default_uses_default_without_prompting() {
	// A chain with a literal default resolves to that default WITHOUT
	// prompting — even when an answer is scripted, it is never consulted.
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.env", Some("staging")));
	let args = args_with(prompter.clone());
	let result = args
		.substitute("env={{ ARG.env ? 'production' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "env=production");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn arg_with_empty_default_uses_empty_without_prompting() {
	// The trailing `?` empty-string default also counts as a default, so it
	// short-circuits the prompt.
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.env", Some("staging")));
	let args = args_with(prompter.clone());
	let result = args.substitute("env={{ ARG.env ? }}", &HashMap::new()).unwrap();
	assert_eq!(result, "env=");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn missing_args_no_default_no_answer_errors() {
	// Required substitution; user pressed Enter; nothing else in the chain
	// → fall through to MissingArg as if --stdin-args wasn't set.
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.name", None));
	let args = args_with(prompter);
	let err = args.substitute("hi {{ ARG.name }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::MissingArg(ref k) if k == "name"));
}

#[test]
fn provided_args_skip_prompt() {
	let prompter = Arc::new(MockPrompter::default());
	let args = RunArgs::parse(&["--name=bob".into()]).with_stdin_prompter(Some(prompter.clone()));
	let result = args.substitute("hi {{ ARG.name }}", &HashMap::new()).unwrap();
	assert_eq!(result, "hi bob");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn missing_positional_args_prompts_for_bare_args() {
	// Bare `{{ ARGS }}` with no positional args should prompt under
	// --stdin-args (the bump-target use case: `"match": "{{ ARGS }}"`).
	let prompter = Arc::new(MockPrompter::default().with_value("ARGS", Some("major")));
	let args = args_with(prompter.clone());
	let result = args.substitute("part={{ ARGS }}", &HashMap::new()).unwrap();
	assert_eq!(result, "part=major");
	let calls = prompter.value_calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0], "ARGS");
}

#[test]
fn missing_positional_args_empty_answer_falls_back_to_empty() {
	// Empty answer (user pressed Enter): `{{ ARGS }}` resolves to "",
	// matching prior behavior.
	let prompter = Arc::new(MockPrompter::default().with_value("ARGS", None));
	let args = args_with(prompter);
	let result = args.substitute("part={{ ARGS }}", &HashMap::new()).unwrap();
	assert_eq!(result, "part=");
}

#[test]
fn provided_positional_args_skip_bare_args_prompt() {
	let prompter = Arc::new(MockPrompter::default());
	let args = RunArgs::parse(&["minor".into()]).with_stdin_prompter(Some(prompter.clone()));
	let result = args.substitute("part={{ ARGS }}", &HashMap::new()).unwrap();
	assert_eq!(result, "part=minor");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn missing_env_prompts_and_uses_answer() {
	let prompter = Arc::new(MockPrompter::default().with_value("ENV.SECRET", Some("hush")));
	let args = args_with(prompter);
	let result = args.substitute("token={{ ENV.SECRET }}", &HashMap::new()).unwrap();
	assert_eq!(result, "token=hush");
}

#[test]
fn provided_env_skips_prompt() {
	let prompter = Arc::new(MockPrompter::default());
	let args = args_with(prompter.clone());
	let mut env = HashMap::new();
	env.insert("HOST".to_string(), "example.com".to_string());
	let result = args.substitute("host={{ ENV.HOST }}", &env).unwrap();
	assert_eq!(result, "host=example.com");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn chain_with_default_uses_default_without_prompting() {
	// `{{ ARG.x ? ENV.X ? 'fallback' }}` — neither source set, but the chain
	// has a literal default, so it resolves to 'fallback' without prompting.
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.x", Some("entered")));
	let args = args_with(prompter.clone());
	let result = args
		.substitute("v={{ ARG.x ? ENV.X ? 'fallback' }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "v=fallback");
	assert!(prompter.value_calls.lock().unwrap().is_empty());
}

#[test]
fn chain_without_default_prompts_once_with_first_source_key() {
	// `{{ ARG.x ? ENV.X }}` — neither source set and no literal default, so
	// the user IS prompted, keyed on the first source (ARG.x).
	let prompter = Arc::new(MockPrompter::default().with_value("ARG.x", Some("entered")));
	let args = args_with(prompter.clone());
	let result = args.substitute("v={{ ARG.x ? ENV.X }}", &HashMap::new()).unwrap();
	assert_eq!(result, "v=entered");
	let calls = prompter.value_calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0], "ARG.x");
}

#[test]
fn bare_flag_boolean_prompts_for_presence() {
	// The bare boolean form `{{ FLAG.x }}` has no explicit default, so it IS
	// prompted (y/N) under --stdin-args.
	let prompter = Arc::new(MockPrompter::default().with_flag("--verbose", true));
	let args = args_with(prompter.clone());
	let result = args.substitute("v={{ FLAG.verbose }}", &HashMap::new()).unwrap();
	assert_eq!(result, "v=true");
	let calls = prompter.flag_calls.lock().unwrap();
	assert_eq!(calls.len(), 1);
	assert_eq!(calls[0], "--verbose");
}

#[test]
fn bare_flag_boolean_declined_returns_false() {
	let prompter = Arc::new(MockPrompter::default().with_flag("--verbose", false));
	let args = args_with(prompter);
	let result = args.substitute("v={{ FLAG.verbose }}", &HashMap::new()).unwrap();
	assert_eq!(result, "v=false");
}

#[test]
fn flag_ternary_absent_uses_false_branch_without_prompting() {
	// The ternary form carries its own default (the false branch), so an
	// absent flag resolves to that branch without prompting — the scripted
	// "present" answer is never consulted.
	let prompter = Arc::new(MockPrompter::default().with_flag("--verbose", true));
	let args = args_with(prompter.clone());
	let result = args
		.substitute("cmd {{ FLAG.verbose ? '-v' : }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "cmd ");
	assert!(prompter.flag_calls.lock().unwrap().is_empty());
}

#[test]
fn flags_provided_skips_prompt() {
	let prompter = Arc::new(MockPrompter::default());
	let args = RunArgs::parse(&["--verbose".into()]).with_stdin_prompter(Some(prompter.clone()));
	let result = args
		.substitute("cmd {{ FLAG.verbose ? '-v' : }}", &HashMap::new())
		.unwrap();
	assert_eq!(result, "cmd -v");
	assert!(prompter.flag_calls.lock().unwrap().is_empty());
}

#[test]
fn flag_ternary_absent_uses_explicit_false_branch() {
	// Even with a scripted "present" answer, the ternary form does not prompt;
	// the absent flag selects the explicit false branch.
	let prompter = Arc::new(MockPrompter::default().with_flag("--release", true));
	let args = args_with(prompter.clone());
	let result = args
		.substitute(
			"cargo build {{ FLAG.release ? '--release' : '--debug' }}",
			&HashMap::new(),
		)
		.unwrap();
	assert_eq!(result, "cargo build --debug");
	assert!(prompter.flag_calls.lock().unwrap().is_empty());
}

#[test]
fn no_prompter_preserves_existing_error() {
	// Sanity check: with no prompter, missing args still error.
	let args = RunArgs::parse(&[]);
	let err = args.substitute("hi {{ ARG.name }}", &HashMap::new()).unwrap_err();
	assert!(matches!(err, SubstitutionError::MissingArg(_)));
}
