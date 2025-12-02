use ask_llm::{Client as LlmClient, Conversation, Model, Role};
use chromiumoxide::Page;
use color_eyre::{
	Result,
	eyre::{bail, eyre},
};

use crate::Question;

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

/// Result of LLM answering a question
pub enum LlmAnswerResult {
	Single { idx: usize, text: String },
	Multi { indices: Vec<usize>, texts: Vec<String> },
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

/// Ask the LLM to answer a multiple-choice question
pub async fn ask_llm_for_answer(page: &Page, question: &Question) -> Result<LlmAnswerResult> {
	let question_text = question.question_text();
	let choices = question.choices();

	let mut options_text = String::new();
	for (i, choice) in choices.iter().enumerate() {
		options_text.push_str(&format!("{}. {}\n", i + 1, choice.text));
	}

	let (prompt, max_tokens) = if question.is_multi() {
		(
			format!(
				r#"You are answering a multiple-choice question where MULTIPLE answers may be correct. Select ALL correct answers.

Question:
{question_text}

Options:
{options_text}
Respond with JSON only, no markdown, in this exact format:
{{"responses": ["<text of first correct answer>", "<text of second correct answer>", ...], "response_numbers": [<number of first correct answer>, <number of second correct answer>, ...]}}"#
			),
			256,
		)
	} else {
		(
			format!(
				r#"You are answering a single-choice question. Pick the ONE correct answer.

Question:
{question_text}

Options:
{options_text}
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

/// Ask the LLM to generate code for a VPL submission
pub async fn ask_llm_for_code(question: &Question) -> Result<Vec<(String, String)>> {
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

	let json_str = response.text.trim();
	let answer: LlmCodeAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM code response: {e} - raw: '{json_str}'"))?;

	Ok(answer.files.into_iter().map(|f| (f.filename, f.content)).collect())
}
