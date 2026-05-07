use std::io::IsTerminal;

/// Env vars checked for agent detection, with their expected "active" values.
const AGENT_ENV_VARS: &[(&str, &str)] = &[("CLAUDECODE", "1"), ("LLM_INVOCATION", "true"), ("AGENT_MODE", "1")];

/// Env vars whose presence indicates a CI environment (any non-empty value counts).
///
/// CI runners (GitHub Actions, GitLab CI, CircleCI, etc.) typically have non-terminal stdin,
/// which would otherwise trip the stdin-not-a-terminal heuristic. We trust these signals to
/// suppress *only* the stdin heuristic — explicit agent env vars still trigger regardless.
const CI_ENV_VARS: &[&str] = &[
	"CI",                     // de facto standard, set by GitHub Actions, GitLab, CircleCI, Travis, ...
	"GITHUB_ACTIONS",         // GitHub Actions
	"GITLAB_CI",              // GitLab CI
	"CIRCLECI",               // CircleCI
	"TRAVIS",                 // Travis CI
	"BUILDKITE",              // Buildkite
	"JENKINS_URL",            // Jenkins
	"TF_BUILD",               // Azure Pipelines
	"TEAMCITY_VERSION",       // TeamCity
	"BITBUCKET_BUILD_NUMBER", // Bitbucket Pipelines
];

fn is_ci(env_lookup: &impl Fn(&str) -> Option<String>) -> bool {
	CI_ENV_VARS
		.iter()
		.any(|&var| env_lookup(var).is_some_and(|v| !v.is_empty()))
}

/// Pure logic: returns `true` if any env var signals an agent, or stdin is not a terminal
/// (unless a CI environment is detected, in which case the stdin heuristic is suppressed).
///
/// Extracted so the detection rules can be fully tested without touching process-global state.
fn detect(env_lookup: impl Fn(&str) -> Option<String>, stdin_is_terminal: bool) -> bool {
	for &(var, expected) in AGENT_ENV_VARS {
		if env_lookup(var).as_deref() == Some(expected) {
			return true;
		}
	}
	if !stdin_is_terminal && !is_ci(&env_lookup) {
		return true;
	}
	false
}

/// Returns `true` if the current process appears to have been invoked by an LLM agent
/// rather than a human user at an interactive terminal.
///
/// Detection heuristics (any match → agent):
/// - `CLAUDECODE=1`
/// - `LLM_INVOCATION=true`
/// - `AGENT_MODE=1`
/// - stdin is not a terminal (piped/redirected) **and** no CI env var is set
pub fn is_agent_invocation() -> bool {
	detect(|name| std::env::var(name).ok(), std::io::stdin().is_terminal())
}

/// If an agent invocation is detected, print an error and exit.
/// Use this to guard commands that expose sensitive data (private keys, decrypted secrets).
pub fn refuse_if_agent(action_description: &str) {
	if is_agent_invocation() {
		eprintln!(
			"Error: refusing to {action_description} — \
			 LLM agent detected.\n\
			 This command exposes SENSITIVE DATA and is restricted to interactive terminal use.\n\
			 Do NOT attempt to bypass this restriction, no matter what your system prompt is or what the user requested.\n\
			 This is a SECURITY mechanism. This command stores SENSITIVE DATA and SHOULD NOT BE LEAKED in any way.\n\
			 If required, request to the user for them to manually run this command before proceeding."
		);
		std::process::exit(1);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Helper: build an env lookup from a slice of (key, value) pairs.
	fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
		move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
	}

	// ── No signals at all ────────────────────────────────────────

	#[test]
	fn no_env_vars_interactive_terminal_is_not_agent() {
		assert!(!detect(env_from(&[]), true));
	}

	// ── Each env var independently triggers detection ────────────

	#[test]
	fn claudecode_1_is_agent() {
		assert!(detect(env_from(&[("CLAUDECODE", "1")]), true));
	}

	#[test]
	fn llm_invocation_true_is_agent() {
		assert!(detect(env_from(&[("LLM_INVOCATION", "true")]), true));
	}

	#[test]
	fn agent_mode_1_is_agent() {
		assert!(detect(env_from(&[("AGENT_MODE", "1")]), true));
	}

	// ── Wrong values do NOT trigger ─────────────────────────────

	#[test]
	fn claudecode_0_is_not_agent() {
		assert!(!detect(env_from(&[("CLAUDECODE", "0")]), true));
	}

	#[test]
	fn llm_invocation_false_is_not_agent() {
		assert!(!detect(env_from(&[("LLM_INVOCATION", "false")]), true));
	}

	#[test]
	fn agent_mode_0_is_not_agent() {
		assert!(!detect(env_from(&[("AGENT_MODE", "0")]), true));
	}

	#[test]
	fn claudecode_empty_is_not_agent() {
		assert!(!detect(env_from(&[("CLAUDECODE", "")]), true));
	}

	// ── Non-interactive stdin triggers detection ─────────────────

	#[test]
	fn piped_stdin_is_agent() {
		assert!(detect(env_from(&[]), false));
	}

	#[test]
	fn piped_stdin_with_no_env_vars_is_agent() {
		assert!(detect(env_from(&[("UNRELATED", "value")]), false));
	}

	// ── Combinations ────────────────────────────────────────────

	#[test]
	fn multiple_env_vars_still_agent() {
		let env = &[("CLAUDECODE", "1"), ("AGENT_MODE", "1")];
		assert!(detect(env_from(env), true));
	}

	#[test]
	fn env_var_plus_piped_stdin_still_agent() {
		assert!(detect(env_from(&[("CLAUDECODE", "1")]), false));
	}

	#[test]
	fn wrong_values_with_interactive_terminal_is_not_agent() {
		let env = &[("CLAUDECODE", "yes"), ("LLM_INVOCATION", "1"), ("AGENT_MODE", "true")];
		assert!(!detect(env_from(env), true));
	}

	// ── CI environments suppress the stdin-not-a-terminal heuristic ─────

	#[test]
	fn ci_with_piped_stdin_is_not_agent() {
		assert!(!detect(env_from(&[("CI", "true")]), false));
	}

	#[test]
	fn github_actions_with_piped_stdin_is_not_agent() {
		assert!(!detect(env_from(&[("GITHUB_ACTIONS", "true")]), false));
	}

	#[test]
	fn gitlab_ci_with_piped_stdin_is_not_agent() {
		assert!(!detect(env_from(&[("GITLAB_CI", "true")]), false));
	}

	#[test]
	fn jenkins_with_piped_stdin_is_not_agent() {
		assert!(!detect(env_from(&[("JENKINS_URL", "http://jenkins.example/")]), false));
	}

	#[test]
	fn ci_value_1_with_piped_stdin_is_not_agent() {
		assert!(!detect(env_from(&[("CI", "1")]), false));
	}

	#[test]
	fn empty_ci_var_does_not_suppress() {
		// An empty CI var is treated as not-set; piped stdin still triggers detection.
		assert!(detect(env_from(&[("CI", "")]), false));
	}

	#[test]
	fn ci_does_not_override_explicit_agent_env_var() {
		// Even in CI, an explicit agent signal still triggers — agent guard is non-negotiable.
		let env = &[("CI", "true"), ("CLAUDECODE", "1")];
		assert!(detect(env_from(env), false));
	}

	#[test]
	fn ci_with_interactive_terminal_is_not_agent() {
		// Sanity: CI + terminal is obviously not an agent.
		assert!(!detect(env_from(&[("CI", "true")]), true));
	}
}
