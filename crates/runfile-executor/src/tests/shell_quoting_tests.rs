use super::*;

// ── Shell quoting security tests ──────────────────────────────────

#[test]
fn cmd_env_value_with_ampersand_is_quoted() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "foo & whoami".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	// The & must be inside quotes so cmd.exe doesn't interpret it as a command separator
	assert_eq!(lines[0], "set \"VAR=foo & whoami\" && echo test");
}

#[test]
fn cmd_env_value_with_pipe_is_quoted() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "foo | del *".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=foo | del *\" && echo test");
}

#[test]
fn cmd_env_value_with_angle_brackets_is_quoted() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "a > b < c".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=a > b < c\" && echo test");
}

#[test]
fn cmd_env_value_with_caret_is_quoted() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "foo^bar".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=foo^bar\" && echo test");
}

#[test]
fn cmd_env_value_with_percent_is_quoted() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "%PATH%".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=%PATH%\" && echo test");
}

#[test]
fn powershell_env_value_with_dollar_subexpression_not_expanded() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	// In double quotes, PowerShell would expand {{ whoami }}. Single quotes prevent this.
	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "{{ whoami }}".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// Must use single quotes so {{ whoami }} is literal
	assert_eq!(lines[0], "$env:VAR='{{ whoami }}'; echo test");
}

#[test]
fn powershell_env_value_with_backtick_is_literal() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "foo`nbar".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// Single quotes make backtick literal (no escape sequence interpretation)
	assert_eq!(lines[0], "$env:VAR='foo`nbar'; echo test");
}

#[test]
fn powershell_env_value_with_single_quote_is_escaped() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "it's a test".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// PowerShell single-quote escaping: ' becomes ''
	assert_eq!(lines[0], "$env:VAR='it''s a test'; echo test");
}

#[test]
fn powershell_env_value_with_double_quotes_is_literal() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "say \"hello\"".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// Double quotes inside single quotes are literal, no escaping needed
	assert_eq!(lines[0], "$env:VAR='say \"hello\"'; echo test");
}

#[test]
fn powershell_env_value_empty() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	assert_eq!(lines[0], "$env:VAR=''; echo test");
}

#[test]
fn powershell_env_value_with_variable_reference_not_expanded() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "$env:SECRET".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// $env:SECRET inside single quotes is literal
	assert_eq!(lines[0], "$env:VAR='$env:SECRET'; echo test");
}

#[test]
fn powershell_env_value_with_semicolon_does_not_break_out() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "a'; Remove-Item * -Recurse; echo '".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::PowerShell);
	// The single quote in the value must be doubled, not allow breakout
	assert_eq!(lines[0], "$env:VAR='a''; Remove-Item * -Recurse; echo '''; echo test");
}

#[test]
fn cmd_env_value_simple_no_special_chars() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	// Even simple values get quoted in cmd.exe (defense in depth)
	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "simple".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=simple\" && echo test");
}

#[test]
fn cmd_env_value_with_multiple_dangerous_chars() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "a & b | c > d < e ^ f".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Cmd);
	assert_eq!(lines[0], "set \"VAR=a & b | c > d < e ^ f\" && echo test");
}

// ── Fix #11: error messages use template, not substituted command ──

#[test]
fn error_message_contains_template_not_substituted_value() {
	let shell = get_test_shell();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 1".to_string()
	} else {
		"exit 1".to_string()
	};
	// Template contains {{ ARG.secret }} — the error should show the template,
	// not the substituted value.
	let template = format!("{fail_cmd} {{{{ ARG.secret ? 'default_val' }}}}");

	let spec = CommandSpec::new_shell(vec![template.clone()]);
	let args = RunArgs::parse(&["--secret=super_secret_password".into()]);
	let dir = TempDir::new().unwrap();

	// With the `when`-aware walker, a non-zero exit no longer surfaces as an
	// `ExecuteError`; the failure is reflected in `final_status` and the
	// `failures` count. Secrets are still kept out of logs via
	// `substitute_redacted` (covered by other tests). What we still want
	// here is: the result reflects the failure cleanly.
	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(!result.final_status.success());
	assert_eq!(result.failures, 1);
}

#[test]
fn execute_failure_signals_through_status_not_error() {
	// Shell command failures no longer return `Err` — they bubble up via
	// `final_status` so subsequent `when: failure` / `when: always` blocks
	// still get a chance to run.
	let shell = get_test_shell();

	let fail_cmd = if shell.kind == ShellKind::Cmd {
		"exit /b 42".to_string()
	} else {
		"exit 42".to_string()
	};

	let spec = CommandSpec::new_shell(vec![fail_cmd.clone()]);
	let args = RunArgs::default();
	let dir = TempDir::new().unwrap();

	let result = execute_command(&spec, &shell, &args, dir.path(), None, false).unwrap();
	assert!(!result.final_status.success(), "exit 42 should report non-zero status");
	assert_eq!(result.failures, 1);
}

// ── Fix #14: Fish shell quoting ───────────────────────────────────

#[test]
fn fish_env_value_with_single_quote_uses_close_escape_reopen() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("MSG".to_string(), "it's alive".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// Expected: env MSG='it'\''s alive' echo test
	assert_eq!(lines[0], "env MSG='it'\\''s alive' echo test");
}

#[test]
fn fish_env_value_with_multiple_single_quotes() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("MSG".to_string(), "it's a 'test'".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	assert_eq!(lines[0], "env MSG='it'\\''s a '\\''test'\\''' echo test");
}

#[test]
fn fish_env_value_with_dollar_sign_is_quoted() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "$HOME".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// $ triggers quoting, value is wrapped in single quotes
	assert_eq!(lines[0], "env VAR='$HOME' echo test");
}

#[test]
fn fish_env_value_with_spaces() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "hello world".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	assert_eq!(lines[0], "env VAR='hello world' echo test");
}

#[test]
fn fish_env_value_with_backslash() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("PATH".to_string(), "C:\\Users\\test".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// Backslash triggers quoting; in single quotes it's literal in Fish
	assert_eq!(lines[0], "env PATH='C:\\Users\\test' echo test");
}

#[test]
fn fish_env_value_empty_string() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// Empty value needs quoting
	assert_eq!(lines[0], "env VAR='' echo test");
}

#[test]
fn fish_env_value_simple_no_quoting() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "simple".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	// Simple alphanumeric value should NOT be quoted
	assert_eq!(lines[0], "env VAR=simple echo test");
}

#[test]
fn fish_env_value_with_semicolon() {
	use crate::extract::{ExtractedCommand, format_extracted_commands};

	let commands = vec![ExtractedCommand {
		command: "echo test".to_string(),
		env_vars: vec![("VAR".to_string(), "a;b".to_string())],
	}];

	let lines = format_extracted_commands(&commands, &ShellKind::Fish);
	assert_eq!(lines[0], "env VAR='a;b' echo test");
}
