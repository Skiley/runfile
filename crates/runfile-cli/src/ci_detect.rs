//! CI environment detection.
//!
//! Used by commands like `:env secret-keys add --key` that are only safe to use
//! non-interactively from automation.

/// Env vars whose presence indicates a CI environment (any non-empty value counts).
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

/// Pure logic: returns `true` if any known CI env var is set to a non-empty value.
/// Extracted for testability — pass any env lookup closure.
pub fn is_ci_with(env_lookup: impl Fn(&str) -> Option<String>) -> bool {
	CI_ENV_VARS
		.iter()
		.any(|&var| env_lookup(var).is_some_and(|v| !v.is_empty()))
}

/// Returns `true` if the current process appears to be running inside a CI environment.
pub fn is_ci() -> bool {
	is_ci_with(|name| std::env::var(name).ok())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn env_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
		move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
	}

	#[test]
	fn no_env_vars_is_not_ci() {
		assert!(!is_ci_with(env_from(&[])));
	}

	#[test]
	fn ci_env_var_set_is_ci() {
		assert!(is_ci_with(env_from(&[("CI", "true")])));
	}

	#[test]
	fn ci_value_1_is_ci() {
		assert!(is_ci_with(env_from(&[("CI", "1")])));
	}

	#[test]
	fn github_actions_is_ci() {
		assert!(is_ci_with(env_from(&[("GITHUB_ACTIONS", "true")])));
	}

	#[test]
	fn gitlab_ci_is_ci() {
		assert!(is_ci_with(env_from(&[("GITLAB_CI", "true")])));
	}

	#[test]
	fn jenkins_is_ci() {
		assert!(is_ci_with(env_from(&[("JENKINS_URL", "http://jenkins.example/")])));
	}

	#[test]
	fn empty_ci_var_is_not_ci() {
		assert!(!is_ci_with(env_from(&[("CI", "")])));
	}

	#[test]
	fn unrelated_env_var_is_not_ci() {
		assert!(!is_ci_with(env_from(&[("UNRELATED", "value")])));
	}
}
