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
