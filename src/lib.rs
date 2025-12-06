use std::fmt;

use serde::{Deserialize, Serialize};

pub mod config;
pub mod llm;
pub mod login;
pub mod runner;

/// Detects if a URL is a VPL (Virtual Programming Lab) activity
pub fn is_vpl_url(url: &str) -> bool {
	url.contains("/mod/vpl/")
}

/// Represents an image in a question
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Image {
	/// The URL of the image
	pub url: String,
	/// Alt text if available
	pub alt: Option<String>,
}

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
	/// Images in this choice (if any)
	#[serde(default)]
	pub images: Vec<Image>,
}

/// Represents a required file for code submission
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RequiredFile {
	/// The filename (e.g., "main.c", "solution.py")
	pub name: String,
	/// Initial content if provided (template code)
	#[serde(default)]
	pub content: String,
}

/// Represents a single dropdown in a matching question
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MatchItem {
	/// The prompt text for this item (what to match)
	pub prompt: String,
	/// The select element's name attribute (for form submission)
	pub select_name: String,
	/// Available options in the dropdown
	pub options: Vec<MatchOption>,
	/// Currently selected value (0 = none selected)
	pub selected_value: String,
}

/// An option in a matching dropdown
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MatchOption {
	/// The value attribute
	pub value: String,
	/// The display text
	pub text: String,
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
		/// Images in the question (not in choices)
		#[serde(default)]
		images: Vec<Image>,
	},
	/// Multiple choice question with checkboxes (multiple answers)
	MultiChoice {
		/// The question text/prompt
		question_text: String,
		/// Available choices
		choices: Vec<Choice>,
		/// Images in the question (not in choices)
		#[serde(default)]
		images: Vec<Image>,
	},
	/// Short answer / text response question (free text input)
	ShortAnswer {
		/// The question text/prompt
		question_text: String,
		/// The input element's name attribute (for form submission)
		input_name: String,
		/// Current answer value (if any)
		current_answer: String,
		/// Images in the question
		#[serde(default)]
		images: Vec<Image>,
	},
	/// Matching question with multiple dropdowns
	Matching {
		/// The question text/prompt
		question_text: String,
		/// Items to match (each has a prompt and dropdown)
		items: Vec<MatchItem>,
		/// Images in the question
		#[serde(default)]
		images: Vec<Image>,
	},
	/// Code submission (VPL - Virtual Programming Lab)
	CodeSubmission {
		/// The problem description/statement
		description: String,
		/// Files that must be submitted
		required_files: Vec<RequiredFile>,
		/// The course module ID (for API submission)
		module_id: String,
		/// Images in the description
		#[serde(default)]
		images: Vec<Image>,
	},
}

impl Question {
	/// Extract question text for display
	pub fn question_text(&self) -> &str {
		match self {
			Question::SingleChoice { question_text, .. }
			| Question::MultiChoice { question_text, .. }
			| Question::ShortAnswer { question_text, .. }
			| Question::Matching { question_text, .. } => question_text,
			Question::CodeSubmission { description, .. } => description,
		}
	}

	/// Get choices for this question (empty for CodeSubmission, ShortAnswer, and Matching)
	pub fn choices(&self) -> &[Choice] {
		match self {
			Question::SingleChoice { choices, .. } | Question::MultiChoice { choices, .. } => choices,
			Question::CodeSubmission { .. } | Question::ShortAnswer { .. } | Question::Matching { .. } => &[],
		}
	}

	/// Get images in the question text (not in choices)
	pub fn images(&self) -> &[Image] {
		match self {
			Question::SingleChoice { images, .. }
			| Question::MultiChoice { images, .. }
			| Question::ShortAnswer { images, .. }
			| Question::Matching { images, .. }
			| Question::CodeSubmission { images, .. } => images,
		}
	}

	/// Returns true if this is a multi-choice (checkbox) question
	pub fn is_multi(&self) -> bool {
		matches!(self, Question::MultiChoice { .. })
	}

	/// Returns true if this is a code submission question
	pub fn is_code_submission(&self) -> bool {
		matches!(self, Question::CodeSubmission { .. })
	}

	/// Returns true if this is a short answer (text response) question
	pub fn is_short_answer(&self) -> bool {
		matches!(self, Question::ShortAnswer { .. })
	}

	/// Get the input name for short answer questions
	pub fn short_answer_input_name(&self) -> Option<&str> {
		match self {
			Question::ShortAnswer { input_name, .. } => Some(input_name),
			_ => None,
		}
	}

	/// Returns true if this is a matching question
	pub fn is_matching(&self) -> bool {
		matches!(self, Question::Matching { .. })
	}

	/// Get match items for matching questions
	pub fn match_items(&self) -> &[MatchItem] {
		match self {
			Question::Matching { items, .. } => items,
			_ => &[],
		}
	}

	/// Get required files for code submission
	pub fn required_files(&self) -> &[RequiredFile] {
		match self {
			Question::CodeSubmission { required_files, .. } => required_files,
			_ => &[],
		}
	}

	/// Get module ID for code submission
	pub fn module_id(&self) -> Option<&str> {
		match self {
			Question::CodeSubmission { module_id, .. } => Some(module_id),
			_ => None,
		}
	}
}

impl fmt::Display for Question {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Question::SingleChoice { question_text, choices, .. } => {
				writeln!(f, "{}", question_text)?;
				writeln!(f)?;
				for (i, choice) in choices.iter().enumerate() {
					writeln!(f, "( ) {}. {}", i + 1, choice.text)?;
				}
			}
			Question::MultiChoice { question_text, choices, .. } => {
				writeln!(f, "{}", question_text)?;
				writeln!(f)?;
				for (i, choice) in choices.iter().enumerate() {
					writeln!(f, "[ ] {}. {}", i + 1, choice.text)?;
				}
			}
			Question::ShortAnswer { question_text, current_answer, .. } => {
				writeln!(f, "{}", question_text)?;
				writeln!(f)?;
				if current_answer.is_empty() {
					writeln!(f, "[____________________]")?;
				} else {
					writeln!(f, "[{}]", current_answer)?;
				}
			}
			Question::Matching { question_text, items, .. } => {
				writeln!(f, "{}", question_text)?;
				writeln!(f)?;
				for item in items {
					let selected = item.options.iter().find(|o| o.value == item.selected_value).map(|o| o.text.as_str()).unwrap_or("___");
					// Show available options for this item (excluding empty placeholder)
					let available: Vec<&str> = item.options.iter().filter(|o| !o.value.is_empty() && o.value != "0").map(|o| o.text.as_str()).collect();
					if available.is_empty() {
						writeln!(f, "  {} -> [{}]", item.prompt, selected)?;
					} else {
						writeln!(f, "  {} -> [{}]  (options: {})", item.prompt, selected, available.join(", "))?;
					}
				}
			}
			Question::CodeSubmission { description, required_files, .. } => {
				writeln!(f, "{}", description)?;
				if !required_files.is_empty() {
					writeln!(f)?;
					writeln!(f, "Required files:")?;
					for file in required_files {
						if file.content.is_empty() {
							writeln!(f, "  - {}", file.name)?;
						} else {
							writeln!(f, "  - {} (has template)", file.name)?;
						}
					}
				}
			}
		}
		Ok(())
	}
}
