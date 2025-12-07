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
	/// Number of retries for LLM code generation when tests fail (default: 5)
	#[serde(default = "default_llm_retries")]
	pub llm_retries: u32,
}

fn default_llm_retries() -> u32 {
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
