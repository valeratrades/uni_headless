use v_utils::macros::{MyConfigPrimitives, Settings};

#[derive(Clone, Debug, Default, MyConfigPrimitives, Settings)]
pub struct AppConfig {
	pub username: String,
	pub password: String,
	/// Auto-submit all LLM answers without confirmation
	#[serde(default)]
	pub auto_submit: bool,
	/// Auto-click continuation prompts when found (default: false)
	#[serde(default)]
	pub continuation_prompts: bool,
	/// Command to run on completion/error (receives message as argument)
	#[serde(default)]
	pub stop_hook: Option<String>,
	/// Number of retries for transient API errors (500, rate limit, etc) (default: 3)
	#[serde(default = "default_api_retries")]
	pub api_retries: u32,
	/// Base delay in ms between API retries, multiplied by attempt number (default: 1000)
	#[serde(default = "default_api_retry_delay_ms")]
	pub api_retry_delay_ms: u64,
	/// Max consecutive LLM failures before stopping (quiz questions or VPL code retries) (default: 5)
	#[serde(default = "default_max_consecutive_failures")]
	pub max_consecutive_failures: u32,
	/// Number of retries for browser button clicks (default: 5)
	#[serde(default = "default_button_click_retries")]
	pub button_click_retries: u32,
	/// Run with visible browser window (non-headless mode)
	#[serde(default)]
	pub visible: bool,
	/// Allow skipping pages without submitted answers (logs error but continues)
	#[serde(default)]
	pub allow_skip: bool,
}

fn default_api_retries() -> u32 {
	3
}

fn default_api_retry_delay_ms() -> u64 {
	1000
}

fn default_max_consecutive_failures() -> u32 {
	5
}

fn default_button_click_retries() -> u32 {
	5
}

impl AppConfig {
	/// Set auto_submit at runtime
	///
	/// # Safety
	/// Only call from single-threaded context or when no other references are reading this field.
	pub unsafe fn set_auto_submit(&mut self, value: bool) {
		self.auto_submit = value;
	}
}
