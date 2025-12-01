use std::fmt;

use serde::{Deserialize, Serialize};

pub mod config;

/// Represents a choice/option in a multiple-choice question
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Choice {
	/// The input element's name attribute (for form submission)
	pub input_name: String,
	/// The input element's value attribute
	pub input_value: String,
	/// The text label for this choice
	pub text: String,
	/// Whether this choice is currently selected
	pub selected: bool,
}

/// Represents different types of quiz questions
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Question {
	/// Multiple choice question with radio buttons (single answer)
	MultiChoice {
		/// The question text/prompt
		question_text: String,
		/// Available choices
		choices: Vec<Choice>,
	},
}

impl Question {
	/// Extract question text for display
	pub fn question_text(&self) -> &str {
		match self {
			Question::MultiChoice { question_text, .. } => question_text,
		}
	}

	/// Get choices if this is a multi-choice question
	pub fn choices(&self) -> Option<&[Choice]> {
		match self {
			Question::MultiChoice { choices, .. } => Some(choices),
		}
	}
}

impl fmt::Display for Question {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Question::MultiChoice { question_text, choices } => {
				writeln!(f, "{}", question_text)?;
				writeln!(f)?;
				for (i, choice) in choices.iter().enumerate() {
					writeln!(f, "{}. {}", i + 1, choice.text)?;
				}
				Ok(())
			}
		}
	}
}
