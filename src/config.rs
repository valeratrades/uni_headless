use v_utils::macros::{MyConfigPrimitives, Settings};

#[derive(Clone, Debug, MyConfigPrimitives, Settings)]
pub struct AppConfig {
	pub username: String,
	pub password: String,
	/// Auto-submit all LLM answers without confirmation
	#[serde(default)]
	pub auto_submit: bool,
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

impl Default for AppConfig {
	fn default() -> Self {
		Self {
			username: String::new(),
			password: String::new(),
			auto_submit: false,
		}
	}
}
