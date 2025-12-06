use ask_llm::{Client as LlmClient, Conversation, Model, Role};
use chromiumoxide::Page;
use color_eyre::{
	Result,
	eyre::{bail, eyre},
};

use crate::{Blank, Question};

/// LLM response for single-choice questions
#[derive(Debug, serde::Deserialize)]
struct LlmSingleAnswer {
	response: String,
	response_number: usize,
}

/// LLM response for multi-choice questions
#[derive(Debug, serde::Deserialize)]
struct LlmMultiAnswer {
	responses: Vec<String>,
	response_numbers: Vec<usize>,
}

/// LLM response for short answer questions
#[derive(Debug, serde::Deserialize)]
struct LlmTextAnswer {
	answer: String,
}

/// LLM response for matching questions
#[derive(Debug, serde::Deserialize)]
struct LlmMatchingAnswer {
	matches: Vec<LlmMatchPair>,
}

#[derive(Debug, serde::Deserialize)]
struct LlmMatchPair {
	prompt: String,
	answer: String,
}

/// LLM response for fill-in-the-blanks questions
#[derive(Debug, serde::Deserialize)]
struct LlmFillInBlanksAnswer {
	blanks: Vec<LlmBlankAnswer>,
}

#[derive(Debug, serde::Deserialize)]
struct LlmBlankAnswer {
	/// The blank number (1-indexed as shown to the LLM)
	blank_number: usize,
	/// The answer (text for text inputs, selected option text for dropdowns)
	answer: String,
}

/// Result of LLM answering a question
pub enum LlmAnswerResult {
	Single {
		idx: usize,
		text: String,
	},
	Multi {
		indices: Vec<usize>,
		texts: Vec<String>,
	},
	Text {
		answer: String,
	},
	/// Matching: vector of (select_name, value_to_select)
	Matching {
		selections: Vec<(String, String)>,
	},
	/// FillInBlanks: vector of (blank_index, answer) where answer is either:
	/// - For text blanks: the text to input
	/// - For select blanks: (select_name, value_to_select)
	FillInBlanks {
		answers: Vec<FillInBlanksAnswerItem>,
	},
}

/// An answer for a single blank in a FillInBlanks question
pub enum FillInBlanksAnswerItem {
	/// Text input answer
	Text { input_name: String, answer: String },
	/// Select/dropdown answer
	Select { select_name: String, value: String },
}

/// LLM response for code submission questions
#[derive(Debug, serde::Deserialize)]
struct LlmCodeAnswer {
	files: Vec<LlmCodeFile>,
}

#[derive(Debug, serde::Deserialize)]
struct LlmCodeFile {
	filename: String,
	content: String,
}

/// Fetch an image via the browser and return its base64 data and media type
async fn fetch_image_as_base64(page: &Page, url: &str) -> Result<(String, String)> {
	let fetch_script = format!(
		r#"
		(async function() {{
			try {{
				const response = await fetch("{}");
				if (!response.ok) return null;
				const blob = await response.blob();
				const mediaType = blob.type || 'image/png';
				return new Promise((resolve) => {{
					const reader = new FileReader();
					reader.onloadend = () => {{
						const base64 = reader.result.split(',')[1];
						resolve(JSON.stringify({{base64: base64, mediaType: mediaType}}));
					}};
					reader.readAsDataURL(blob);
				}});
			}} catch (e) {{
				return null;
			}}
		}})()
		"#,
		url
	);

	let result = page.evaluate(fetch_script).await.map_err(|e| eyre!("Failed to fetch image: {}", e))?;

	let json_str = result.value().and_then(|v| v.as_str()).ok_or_else(|| eyre!("Failed to fetch image: browser returned null"))?;

	let parsed: serde_json::Value = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse image data: {}", e))?;

	let base64 = parsed["base64"].as_str().ok_or_else(|| eyre!("Missing base64 data"))?.to_string();
	let media_type = parsed["mediaType"].as_str().unwrap_or("image/png").to_string();

	Ok((base64, media_type))
}

/// Ask the LLM to answer a quiz question (multiple-choice or short answer)
pub async fn ask_llm_for_answer(page: &Page, question: &Question) -> Result<LlmAnswerResult> {
	let question_display = question.to_string();

	// Handle short answer questions
	if question.is_short_answer() {
		let prompt = format!(
			r#"You are answering a short answer question. Provide a concise, direct answer.

{question_display}
Respond with JSON only, no markdown, in this exact format:
{{"answer": "<your concise answer>"}}"#
		);

		let mut client = LlmClient::new().model(Model::Medium).max_tokens(128).force_json();

		// Attach question images
		for img in question.images() {
			match fetch_image_as_base64(page, &img.url).await {
				Ok((base64, media_type)) => {
					client = client.append_file(base64, media_type);
				}
				Err(e) => {
					tracing::warn!("Failed to fetch image for LLM: {}", e);
				}
			}
		}

		let mut conv = Conversation::new();
		conv.add(Role::User, prompt);

		let response = client.conversation(&conv).await?;
		tracing::debug!("LLM raw response: {}", response.text);

		let json_str = response.text.trim();
		let answer: LlmTextAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM JSON response: {} - raw: '{}'", e, json_str))?;

		return Ok(LlmAnswerResult::Text { answer: answer.answer });
	}

	// Handle matching questions
	if question.is_matching() {
		let items = question.match_items();

		let prompt = format!(
			r#"You are answering a matching question. For each item, select the correct option from its available choices.

{question_display}
Respond with JSON only, no markdown, in this exact format:
{{"matches": [{{"prompt": "<item prompt text or slot number like '[1]'>", "answer": "<chosen option text>"}}]}}"#
		);

		let mut client = LlmClient::new().model(Model::Medium).max_tokens(512).force_json();

		// Attach question images
		for img in question.images() {
			match fetch_image_as_base64(page, &img.url).await {
				Ok((base64, media_type)) => {
					client = client.append_file(base64, media_type);
				}
				Err(e) => {
					tracing::warn!("Failed to fetch image for LLM: {}", e);
				}
			}
		}

		let mut conv = Conversation::new();
		conv.add(Role::User, prompt);

		let response = client.conversation(&conv).await?;
		tracing::debug!("LLM raw response: {}", response.text);

		let json_str = response.text.trim();
		let answer: LlmMatchingAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM JSON response: {} - raw: '{}'", e, json_str))?;

		// Convert LLM answer to selections (select_name, value)
		let mut selections = Vec::new();
		for match_pair in answer.matches {
			// Find the item that matches this prompt
			// For inline selects, the prompt might be a slot number like "[1]"
			for (i, item) in items.iter().enumerate() {
				let slot_format = format!("[{}]", i + 1);
				let matches_prompt = if item.prompt.is_empty() {
					// For inline selects, check if LLM returned the slot number
					match_pair.prompt == slot_format || match_pair.prompt == (i + 1).to_string()
				} else {
					item.prompt.contains(&match_pair.prompt) || match_pair.prompt.contains(&item.prompt)
				};

				if matches_prompt {
					// Find the option value for the answer text
					for opt in &item.options {
						if opt.text == match_pair.answer {
							selections.push((item.select_name.clone(), opt.value.clone()));
							break;
						}
					}
					break;
				}
			}
		}

		return Ok(LlmAnswerResult::Matching { selections });
	}

	// Handle fill-in-the-blanks questions
	if question.is_fill_in_blanks() {
		let fill = question.fill_in_blanks().unwrap();

		let prompt = format!(
			r#"You are answering a fill-in-the-blanks question. Fill in each numbered blank with the correct answer.

{question_display}
Respond with JSON only, no markdown, in this exact format:
{{"blanks": [{{"blank_number": <number>, "answer": "<the answer for this blank>"}}]}}

For text input blanks, provide the exact text to enter.
For dropdown blanks, provide the exact text of the option to select (one of the listed choices)."#
		);

		let mut client = LlmClient::new().model(Model::Medium).max_tokens(1024).force_json();

		// Attach question images
		for img in question.images() {
			match fetch_image_as_base64(page, &img.url).await {
				Ok((base64, media_type)) => {
					client = client.append_file(base64, media_type);
				}
				Err(e) => {
					tracing::warn!("Failed to fetch image for LLM: {}", e);
				}
			}
		}

		let mut conv = Conversation::new();
		conv.add(Role::User, prompt);

		let response = client.conversation(&conv).await?;
		tracing::debug!("LLM raw response: {}", response.text);

		let json_str = response.text.trim();
		let answer: LlmFillInBlanksAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM JSON response: {} - raw: '{}'", e, json_str))?;

		// Convert LLM answer to FillInBlanksAnswerItem
		let mut answers = Vec::new();
		for blank_answer in answer.blanks {
			let blank_idx = blank_answer.blank_number.saturating_sub(1); // Convert 1-indexed to 0-indexed
			if blank_idx >= fill.blanks.len() {
				tracing::warn!("LLM returned invalid blank number: {} (max: {})", blank_answer.blank_number, fill.blanks.len());
				continue;
			}

			let blank = &fill.blanks[blank_idx];
			match blank {
				Blank::Text { input_name, .. } => {
					answers.push(FillInBlanksAnswerItem::Text {
						input_name: input_name.clone(),
						answer: blank_answer.answer,
					});
				}
				Blank::Select { select_name, options, .. } => {
					// Find the option value for the answer text
					if let Some(opt) = options.iter().find(|o| o.text == blank_answer.answer) {
						answers.push(FillInBlanksAnswerItem::Select {
							select_name: select_name.clone(),
							value: opt.value.clone(),
						});
					} else {
						tracing::warn!("LLM returned unknown option '{}' for blank {}", blank_answer.answer, blank_answer.blank_number);
					}
				}
			}
		}

		return Ok(LlmAnswerResult::FillInBlanks { answers });
	}

	// Handle multiple-choice questions
	let choices = question.choices();
	let (prompt, max_tokens) = if question.is_multi() {
		(
			format!(
				r#"You are answering a multiple-choice question where MULTIPLE answers may be correct. Select ALL correct answers.

{question_display}
Respond with JSON only, no markdown, in this exact format:
{{"responses": ["<text of first correct answer>", "<text of second correct answer>", ...], "response_numbers": [<number of first correct answer>, <number of second correct answer>, ...]}}"#
			),
			256,
		)
	} else {
		(
			format!(
				r#"You are answering a single-choice question. Pick the ONE correct answer.

{question_display}
Respond with JSON only, no markdown, in this exact format:
{{"response": "<the text of the correct answer>", "response_number": <the number of the correct answer>}}"#
			),
			128,
		)
	};

	// Build client and attach images
	let mut client = LlmClient::new().model(Model::Medium).max_tokens(max_tokens).force_json();

	// Attach question images
	for img in question.images() {
		match fetch_image_as_base64(page, &img.url).await {
			Ok((base64, media_type)) => {
				client = client.append_file(base64, media_type);
			}
			Err(e) => {
				tracing::warn!("Failed to fetch image for LLM: {}", e);
			}
		}
	}

	// Attach choice images
	for choice in choices {
		for img in &choice.images {
			match fetch_image_as_base64(page, &img.url).await {
				Ok((base64, media_type)) => {
					client = client.append_file(base64, media_type);
				}
				Err(e) => {
					tracing::warn!("Failed to fetch choice image for LLM: {}", e);
				}
			}
		}
	}

	let mut conv = Conversation::new();
	conv.add(Role::User, prompt);

	let response = client.conversation(&conv).await?;

	tracing::debug!("LLM raw response: {}", response.text);

	let json_str = response.text.trim();

	if question.is_multi() {
		let answer: LlmMultiAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM JSON response: {} - raw: '{}'", e, json_str))?;

		// Validate all indices
		for &num in &answer.response_numbers {
			if num == 0 || num > choices.len() {
				return Err(eyre!("LLM returned invalid answer index: {} (expected 1-{})", num, choices.len()));
			}
		}

		let indices: Vec<usize> = answer.response_numbers.iter().map(|n| n - 1).collect();
		Ok(LlmAnswerResult::Multi { indices, texts: answer.responses })
	} else {
		let answer: LlmSingleAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM JSON response: {} - raw: '{}'", e, json_str))?;

		if answer.response_number == 0 || answer.response_number > choices.len() {
			return Err(eyre!("LLM returned invalid answer index: {} (expected 1-{})", answer.response_number, choices.len()));
		}

		Ok(LlmAnswerResult::Single {
			idx: answer.response_number - 1,
			text: answer.response,
		})
	}
}

/// Result of asking LLM for code - includes conversation for potential retries
pub struct LlmCodeResult {
	/// Generated files (filename -> content)
	pub files: Vec<(String, String)>,
	/// The conversation history (for retries with test results)
	pub conversation: Conversation,
}

/// Ask the LLM to generate code for a VPL submission
pub async fn ask_llm_for_code(question: &Question) -> Result<LlmCodeResult> {
	let Question::CodeSubmission { description, required_files, .. } = question else {
		bail!("Expected CodeSubmission question");
	};

	let files_list = if required_files.is_empty() {
		"No specific files required - determine appropriate filename(s) based on the problem.".to_string()
	} else {
		required_files
			.iter()
			.map(|f| {
				if f.content.is_empty() {
					format!("- {}", f.name)
				} else {
					format!("- {} (template provided):\n```\n{}\n```", f.name, f.content)
				}
			})
			.collect::<Vec<_>>()
			.join("\n")
	};

	let prompt = format!(
		r#"You are solving a programming assignment. Write the complete solution code.

Problem Description:
{description}

Required Files:
{files_list}

IMPORTANT: Respond with JSON only, no markdown, in this exact format:
{{"files": [{{"filename": "<filename>", "content": "<complete file content>"}}]}}

Make sure the code is correct and ready to submit. Do not include docstrings or comments."#
	);

	let mut conv = Conversation::new();
	conv.add(Role::User, prompt);

	let client = LlmClient::new().model(Model::Medium).max_tokens(4096).force_json();

	let response = client.conversation(&conv).await?;

	tracing::debug!("LLM code response: {}", response.text);

	// Add assistant response to conversation for potential retries
	conv.add(Role::Assistant, &response.text);

	let json_str = response.text.trim();
	let answer: LlmCodeAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM code response: {e} - raw: '{json_str}'"))?;

	let files = answer.files.into_iter().map(|f| (f.filename, f.content)).collect();
	Ok(LlmCodeResult { files, conversation: conv })
}

/// Retry code generation with test results feedback
pub async fn retry_llm_with_test_results(mut conversation: Conversation, test_results: &str) -> Result<LlmCodeResult> {
	// Add test results as a new user message (no additional commentary)
	conversation.add(Role::User, test_results);

	let client = LlmClient::new().model(Model::Medium).max_tokens(4096).force_json();

	let response = client.conversation(&conversation).await?;

	tracing::debug!("LLM retry response: {}", response.text);

	// Add assistant response to conversation
	conversation.add(Role::Assistant, &response.text);

	let json_str = response.text.trim();
	let answer: LlmCodeAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM retry response: {e} - raw: '{json_str}'"))?;

	let files = answer.files.into_iter().map(|f| (f.filename, f.content)).collect();
	Ok(LlmCodeResult { files, conversation })
}
