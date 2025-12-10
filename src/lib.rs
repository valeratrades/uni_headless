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

/// A drop zone in a DragDropIntoText question
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DropZone {
	/// The hidden input name (e.g., "q202791:5_p1")
	pub input_name: String,
	/// Which place number this is (1-indexed)
	pub place_number: usize,
	/// Currently selected choice (0 = none)
	pub current_choice: usize,
}

/// A draggable choice in a DragDropIntoText question
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DragChoice {
	/// The choice number (1-indexed, used as value in hidden inputs)
	pub choice_number: usize,
	/// The text label
	pub text: String,
}

/// A DragDropIntoText question (qtype_ddwtos)
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DragDropIntoText {
	/// The question prompt with drop zones indicated
	pub question_text: String,
	/// Available choices to drag
	pub choices: Vec<DragChoice>,
	/// Drop zones where choices can be placed
	pub drop_zones: Vec<DropZone>,
	/// Images in the question
	#[serde(default)]
	pub images: Vec<Image>,
}

impl fmt::Display for DragDropIntoText {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		writeln!(f, "{}", self.question_text)?;
		writeln!(f)?;
		writeln!(f, "Drag choices:")?;
		for choice in &self.choices {
			writeln!(f, "  {}. {}", choice.choice_number, choice.text)?;
		}
		writeln!(f)?;
		writeln!(f, "Drop zones: {} places to fill", self.drop_zones.len())?;
		Ok(())
	}
}

/// A blank (input field) within a FillInBlanks question
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Blank {
	/// A text input field (like ShortAnswer)
	Text {
		/// The input element's name attribute
		input_name: String,
		/// Current value (if any)
		current_value: String,
	},
	/// A dropdown select (like Match)
	Select {
		/// The select element's name attribute
		select_name: String,
		/// Available options
		options: Vec<MatchOption>,
		/// Currently selected value
		selected_value: String,
	},
}

impl fmt::Display for Blank {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Blank::Text { current_value, .. } =>
				if current_value.is_empty() {
					write!(f, "[___]")
				} else {
					write!(f, "[{}]", current_value)
				},
			Blank::Select { options, .. } => {
				let available: Vec<&str> = options.iter().filter(|o| !o.value.is_empty()).map(|o| o.text.as_str()).collect();
				write!(f, "[select from: {}]", available.join(" | "))
			}
		}
	}
}

/// A segment of text in a FillInBlanks question
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum FillSegment {
	/// Plain text
	Text(String),
	/// A blank with its index (0-based)
	Blank(usize),
}

/// A fill-in-the-blanks question with text and embedded inputs
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FillInBlanks {
	/// The question prompt/header text
	pub question_text: String,
	/// Segments of text and blanks in order
	pub segments: Vec<FillSegment>,
	/// All blanks (referenced by index in segments)
	pub blanks: Vec<Blank>,
	/// Images in the question
	#[serde(default)]
	pub images: Vec<Image>,
}

impl fmt::Display for FillInBlanks {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// First, show the question text if present
		if !self.question_text.is_empty() {
			writeln!(f, "{}", self.question_text)?;
			writeln!(f)?;
		}

		// Show the fill-in text with numbered blanks
		write!(f, "Fill in: ")?;
		for segment in &self.segments {
			match segment {
				FillSegment::Text(text) => write!(f, "{}", text)?,
				FillSegment::Blank(idx) => write!(f, "[{}]", idx + 1)?,
			}
		}
		writeln!(f)?;
		writeln!(f)?;

		// Show the blanks with their types and options
		writeln!(f, "Blanks:")?;
		for (i, blank) in self.blanks.iter().enumerate() {
			match blank {
				Blank::Text { .. } => {
					writeln!(f, "  [{}]: text input", i + 1)?;
				}
				Blank::Select { options, .. } => {
					let available: Vec<&str> = options.iter().filter(|o| !o.value.is_empty()).map(|o| o.text.as_str()).collect();
					writeln!(f, "  [{}]: select from: {}", i + 1, available.join(", "))?;
				}
			}
		}

		Ok(())
	}
}

impl fmt::Display for MatchItem {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let available: Vec<&str> = self.options.iter().filter(|o| !o.value.is_empty() && o.value != "0").map(|o| o.text.as_str()).collect();
		if self.prompt.is_empty() {
			write!(f, "[___] -> choose from: {}", available.join(", "))
		} else {
			write!(f, "{} -> choose from: {}", self.prompt, available.join(", "))
		}
	}
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
	/// Fill-in-the-blanks question with embedded text inputs and/or dropdowns
	FillInBlanks(FillInBlanks),
	/// Drag-and-drop into text question (qtype_ddwtos)
	DragDropIntoText(DragDropIntoText),
	/// Code block question (inline code editor in quiz, not full VPL page)
	CodeBlock {
		/// The question text/prompt
		question_text: String,
		/// The textarea's name attribute (for form submission)
		input_name: String,
		/// Programming language (e.g., "python", "c", "java")
		language: String,
		/// Current code content (if any template provided)
		current_code: String,
		/// Images in the question
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
			| Question::Matching { question_text, .. }
			| Question::CodeBlock { question_text, .. } => question_text,
			Question::CodeSubmission { description, .. } => description,
			Question::FillInBlanks(fill) => &fill.question_text,
			Question::DragDropIntoText(ddwtos) => &ddwtos.question_text,
		}
	}

	/// Get choices for this question (empty for CodeSubmission, ShortAnswer, Matching, FillInBlanks, DragDropIntoText, and CodeBlock)
	pub fn choices(&self) -> &[Choice] {
		match self {
			Question::SingleChoice { choices, .. } | Question::MultiChoice { choices, .. } => choices,
			Question::CodeSubmission { .. }
			| Question::ShortAnswer { .. }
			| Question::Matching { .. }
			| Question::FillInBlanks { .. }
			| Question::DragDropIntoText { .. }
			| Question::CodeBlock { .. } => &[],
		}
	}

	/// Get images in the question text (not in choices)
	pub fn images(&self) -> &[Image] {
		match self {
			Question::SingleChoice { images, .. }
			| Question::MultiChoice { images, .. }
			| Question::ShortAnswer { images, .. }
			| Question::Matching { images, .. }
			| Question::CodeSubmission { images, .. }
			| Question::CodeBlock { images, .. } => images,
			Question::FillInBlanks(fill) => &fill.images,
			Question::DragDropIntoText(ddwtos) => &ddwtos.images,
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

	/// Returns true if this is a fill-in-the-blanks question
	pub fn is_fill_in_blanks(&self) -> bool {
		matches!(self, Question::FillInBlanks { .. })
	}

	/// Get fill-in-blanks data for FillInBlanks questions
	pub fn fill_in_blanks(&self) -> Option<&FillInBlanks> {
		match self {
			Question::FillInBlanks(fill) => Some(fill),
			_ => None,
		}
	}

	/// Returns true if this is a code block (inline code editor) question
	pub fn is_code_block(&self) -> bool {
		matches!(self, Question::CodeBlock { .. })
	}

	/// Get the input name for code block questions
	pub fn code_block_input_name(&self) -> Option<&str> {
		match self {
			Question::CodeBlock { input_name, .. } => Some(input_name),
			_ => None,
		}
	}

	/// Get the language for code block questions
	pub fn code_block_language(&self) -> Option<&str> {
		match self {
			Question::CodeBlock { language, .. } => Some(language),
			_ => None,
		}
	}

	/// Returns true if this is a drag-drop-into-text question
	pub fn is_drag_drop_into_text(&self) -> bool {
		matches!(self, Question::DragDropIntoText { .. })
	}

	/// Get drag-drop-into-text data for DragDropIntoText questions
	pub fn drag_drop_into_text(&self) -> Option<&DragDropIntoText> {
		match self {
			Question::DragDropIntoText(ddwtos) => Some(ddwtos),
			_ => None,
		}
	}
}

impl fmt::Display for Question {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Question::SingleChoice { question_text, choices, .. } | Question::MultiChoice { question_text, choices, .. } => {
				writeln!(f, "{}", question_text)?;
				writeln!(f)?;
				for (i, choice) in choices.iter().enumerate() {
					writeln!(f, "{}. {}", i + 1, choice.text)?;
				}
			}
			Question::ShortAnswer { question_text, .. } => {
				writeln!(f, "{}", question_text)?;
			}
			Question::Matching { question_text, items, .. } => {
				writeln!(f, "{}", question_text)?;
				writeln!(f)?;
				for (i, item) in items.iter().enumerate() {
					writeln!(f, "{}. {}", i + 1, item)?;
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
			Question::FillInBlanks(fill) => {
				write!(f, "{}", fill)?;
			}
			Question::DragDropIntoText(ddwtos) => {
				write!(f, "{}", ddwtos)?;
			}
			Question::CodeBlock {
				question_text,
				language,
				current_code,
				..
			} => {
				writeln!(f, "{}", question_text)?;
				writeln!(f)?;
				writeln!(f, "Language: {}", language)?;
				if !current_code.is_empty() {
					writeln!(f, "Template code provided")?;
				}
			}
		}
		Ok(())
	}
}
