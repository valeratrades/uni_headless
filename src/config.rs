use v_utils::macros::{MyConfigPrimitives, Settings};

#[derive(Clone, Debug, MyConfigPrimitives, Settings)]
pub struct AppConfig {
	pub username: String,
	pub password: String,
}

impl Default for AppConfig {
	fn default() -> Self {
		Self {
			username: String::new(),
			password: String::new(),
		}
	}
}
