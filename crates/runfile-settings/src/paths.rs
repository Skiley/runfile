use std::path::PathBuf;

/// Environment variable that overrides the settings directory. When set to a
/// non-empty value it points **directly** at the directory holding
/// `settings.json` (no `runfile` subfolder is appended), taking precedence over
/// the platform default.
///
/// This is the reliable way to redirect settings for tests and portable/CI
/// installs. It matters most on Windows, where [`dirs::config_dir`] resolves via
/// the Known Folder API (`FOLDERID_RoamingAppData`) and ignores the `%APPDATA%`
/// environment variable — so pointing `%APPDATA%` at a scratch dir does *not*
/// isolate the settings, but this variable does.
pub const CONFIG_DIR_ENV_VAR: &str = "RUNFILE_CONFIG_DIR";

/// Get the platform-appropriate settings directory for Runfile.
///
/// - [`CONFIG_DIR_ENV_VAR`], when set to a non-empty value (used verbatim)
/// - Linux/macOS: `~/.config/runfile/`
/// - Windows: `%APPDATA%\runfile\`
pub fn settings_dir() -> Option<PathBuf> {
	if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV_VAR)
		&& !dir.is_empty()
	{
		return Some(PathBuf::from(dir));
	}
	dirs::config_dir().map(|d| d.join("runfile"))
}

/// Get the full path to the settings file.
pub fn settings_file_path() -> Option<PathBuf> {
	settings_dir().map(|d| d.join("settings.json"))
}
