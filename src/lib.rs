use std::fmt;

use serde::{Deserialize, Serialize};

pub mod config;

/// Represents a choice/option in a question
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
	/// Single choice question with radio buttons (one answer)
	SingleChoice {
		/// The question text/prompt
		question_text: String,
		/// Available choices
		choices: Vec<Choice>,
	},
	/// Multiple choice question with checkboxes (multiple answers)
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
			Question::SingleChoice { question_text, .. } | Question::MultiChoice { question_text, .. } => question_text,
		}
	}

	/// Get choices for this question
	pub fn choices(&self) -> &[Choice] {
		match self {
			Question::SingleChoice { choices, .. } | Question::MultiChoice { choices, .. } => choices,
		}
	}

	/// Returns true if this is a multi-choice (checkbox) question
	pub fn is_multi(&self) -> bool {
		matches!(self, Question::MultiChoice { .. })
	}
}

impl fmt::Display for Question {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let (question_text, choices, marker) = match self {
			Question::SingleChoice { question_text, choices } => (question_text, choices, "( )"),
			Question::MultiChoice { question_text, choices } => (question_text, choices, "[ ]"),
		};

		writeln!(f, "{}", question_text)?;
		writeln!(f)?;
		for (i, choice) in choices.iter().enumerate() {
			writeln!(f, "{} {}. {}", marker, i + 1, choice.text)?;
		}
		Ok(())
	}
}
