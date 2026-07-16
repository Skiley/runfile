use crate::ci_detect::*;

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
