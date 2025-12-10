//! Page execution logic - handles VPL and quiz pages

#[cfg(feature = "xdg")]
use std::path::PathBuf;

use chromiumoxide::Page;
use color_eyre::{
	Result,
	eyre::{bail, eyre},
};
#[cfg(feature = "xdg")]
use v_utils::xdg_state_dir;
use v_utils::{Percent, elog, io::confirm, log};

use crate::{
	Blank, Choice, FillInBlanks, FillSegment, Image, MatchItem, MatchOption, Question, RequiredFile,
	config::AppConfig,
	llm::{FillInBlanksAnswerItem, LlmAnswerResult, ask_llm_for_answer, ask_llm_for_code, retry_llm_with_test_results},
};

/// Run the stop hook with a message if configured
fn run_stop_hook(config: &AppConfig, message: &str) {
	if let Some(ref hook) = config.stop_hook {
		log!("Running stop hook: {} {:?}", hook, message);
		// Escape single quotes for shell: replace ' with '\''
		let escaped = message.replace('\'', "'\\''");
		let _ = tokio::process::Command::new("sh").arg("-c").arg(format!("{} '{}'", hook, escaped)).spawn();
	}
}

/// Handle a VPL (Virtual Programming Lab) code submission page
/// Returns true if got perfect grade (100%)
pub async fn handle_vpl_page(page: &Page, ask_llm: bool, config: &mut AppConfig, session_id: &str) -> Result<bool> {
	let question = parse_vpl_page(page).await?;

	let Some(question) = question else {
		log!("No VPL question found on this page.");
		return Ok(false);
	};

	// Display the question
	let header = "--- Code Submission [VPL] ---";
	tracing::info!("{}", header);
	eprintln!("{}", header);

	let text = question.question_text();
	tracing::info!("{}", text);
	eprintln!("{}", text);

	// Display images
	for img in question.images() {
		if let Err(e) = display_image_chafa(page, &img.url, 60).await {
			elog!("Failed to display image: {}", e);
			eprintln!("  [Image: {}]", img.alt.as_deref().unwrap_or(&img.url));
		}
	}

	// Display required files
	let required_files = question.required_files();
	if !required_files.is_empty() {
		eprintln!("\nRequired files:");
		for file in required_files {
			if file.content.is_empty() {
				eprintln!("  - {}", file.name);
			} else {
				eprintln!("  - {} (has template)", file.name);
			}
		}
	}
	eprintln!();

	if !ask_llm {
		// If not using LLM, just display the question
		return Ok(false);
	}

	// Ask LLM to generate code
	log!("Asking LLM to generate code solution...");
	let code_result = match ask_llm_for_code(&question, config).await {
		Ok(result) => {
			eprintln!("\nGenerated code:");
			for (filename, content) in &result.files {
				eprintln!("\n=== {} ===", filename);
				eprintln!("{}", content);
			}
			eprintln!();
			result
		}
		Err(e) => {
			elog!("Failed to generate code: {}", e);
			return Ok(false);
		}
	};

	if code_result.files.is_empty() {
		elog!("No code files generated");
		return Ok(false);
	}

	// Ask for confirmation before pasting (skip if auto_submit is enabled)
	if !config.auto_submit && !confirm("Paste generated code into editor?").await {
		log!("Cancelled by user");
		return Ok(false);
	}

	// Track conversation for retries
	let mut conversation = code_result.conversation;
	let mut files = code_result.files;

	// Navigate to the Edit page (only on first attempt)
	log!("Navigating to VPL editor...");
	if !click_vpl_edit_button(page).await? {
		elog!("Could not find Edit button on VPL page");
		return Ok(false);
	}

	// Wait for editor page to fully load
	page.wait_for_navigation().await.map_err(|e| eyre!("Failed waiting for navigation: {e}"))?;
	tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

	// Retry loop for test failures
	let max_retries = config.max_consecutive_failures;
	for attempt in 0..=max_retries {
		if attempt > 0 {
			log!("Retry attempt {}/{}", attempt, max_retries);
		}

		// Save the editor page HTML
		#[cfg(feature = "xdg")]
		if let Err(e) = save_page_html(page, session_id).await {
			elog!("Failed to save editor page HTML: {e}");
		}

		log!("Pasting code into editor...");
		tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
		for (filename, content) in &files {
			// Prepend empty line - VPL panics without it
			let content = format!("\n{content}");
			if let Err(e) = set_vpl_file_content(page, filename, &content).await {
				elog!("Failed to set content for {filename}: {e}");
			}
		}
		tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

		log!("Saving code...");
		tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
		if !click_vpl_button_with_retry(page, "save", config.button_click_retries).await? {
			run_stop_hook(config, "Could not find Save button");
			bail!("Could not find Save button - aborting");
		}

		tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
		log!("Running evaluation...");
		if !click_vpl_button_with_retry(page, "evaluate", config.button_click_retries).await? {
			run_stop_hook(config, "Could not find Evaluate button");
			bail!("Could not find Evaluate button - aborting");
		}
		log!("Waiting for evaluation results...");
		tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

		let eval_result = parse_vpl_evaluation_result(page).await?;
		if let Some(result) = &eval_result {
			eprintln!("\n=== Evaluation Result ===");
			eprintln!("{result}");
		} else {
			log!("No evaluation result found (may still be running)");
		}

		// Parse proposed grade
		let grade = parse_vpl_proposed_grade(page).await?;
		if let Some(grade) = grade {
			eprintln!("Proposed grade: {grade}");
			if grade >= 1.0 {
				log!("Full marks! Evaluation successful.");
				run_stop_hook(config, "VPL: Full marks!");
				return Ok(true);
			}

			// Not perfect - try to get test results and retry
			if attempt < max_retries {
				let test_results = parse_vpl_test_results(page).await?;
				if let Some(test_results) = test_results {
					eprintln!("\n=== Test Failure Details ===");
					eprintln!("{}", test_results);

					// Ask LLM to fix the code with test results
					log!("Asking LLM to fix the code based on test results...");
					match retry_llm_with_test_results(conversation, &test_results, config).await {
						Ok(result) => {
							eprintln!("\nRegenerated code:");
							for (filename, content) in &result.files {
								eprintln!("\n=== {filename} ===");
								eprintln!("{content}");
							}
							eprintln!();

							// Ask for confirmation before pasting regenerated code
							if !config.auto_submit && !confirm("Paste regenerated code into editor?").await {
								log!("Cancelled by user");
								run_stop_hook(config, "VPL: Cancelled by user");
								bail!("Evaluation failed: got {} (expected 100%)", grade * Percent(1.0));
							}

							// Update for next iteration
							conversation = result.conversation;
							files = result.files;
							continue;
						}
						Err(e) => {
							elog!("Failed to regenerate code: {}", e);
							run_stop_hook(config, &format!("VPL: Failed to regenerate code: {}", e));
							bail!("Evaluation failed: got {} (expected 100%)", grade * Percent(1.0));
						}
					}
				} else {
					elog!("Could not parse test results for retry");
					run_stop_hook(config, "VPL: Could not parse test results");
					bail!("Evaluation failed: got {} (expected 100%)", grade * Percent(1.0));
				}
			} else {
				let msg = format!("VPL: Failed after {} retries ({}%)", max_retries, grade * Percent(1.0));
				run_stop_hook(config, &msg);
				bail!("Evaluation failed after {} retries: got {} (expected 100%)", max_retries, grade * Percent(1.0));
			}
		} else {
			run_stop_hook(config, "VPL: Could not find proposed grade");
			bail!("Could not find proposed grade in evaluation results");
		}
	}

	run_stop_hook(config, "VPL: Exhausted all retry attempts");
	bail!("Exhausted all retry attempts");
}

/// Handle a quiz (multi-choice) page
/// Returns Ok(true) if at least one answer was submitted, Ok(false) if questions existed but none were answered
pub async fn handle_quiz_page(page: &Page, ask_llm: bool, config: &mut AppConfig, session_id: &str) -> Result<bool> {
	use v_utils::io::{ConfirmAllResult, confirm_all};

	let mut question_num = 0;
	let mut consecutive_failures = 0;
	let mut first_page = true;
	let mut total_questions_found = 0;
	let mut total_answers_submitted = 0;

	loop {
		// Print page separator
		let current_url = page.url().await.ok().flatten().unwrap_or_default();
		let page_num = current_url.split("page=").nth(1).and_then(|s| s.split('&').next()).and_then(|s| s.parse::<u32>().ok());

		if !first_page {
			if let Some(num) = page_num {
				log!("\n==================== Page {num} ====================");
			} else {
				log!("\n================================================");
			}
		}
		first_page = false;

		// Save page HTML before parsing for debugging
		#[cfg(feature = "xdg")]
		if let Err(e) = save_page_html(page, session_id).await {
			elog!("Failed to save quiz page HTML: {e}");
		}

		let questions = parse_questions(page).await?;

		if questions.is_empty() {
			// Only check for confirmation prompts when there are no questions to answer
			let confirmation_buttons = find_confirmation_buttons(page, false).await?;
			if !confirmation_buttons.is_empty() {
				log!("Found {} confirmation prompt(s):", confirmation_buttons.len());
				for btn in &confirmation_buttons {
					log!("  - {btn}");
				}

				if config.continuation_prompts {
					log!("Auto-clicking confirmation buttons...");
					if click_all_confirmations(page).await? {
						// Modal confirmation clicked = quiz submitted, we're done
						run_stop_hook(config, "Quiz submitted successfully");
						return Ok(total_answers_submitted > 0 || total_questions_found == 0);
					}
				} else {
					log!("(set continuation_prompts = true in config to auto-click)");
				}
			}

			if !config.visible {
				elog!("No questions found on page. // Might be a fucky-wucky, but we're in headless, so exiting.");
				run_stop_hook(config, "No questions found on page");
				std::process::exit(1);
			}
			log!("No more questions found. Waiting for manual intervention or page change...");
			run_stop_hook(config, "No more questions found");
			wait_for_page_change(page).await?;
			continue;
		}

		total_questions_found += questions.len();

		// Display all questions on this page
		for (i, question) in questions.iter().enumerate() {
			let type_marker = if question.is_short_answer() {
				"[text]"
			} else if question.is_matching() {
				"[match]"
			} else if question.is_fill_in_blanks() {
				"[fill]"
			} else if question.is_code_block() {
				"[code]"
			} else if question.is_multi() {
				"[multi]"
			} else {
				"[single]"
			};
			let header = format!("--- Question {} {} ---", question_num + i + 1, type_marker);
			tracing::info!("{}", header);
			eprintln!("{}", header);

			let question_str = question.to_string();
			tracing::info!("{}", question_str);
			eprint!("{}", question_str);

			// Display question images
			for img in question.images() {
				if let Err(e) = display_image_chafa(page, &img.url, 60).await {
					elog!("Failed to display image: {}", e);
					eprintln!("  [Image: {}]", img.alt.as_deref().unwrap_or(&img.url));
				}
			}

			// Display choice images
			for choice in question.choices() {
				for img in &choice.images {
					if let Err(e) = display_image_chafa(page, &img.url, 40).await {
						elog!("Failed to display choice image: {}", e);
						eprintln!("    [Image: {}]", img.alt.as_deref().unwrap_or(&img.url));
					}
				}
			}

			eprintln!(); // newline between questions
		}

		if !ask_llm {
			// If not using LLM, just display questions and exit
			break;
		}

		// Collect answers for all questions on this page
		let mut answers_to_select: Vec<(&Question, LlmAnswerResult)> = Vec::new();
		let mut answer_logs: Vec<String> = Vec::new();

		for question in &questions {
			question_num += 1;

			match ask_llm_for_answer(page, question, config).await {
				Ok(answer_result) => {
					consecutive_failures = 0; // Reset on success

					// Collect answer display for later
					let type_marker = if question.is_short_answer() {
						"[text]"
					} else if question.is_matching() {
						"[match]"
					} else if question.is_fill_in_blanks() {
						"[fill]"
					} else if question.is_code_block() {
						"[code]"
					} else if question.is_multi() {
						"[multi]"
					} else {
						"[single]"
					};
					answer_logs.push(format!("Question {question_num} {type_marker} answer:"));
					match &answer_result {
						LlmAnswerResult::Single { idx, text } => {
							answer_logs.push(format!("  Selected: {}. {}", idx + 1, text));
						}
						LlmAnswerResult::Multi { indices, texts } => {
							answer_logs.push("  Selected:".to_string());
							for (idx, text) in indices.iter().zip(texts.iter()) {
								answer_logs.push(format!("    {}. {}", idx + 1, text));
							}
						}
						LlmAnswerResult::Text { answer } => {
							answer_logs.push(format!("  Answer: {}", answer));
						}
						LlmAnswerResult::Matching { selections } => {
							answer_logs.push("  Matches:".to_string());
							// Find the answer text for each selection
							for (select_name, value) in selections {
								// Find the item and option text
								for item in question.match_items() {
									if &item.select_name == select_name {
										let answer_text = item.options.iter().find(|o| &o.value == value).map(|o| o.text.as_str()).unwrap_or("?");
										answer_logs.push(format!("    {} -> {}", item.prompt, answer_text));
										break;
									}
								}
							}
						}
						LlmAnswerResult::FillInBlanks { answers } => {
							answer_logs.push("  Blanks:".to_string());
							if let Some(fill) = question.fill_in_blanks() {
								for (i, blank) in fill.blanks.iter().enumerate() {
									// Find the answer for this blank
									let answer_text = answers
										.iter()
										.find(|a| match (a, blank) {
											(FillInBlanksAnswerItem::Text { input_name, .. }, Blank::Text { input_name: bn, .. }) => input_name == bn,
											(FillInBlanksAnswerItem::Select { select_name, .. }, Blank::Select { select_name: sn, .. }) => select_name == sn,
											_ => false,
										})
										.map(|a| match a {
											FillInBlanksAnswerItem::Text { answer, .. } => answer.clone(),
											FillInBlanksAnswerItem::Select { value, .. } => {
												// Find the option text for this value
												if let Blank::Select { options, .. } = blank {
													options.iter().find(|o| &o.value == value).map(|o| o.text.clone()).unwrap_or_else(|| value.clone())
												} else {
													value.clone()
												}
											}
										})
										.unwrap_or_else(|| "?".to_string());
									answer_logs.push(format!("    [{}]: {}", i + 1, answer_text));
								}
							}
						}
						LlmAnswerResult::CodeBlock { code } => {
							// Show first few lines of code
							let lines: Vec<&str> = code.lines().take(5).collect();
							answer_logs.push("  Code:".to_string());
							for line in lines {
								answer_logs.push(format!("    {line}"));
							}
							if code.lines().count() > 5 {
								answer_logs.push(format!("    ... ({} more lines)", code.lines().count() - 5));
							}
						}
					}

					answers_to_select.push((question, answer_result));
				}
				Err(e) => {
					consecutive_failures += 1;
					elog!(
						"Failed to get LLM answer for question {question_num}: {e} ({consecutive_failures}/{})",
						config.max_consecutive_failures
					);
					if consecutive_failures >= config.max_consecutive_failures {
						run_stop_hook(config, &format!("Quiz: Exceeded {} consecutive LLM failures", config.max_consecutive_failures));
						bail!("Exceeded {} consecutive LLM failures", config.max_consecutive_failures);
					}
					// Skip this question but continue with others
				}
			}
		}

		// Display all answers at once with newlines around
		if !answer_logs.is_empty() {
			let mut output = String::from("\n");
			for line in &answer_logs {
				tracing::info!("{line}");
				output.push_str(line);
				output.push('\n');
			}
			output.push('\n');
			print!("{}", output);
		}

		if answers_to_select.is_empty() {
			// We had questions but couldn't get any answers from LLM
			if total_questions_found > 0 && total_answers_submitted == 0 {
				elog!(
					"No answers to submit. LLM failed to answer all {total_questions_found} question(s).\nThis may be a transient API error. Try running again, or check your CLAUDE_TOKEN."
				);
			} else {
				log!("No answers to submit on this page.");
			}

			break;
		}

		// Ask for confirmation once for all answers on this page
		let should_submit = if config.auto_submit {
			Some(true)
		} else {
			// Race between user confirmation and detecting manual submission
			let confirm_msg = format!("Submit {} answer(s)?", answers_to_select.len());
			tokio::select! {
				biased;
				result = confirm_all(&confirm_msg) => {
					match result {
						ConfirmAllResult::Yes => Some(true),
						ConfirmAllResult::All => {
							// SAFETY: single-threaded, no concurrent reads
							unsafe { config.set_auto_submit(true) };
							Some(true)
						}
						ConfirmAllResult::No => None, // User will submit manually
					}
				}
				_ = wait_for_page_change(page) => {
					log!("User submitted manually.");
					Some(false) // Already submitted, don't submit again
				}
			}
		};

		match should_submit {
			Some(true) => {
				// Select all answers on this page
				for (question, answer_result) in &answers_to_select {
					match answer_result {
						LlmAnswerResult::Single { idx, .. } => {
							let choices = question.choices();
							let choice = &choices[*idx];
							// Only click if not already selected
							if !choice.selected {
								toggle_answer(page, &choice.input_name, &choice.input_value).await?;
							}
						}
						LlmAnswerResult::Multi { indices, .. } => {
							let choices = question.choices();
							let should_select: std::collections::HashSet<usize> = indices.iter().copied().collect();
							for (i, choice) in choices.iter().enumerate() {
								let want_selected = should_select.contains(&i);
								if want_selected != choice.selected {
									// Need to toggle this choice
									toggle_answer(page, &choice.input_name, &choice.input_value).await?;
								}
							}
						}
						LlmAnswerResult::Text { answer } =>
							if let Some(input_name) = question.short_answer_input_name() {
								set_text_answer(page, input_name, answer).await?;
							},
						LlmAnswerResult::Matching { selections } =>
							for (select_name, value) in selections {
								set_select_value(page, select_name, value).await?;
							},
						LlmAnswerResult::FillInBlanks { answers } =>
							for item in answers {
								match item {
									FillInBlanksAnswerItem::Text { input_name, answer } => {
										set_text_answer(page, input_name, answer).await?;
									}
									FillInBlanksAnswerItem::Select { select_name, value } => {
										set_select_value(page, select_name, value).await?;
									}
								}
							},
						LlmAnswerResult::CodeBlock { code } =>
							if let Some(input_name) = question.code_block_input_name() {
								set_code_editor_content(page, input_name, code).await?;
							},
					}
				}
				// Submit once for all questions on this page
				click_submit(page).await?;
				total_answers_submitted += answers_to_select.len();
				log!("All {} answer(s) submitted!", answers_to_select.len());
			}
			Some(false) => {
				// Already submitted by user, count as submitted
				total_answers_submitted += answers_to_select.len();
			}
			None => {
				// User said no, wait for them to submit manually
				log!("Waiting for manual submission...");
				wait_for_page_change(page).await?;
				log!("Page changed, continuing...");
			}
		}
	}

	// Return success if we submitted at least one answer, or if there were no questions to answer
	Ok(total_answers_submitted > 0 || total_questions_found == 0)
}

/// Click the Edit button on a VPL page to open the editor
async fn click_vpl_edit_button(page: &Page) -> Result<bool> {
	let script = r#"
		(function() {
			// Look for nav-link with title "Edit"
			const editLink = document.querySelector('a.nav-link[title="Edit"]');
			if (editLink) {
				editLink.click();
				return true;
			}

			// Fallback: href-based
			const hrefLink = document.querySelector('a[href*="forms/edit.php"]');
			if (hrefLink) {
				hrefLink.click();
				return true;
			}

			return false;
		})()
	"#;

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to click Edit button: {}", e))?;
	Ok(result.value().and_then(|v| v.as_bool()).unwrap_or(false))
}

/// Click a VPL button by action name (save, evaluate, run, debug)
/// Uses chromiumoxide's native click to emulate a real mouse click
/// Returns Ok(true) if clicked, Ok(false) if button not found, Err if click failed
async fn click_vpl_button(page: &Page, action: &str) -> Result<bool> {
	// First, try to find by exact ID
	let button_id = format!("vpl_ide_{}", action);
	let selector = format!("#{}", button_id);

	// Try to find and click the element using CDP
	let el = page.find_element(&selector).await;
	if let Ok(element) = el {
		element.click().await.map_err(|e| eyre!("Failed to click element: {}", e))?;
		return Ok(true);
	}

	// Fallback: search by title attribute containing the action
	let fallback_selector = format!(r#"[id^="vpl_ide_"][title*="{}" i]"#, action);
	let el = page.find_element(&fallback_selector).await;
	if let Ok(element) = el {
		element.click().await.map_err(|e| eyre!("Failed to click element: {}", e))?;
		return Ok(true);
	}

	Ok(false)
}

/// Click a VPL button with retry logic
/// Retries up to max_retries times if the click fails (timeout, etc.)
async fn click_vpl_button_with_retry(page: &Page, action: &str, max_retries: u32) -> Result<bool> {
	for attempt in 1..=max_retries {
		match click_vpl_button(page, action).await {
			Ok(true) => return Ok(true),
			Ok(false) => return Ok(false), // Button not found, no point retrying
			Err(e) =>
				if attempt < max_retries {
					elog!("Click on '{action}' failed (attempt {attempt}/{max_retries}): {e}");
					tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
				} else {
					return Err(e);
				},
		}
	}
	Ok(false)
}

/// Set the content of a file in the VPL editor
async fn set_vpl_file_content(page: &Page, filename: &str, content: &str) -> Result<()> {
	// Escape the content for JavaScript
	let escaped_content = content
		.replace('\\', "\\\\")
		.replace('`', "\\`")
		.replace('$', "\\$")
		.replace('\n', "\\n")
		.replace('\r', "\\r")
		.replace('\t', "\\t");

	let script = format!(
		r#"
		(function() {{
			const filename = "{}";
			const content = `{}`;

			// VPL uses ACE editor - find and set content
			if (typeof ace !== 'undefined') {{
				const editors = document.querySelectorAll('.ace_editor');
				for (const editorEl of editors) {{
					const editor = ace.edit(editorEl);
					if (editor) {{
						editor.setValue(content, -1);
						return true;
					}}
				}}
			}}

			// Try VPL's own editor API
			if (typeof VPL !== 'undefined' && VPL.editor) {{
				VPL.editor.setContent(content);
				return true;
			}}

			// Fallback: find textarea and set value
			const textareas = document.querySelectorAll('textarea');
			for (const ta of textareas) {{
				if (ta.name && ta.name.includes('file') || ta.id && ta.id.includes('file')) {{
					ta.value = content;
					ta.dispatchEvent(new Event('input', {{ bubbles: true }}));
					return true;
				}}
			}}

			// Last resort: find any visible textarea
			for (const ta of textareas) {{
				if (ta.offsetParent !== null) {{
					ta.value = content;
					ta.dispatchEvent(new Event('input', {{ bubbles: true }}));
					return true;
				}}
			}}

			return false;
		}})()
		"#,
		filename, escaped_content
	);

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to set file content: {}", e))?;

	if result.value().and_then(|v| v.as_bool()) != Some(true) {
		return Err(eyre!("Could not find editor to set content"));
	}

	Ok(())
}

/// Parse the evaluation result from the VPL page
async fn parse_vpl_evaluation_result(page: &Page) -> Result<Option<String>> {
	let script = r#"
		(function() {
			const selectors = [
				'.vpl_ide_console',
				'.vpl_ide_result',
				'#vpl_console',
				'.console-output',
				'#result',
				'.evaluation-result',
				'pre.result'
			];

			for (const selector of selectors) {
				const el = document.querySelector(selector);
				if (el && el.textContent.trim()) {
					return el.textContent.trim();
				}
			}

			const allElements = document.querySelectorAll('*');
			for (const el of allElements) {
				const text = el.textContent;
				if (text && (text.includes('Grade:') || text.includes('Result:') ||
				    text.includes('Passed') || text.includes('Failed') ||
				    text.includes('Score:') || text.includes('Points:'))) {
					const directText = Array.from(el.childNodes)
						.filter(n => n.nodeType === Node.TEXT_NODE)
						.map(n => n.textContent.trim())
						.join(' ');
					if (directText) return directText;
				}
			}

			return null;
		})()
	"#;

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to parse evaluation result: {}", e))?;

	Ok(result.value().and_then(|v| v.as_str()).map(|s| s.to_string()))
}

/// Parse test results from the VPL comments section
/// Returns the test failure messages if found
async fn parse_vpl_test_results(page: &Page) -> Result<Option<String>> {
	let script = r#"
		(function() {
			// Find comments section by class
			const comments = document.querySelector('.vpl_ide_accordion_c_comments');
			if (!comments) return null;

			// Get all text content, preserving structure
			const parts = [];
			let inTestResult = false;

			function walkNode(node) {
				if (node.nodeType === Node.TEXT_NODE) {
					const text = node.textContent.trim();
					if (text) {
						// Stop at "Description" - that's where problem description starts
						if (text.startsWith('Description')) {
							return false;
						}
						parts.push(text);
					}
				} else if (node.nodeType === Node.ELEMENT_NODE) {
					const tag = node.tagName.toLowerCase();
					if (tag === 'br') {
						parts.push('\n');
					} else if (tag === 'b') {
						// Bold = test header, start collecting
						inTestResult = true;
						parts.push('\n[TEST] ');
						for (const child of node.childNodes) {
							if (walkNode(child) === false) return false;
						}
					} else {
						for (const child of node.childNodes) {
							if (walkNode(child) === false) return false;
						}
					}
				}
				return true;
			}

			walkNode(comments);

			// Clean up and return
			const result = parts.join('').trim();
			if (!result || result.length < 10) return null;

			return result;
		})()
	"#;

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to parse test results: {}", e))?;

	Ok(result.value().and_then(|v| v.as_str()).map(|s| s.to_string()))
}

/// Parse the proposed grade from VPL evaluation results
async fn parse_vpl_proposed_grade(page: &Page) -> Result<Option<Percent>> {
	let script = r#"
		(function() {
			const allElements = document.querySelectorAll('*');
			for (const el of allElements) {
				const text = el.textContent || '';
				if (text.startsWith('Proposed grade:')) {
					return text;
				}
			}
			const results = document.querySelector('.vpl_ide_results, #vpl_results, .console-output');
			if (results) {
				const text = results.textContent || '';
				const match = text.match(/Proposed grade:\s*[\d.]+\s*\/\s*[\d.]+/);
				if (match) return match[0];
			}
			return null;
		})()
	"#;

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to parse proposed grade: {}", e))?;

	let Some(text) = result.value().and_then(|v| v.as_str()) else {
		return Ok(None);
	};

	let re = regex::Regex::new(r"Proposed grade:\s*([\d.]+)\s*/\s*([\d.]+)").map_err(|e| eyre!("Regex error: {}", e))?;
	let Some(caps) = re.captures(text) else {
		return Ok(None);
	};

	let score: f64 = caps.get(1).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0);
	let total: f64 = caps.get(2).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(1.0);

	let percent = if total > 0.0 { score / total } else { 0.0 };
	Ok(Some(Percent(percent)))
}

/// Shared JS helper to check if text matches confirmation keywords
const CONFIRMATION_MATCH_JS: &str = r#"
	function isConfirmationText(text) {
		const t = text.toLowerCase();
		return t.includes('envoyer') || t.includes('terminer') || t.includes('submit') || t.includes('finir') || t.includes('confirm') || t.includes('valider');
	}
"#;

/// Find confirmation buttons on the page and optionally click them
/// Returns a list of button names found
async fn find_confirmation_buttons(page: &Page, click: bool) -> Result<Vec<String>> {
	let script = format!(
		r#"
		(function() {{
			{CONFIRMATION_MATCH_JS}
			const shouldClick = {click};
			const names = [];

			// Mark as done buttons
			const markDoneButtons = document.querySelectorAll(
				'button[data-action="toggle-manual-completion"], button[data-toggletype="manual:mark-done"]'
			);
			for (const btn of markDoneButtons) {{
				const name = btn.getAttribute('data-activityname') || btn.textContent.trim();
				names.push(name);
				if (shouldClick) btn.click();
			}}

			// Submit all and finish buttons (quiz summary page)
			const submitAllBtns = document.querySelectorAll('button[type="submit"].btn-primary');
			for (const btn of submitAllBtns) {{
				if (isConfirmationText(btn.textContent)) {{
					names.push(btn.textContent.trim());
					if (shouldClick) btn.click();
				}}
			}}

			// Finish attempt button/link on quiz attempt pages (navigates to summary)
			const finishAttemptBtns = document.querySelectorAll('.mod_quiz-next-nav, button[name="next"], input[name="next"][type="submit"]');
			for (const btn of finishAttemptBtns) {{
				const text = btn.textContent?.trim() || btn.value || '';
				if (isConfirmationText(text)) {{
					names.push(text || 'Finish attempt');
					if (shouldClick) btn.click();
				}}
			}}

			// Also check for "Finish attempt..." links in the quiz navigation
			const finishLinks = document.querySelectorAll('a.endtestlink, a[href*="summary"]');
			for (const link of finishLinks) {{
				if (isConfirmationText(link.textContent)) {{
					names.push(link.textContent.trim());
					if (shouldClick) link.click();
				}}
			}}

			return JSON.stringify(names);
		}})()
	"#
	);

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to find confirmation buttons: {}", e))?;
	let json_str = result.value().and_then(|v| v.as_str()).unwrap_or("[]");
	let names: Vec<String> = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse JSON: {}", e))?;

	if click && !names.is_empty() {
		log!("Clicked {} confirmation button(s)", names.len());
	}

	Ok(names)
}

/// Click all confirmation buttons, then wait and handle any modal that appears
/// Returns true if a modal confirmation was clicked (quiz is done)
async fn click_all_confirmations(page: &Page) -> Result<bool> {
	find_confirmation_buttons(page, true).await?;
	// Wait for potential modal to appear
	tokio::time::sleep(std::time::Duration::from_millis(500)).await;
	click_modal_confirmation(page).await
}

/// Click confirmation button in modal dialogs (e.g., "Tout envoyer et terminer" popup)
/// Returns true if a modal confirmation was clicked
async fn click_modal_confirmation(page: &Page) -> Result<bool> {
	let script = format!(
		r#"
		(function() {{
			{CONFIRMATION_MATCH_JS}
			// Look for modal confirmation buttons - try multiple selectors for different Moodle versions
			const modalBtns = document.querySelectorAll(
				'.modal button.btn-primary, .modal-dialog button.btn-primary, [role="dialog"] button.btn-primary, ' +
				'.moodle-dialogue button.btn-primary, .yui3-panel button.btn-primary, [data-region="modal"] button.btn-primary'
			);
			for (const btn of modalBtns) {{
				if (isConfirmationText(btn.textContent)) {{
					btn.click();
					return true;
				}}
			}}
			return false;
		}})()
	"#
	);

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to click modal confirmation: {}", e))?;
	let clicked = result.value().and_then(|v| v.as_bool()) == Some(true);
	if clicked {
		log!("Clicked modal confirmation button");
	}

	Ok(clicked)
}

/// Parse questions from the quiz page
async fn parse_questions(page: &Page) -> Result<Vec<Question>> {
	let parse_script = r#"
		(function() {
			function extractImages(element) {
				if (!element) return [];
				const images = [];
				const imgElements = element.querySelectorAll('img');
				for (const img of imgElements) {
					const url = img.src || '';
					if (url) {
						images.push({ url: url, alt: img.alt || null });
					}
				}
				return images;
			}

			function extractTextWithLatex(element) {
				if (!element) return '';
				const clone = element.cloneNode(true);

				const mjxContainers = clone.querySelectorAll('mjx-container');
				for (const container of mjxContainers) {
					let latex = null;
					const annotation = container.querySelector('annotation[encoding="application/x-tex"]');
					if (annotation) latex = annotation.textContent;
					if (!latex && container.dataset.latex) latex = container.dataset.latex;
					const mathScript = container.querySelector('script[type="math/tex"]');
					if (!latex && mathScript) latex = mathScript.textContent;

					if (latex) {
						const isDisplay = container.getAttribute('display') === 'true' || container.classList.contains('MJXc-display');
						const wrapper = isDisplay ? ['\\[', '\\]'] : ['\\(', '\\)'];
						container.replaceWith(document.createTextNode(wrapper[0] + latex + wrapper[1]));
					} else {
						const accessibleText = container.querySelector('.MJX_Assistive_MathML, mjx-assistive-mml');
						if (accessibleText) container.replaceWith(document.createTextNode(accessibleText.textContent || ''));
					}
				}

				const mj2Spans = clone.querySelectorAll('.MathJax, .MathJax_Preview, .MathJax_Display');
				for (const span of mj2Spans) {
					const script = span.nextElementSibling;
					if (script && script.tagName === 'SCRIPT' && script.type && script.type.includes('math/tex')) {
						const latex = script.textContent;
						const isDisplay = script.type.includes('mode=display');
						const wrapper = isDisplay ? ['\\[', '\\]'] : ['\\(', '\\)'];
						span.replaceWith(document.createTextNode(wrapper[0] + latex + wrapper[1]));
						script.remove();
					} else {
						span.remove();
					}
				}

				const mathScripts = clone.querySelectorAll('script[type*="math/tex"]');
				for (const script of mathScripts) {
					const latex = script.textContent;
					const isDisplay = script.type.includes('mode=display');
					const wrapper = isDisplay ? ['\\[', '\\]'] : ['\\(', '\\)'];
					script.replaceWith(document.createTextNode(wrapper[0] + latex + wrapper[1]));
				}

				return clone.textContent.replace(/\s+/g, ' ').trim();
			}

			const questions = [];
			const formulations = document.querySelectorAll('.formulation.clearfix');

			for (const formulation of formulations) {
				const qtextEl = formulation.querySelector('.qtext');
				// For multianswer questions, qtext may not exist - question is directly in formulation
				// In that case, extract text from the filter_mathjaxloader_equation span
				let questionText = extractTextWithLatex(qtextEl) || '';
				if (!questionText) {
					const mathjaxSpan = formulation.querySelector('.filter_mathjaxloader_equation');
					if (mathjaxSpan) {
						questionText = extractTextWithLatex(mathjaxSpan) || '';
					}
				}
				const questionImages = extractImages(qtextEl) || extractImages(formulation);

				// Check for code block questions (vplquestion with code-editor textarea)
				const questionWrapper = formulation.closest('.que');
				if (questionWrapper && questionWrapper.classList.contains('vplquestion')) {
					const codeTextarea = formulation.querySelector('textarea[data-role="code-editor"]');
					if (codeTextarea) {
						const language = codeTextarea.dataset.templatelang || 'text';
						// For vplquestion, question text is in .clearfix div, not .qtext
						let codeQuestionText = questionText;
						if (!codeQuestionText) {
							const clearfixDiv = formulation.querySelector('.clearfix');
							codeQuestionText = extractTextWithLatex(clearfixDiv) || '';
						}
						const codeQuestionImages = questionImages.length > 0 ? questionImages : extractImages(formulation.querySelector('.clearfix'));
						questions.push({
							type: 'CodeBlock',
							question_text: codeQuestionText,
							input_name: codeTextarea.name || '',
							language: language,
							current_code: codeTextarea.value || '',
							images: codeQuestionImages
						});
						continue;
					}
				}

				// Check for fill-in-the-blanks (multianswer / cloze questions)
				// These have .subquestion spans with inputs/selects embedded in the content
				// Also check for inputs directly in .qtext, .ablock, or the formulation itself
				const ablockDiv = formulation.querySelector('.ablock');
				const subquestionInputs = formulation.querySelectorAll('.subquestion input[type="text"], .subquestion select');
				const allInlineInputs = formulation.querySelectorAll(
					'.qtext input[type="text"], .ablock input[type="text"], .qtext select, .ablock select, ' +
					'.subquestion input[type="text"], .subquestion select'
				);
				const hasMultipleInlineInputs = allInlineInputs.length > 1;
				const hasInlineSelect = formulation.querySelector('.qtext select, .ablock select, .subquestion select') !== null;
				const hasInlineTextInput = formulation.querySelector('.qtext input[type="text"], .ablock input[type="text"], .subquestion input[type="text"]') !== null;

				// If we have multiple inline inputs OR a mix of text inputs and selects OR any subquestion inputs, it's fill-in-blanks
				// (single subquestion input should also be parsed as fill-in-blanks to preserve context)
				const hasSubquestionInputs = subquestionInputs.length > 0;
				if (hasMultipleInlineInputs || (hasInlineSelect && hasInlineTextInput) || hasSubquestionInputs) {
					// Parse segments: walk through the formulation content and extract text/blanks in order
					// Use formulation itself since content may be directly in it (multianswer questions)
					const contentArea = formulation;
					const segments = [];
					const blanks = [];
					let blankIndex = 0;

					function walkForSegments(node) {
						if (node.nodeType === Node.TEXT_NODE) {
							const text = node.textContent;
							if (text.trim()) {
								segments.push({ type: 'text', text: text });
							}
						} else if (node.nodeType === Node.ELEMENT_NODE) {
							const tag = node.tagName.toLowerCase();

							// Skip hidden inputs and accessibility labels
							if (tag === 'input' && node.type === 'hidden') {
								return;
							}
							if (tag === 'label' && node.classList.contains('accesshide')) {
								return;
							}
							// Skip info/header elements
							if (tag === 'h4' && node.classList.contains('accesshide')) {
								return;
							}

							if (tag === 'input' && node.type === 'text') {
								segments.push({ type: 'blank', index: blankIndex });
								blanks.push({
									type: 'text',
									input_name: node.name || '',
									current_value: node.value || ''
								});
								blankIndex++;
							} else if (tag === 'select') {
								segments.push({ type: 'blank', index: blankIndex });
								const options = [];
								for (const opt of node.options) {
									if (opt.value !== '') {
										options.push({
											value: opt.value,
											text: opt.textContent.trim()
										});
									}
								}
								blanks.push({
									type: 'select',
									select_name: node.name || '',
									options: options,
									selected_value: node.value || ''
								});
								blankIndex++;
							} else if (tag === 'br') {
								segments.push({ type: 'text', text: '\n' });
							} else if (tag === 'p') {
								// Add paragraph break
								segments.push({ type: 'text', text: '\n' });
								for (const child of node.childNodes) {
									walkForSegments(child);
								}
								segments.push({ type: 'text', text: '\n' });
							} else if (!['script', 'style', 'mjx-container', 'img'].includes(tag)) {
								// Recurse into child nodes
								for (const child of node.childNodes) {
									walkForSegments(child);
								}
							}
						}
					}

					if (contentArea) {
						walkForSegments(contentArea);
					}

					if (blanks.length > 0) {
						questions.push({
							type: 'FillInBlanks',
							question_text: questionText,
							segments: segments,
							blanks: blanks,
							images: questionImages
						});
						continue;
					}
				}

				// Check for short answer (text input) questions
				// These have .ablock with a single input[type="text"] but no .answer div with radio/checkbox
				const textInput = ablockDiv?.querySelector('input[type="text"]');
				if (textInput && textInput.name && !hasMultipleInlineInputs) {
					questions.push({
						type: 'ShortAnswer',
						question_text: questionText,
						input_name: textInput.name,
						current_answer: textInput.value || '',
						images: questionImages
					});
					continue;
				}

				// Check for matching questions (dropdowns in a table)
				const answerTable = formulation.querySelector('.ablock table.answer');
				if (answerTable) {
					const selects = answerTable.querySelectorAll('select');
					if (selects.length > 0) {
						const items = [];
						for (const select of selects) {
							const row = select.closest('tr');
							const textCell = row?.querySelector('.text');
							const prompt = extractTextWithLatex(textCell);

							const options = [];
							for (const opt of select.options) {
								options.push({
									value: opt.value,
									text: opt.textContent.trim()
								});
							}

							items.push({
								prompt: prompt,
								select_name: select.name || '',
								options: options,
								selected_value: select.value || '0'
							});
						}

						if (items.length > 0) {
							questions.push({
								type: 'Matching',
								question_text: questionText,
								items: items,
								images: questionImages
							});
							continue;
						}
					}
				}

				// Check for inline dropdown questions (select embedded in question text)
				const inlineSelects = formulation.querySelectorAll('.subquestion select, .qtext select');
				if (inlineSelects.length > 0) {
					const items = [];
					for (const select of inlineSelects) {
						// For inline selects, the prompt is the surrounding text
						// We'll use an empty prompt since the context is in questionText
						const options = [];
						for (const opt of select.options) {
							if (opt.value !== '') {  // Skip empty placeholder option
								options.push({
									value: opt.value,
									text: opt.textContent.trim()
								});
							}
						}

						items.push({
							prompt: '',  // Context is in the question text
							select_name: select.name || '',
							options: options,
							selected_value: select.value || ''
						});
					}

					if (items.length > 0) {
						questions.push({
							type: 'Matching',
							question_text: questionText,
							items: items,
							images: questionImages
						});
						continue;
					}
				}

				const answerDiv = formulation.querySelector('.answer');
				if (!answerDiv) continue;

				const radioInputs = answerDiv.querySelectorAll('input[type="radio"]');
				const checkboxInputs = answerDiv.querySelectorAll('input[type="checkbox"]');

				const choices = [];
				let questionType = 'SingleChoice';

				if (radioInputs.length > 0) {
					questionType = 'SingleChoice';
					for (const radio of radioInputs) {
						const labelEl = radio.closest('div')?.querySelector('label, .ml-1, .flex-fill');
						choices.push({
							input_name: radio.name || '',
							input_value: radio.value || '',
							text: extractTextWithLatex(labelEl),
							selected: radio.checked,
							images: extractImages(labelEl)
						});
					}
				} else if (checkboxInputs.length > 0) {
					questionType = 'MultiChoice';
					for (const checkbox of checkboxInputs) {
						const labelEl = checkbox.closest('div')?.querySelector('label, .ml-1, .flex-fill');
						choices.push({
							input_name: checkbox.name || '',
							input_value: checkbox.value || '',
							text: extractTextWithLatex(labelEl),
							selected: checkbox.checked,
							images: extractImages(labelEl)
						});
					}
				}

				if (choices.length > 0) {
					questions.push({ type: questionType, question_text: questionText, choices: choices, images: questionImages });
				}
			}

			return JSON.stringify(questions);
		})()
	"#;

	let result = page.evaluate(parse_script).await.map_err(|e| eyre!("Failed to parse questions: {}", e))?;
	let json_str = result.value().and_then(|v| v.as_str()).unwrap_or("[]");
	let parsed: Vec<serde_json::Value> = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse JSON: {}", e))?;

	let mut questions = Vec::new();

	for item in parsed {
		let question_text = item["question_text"].as_str().unwrap_or("").to_string();
		let question_type = item["type"].as_str().unwrap_or("SingleChoice");
		let images_json = item["images"].as_array();

		let images: Vec<Image> = images_json
			.map(|arr| {
				arr.iter()
					.map(|img| Image {
						url: img["url"].as_str().unwrap_or("").to_string(),
						alt: img["alt"].as_str().map(|s| s.to_string()),
					})
					.collect()
			})
			.unwrap_or_default();

		match question_type {
			"FillInBlanks" => {
				let segments_json = item["segments"].as_array();
				let blanks_json = item["blanks"].as_array();

				if let (Some(segs_arr), Some(blanks_arr)) = (segments_json, blanks_json) {
					let segments: Vec<FillSegment> = segs_arr
						.iter()
						.filter_map(|seg| {
							let seg_type = seg["type"].as_str()?;
							match seg_type {
								"text" => Some(FillSegment::Text(seg["text"].as_str().unwrap_or("").to_string())),
								"blank" => Some(FillSegment::Blank(seg["index"].as_u64().unwrap_or(0) as usize)),
								_ => None,
							}
						})
						.collect();

					let blanks: Vec<Blank> = blanks_arr
						.iter()
						.filter_map(|b| {
							let blank_type = b["type"].as_str()?;
							match blank_type {
								"text" => Some(Blank::Text {
									input_name: b["input_name"].as_str().unwrap_or("").to_string(),
									current_value: b["current_value"].as_str().unwrap_or("").to_string(),
								}),
								"select" => {
									let options: Vec<MatchOption> = b["options"]
										.as_array()
										.map(|arr| {
											arr.iter()
												.map(|opt| MatchOption {
													value: opt["value"].as_str().unwrap_or("").to_string(),
													text: opt["text"].as_str().unwrap_or("").to_string(),
												})
												.collect()
										})
										.unwrap_or_default();
									Some(Blank::Select {
										select_name: b["select_name"].as_str().unwrap_or("").to_string(),
										options,
										selected_value: b["selected_value"].as_str().unwrap_or("").to_string(),
									})
								}
								_ => None,
							}
						})
						.collect();

					questions.push(Question::FillInBlanks(FillInBlanks {
						question_text,
						segments,
						blanks,
						images,
					}));
				}
			}
			"ShortAnswer" => {
				let input_name = item["input_name"].as_str().unwrap_or("").to_string();
				let current_answer = item["current_answer"].as_str().unwrap_or("").to_string();
				questions.push(Question::ShortAnswer {
					question_text,
					input_name,
					current_answer,
					images,
				});
			}
			"Matching" => {
				let items_json = item["items"].as_array();
				if let Some(items_arr) = items_json {
					let items: Vec<MatchItem> = items_arr
						.iter()
						.map(|it| {
							let options: Vec<MatchOption> = it["options"]
								.as_array()
								.map(|arr| {
									arr.iter()
										.map(|opt| MatchOption {
											value: opt["value"].as_str().unwrap_or("").to_string(),
											text: opt["text"].as_str().unwrap_or("").to_string(),
										})
										.collect()
								})
								.unwrap_or_default();

							MatchItem {
								prompt: it["prompt"].as_str().unwrap_or("").to_string(),
								select_name: it["select_name"].as_str().unwrap_or("").to_string(),
								options,
								selected_value: it["selected_value"].as_str().unwrap_or("0").to_string(),
							}
						})
						.collect();

					questions.push(Question::Matching { question_text, items, images });
				}
			}
			"CodeBlock" => {
				let input_name = item["input_name"].as_str().unwrap_or("").to_string();
				let language = item["language"].as_str().unwrap_or("text").to_string();
				let current_code = item["current_code"].as_str().unwrap_or("").to_string();
				questions.push(Question::CodeBlock {
					question_text,
					input_name,
					language,
					current_code,
					images,
				});
			}
			_ => {
				let choices_json = item["choices"].as_array();
				if let Some(choices_arr) = choices_json {
					let choices: Vec<Choice> = choices_arr
						.iter()
						.map(|c| {
							let choice_images: Vec<Image> = c["images"]
								.as_array()
								.map(|arr| {
									arr.iter()
										.map(|img| Image {
											url: img["url"].as_str().unwrap_or("").to_string(),
											alt: img["alt"].as_str().map(|s| s.to_string()),
										})
										.collect()
								})
								.unwrap_or_default();

							Choice {
								input_name: c["input_name"].as_str().unwrap_or("").to_string(),
								input_value: c["input_value"].as_str().unwrap_or("").to_string(),
								text: c["text"].as_str().unwrap_or("").to_string(),
								selected: c["selected"].as_bool().unwrap_or(false),
								images: choice_images,
							}
						})
						.collect();

					let question = match question_type {
						"MultiChoice" => Question::MultiChoice { question_text, choices, images },
						_ => Question::SingleChoice { question_text, choices, images },
					};
					questions.push(question);
				}
			}
		}
	}

	Ok(questions)
}

/// Toggle an answer by clicking the input (select or deselect)
async fn toggle_answer(page: &Page, input_name: &str, input_value: &str) -> Result<()> {
	let script = format!(
		r#"
		(function() {{
			const input = document.querySelector('input[name="{}"][value="{}"]');
			if (input) {{ input.click(); return true; }}
			return false;
		}})()
		"#,
		input_name, input_value
	);

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to select answer: {}", e))?;

	if result.value().and_then(|v| v.as_bool()) != Some(true) {
		return Err(eyre!("Failed to find input element"));
	}

	Ok(())
}

/// Set a text answer in an input field
async fn set_text_answer(page: &Page, input_name: &str, answer: &str) -> Result<()> {
	// Escape special characters for JavaScript string
	let escaped_answer = answer.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r");

	let script = format!(
		r#"
		(function() {{
			const input = document.querySelector('input[name="{}"]');
			if (input) {{
				input.value = "{}";
				input.dispatchEvent(new Event('input', {{ bubbles: true }}));
				input.dispatchEvent(new Event('change', {{ bubbles: true }}));
				return true;
			}}
			return false;
		}})()
		"#,
		input_name, escaped_answer
	);

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to set text answer: {}", e))?;

	if result.value().and_then(|v| v.as_bool()) != Some(true) {
		return Err(eyre!("Failed to find text input element"));
	}

	Ok(())
}

/// Set a select dropdown value
async fn set_select_value(page: &Page, select_name: &str, value: &str) -> Result<()> {
	let script = format!(
		r#"
		(function() {{
			const select = document.querySelector('select[name="{}"]');
			if (select) {{
				select.value = "{}";
				select.dispatchEvent(new Event('change', {{ bubbles: true }}));
				return true;
			}}
			return false;
		}})()
		"#,
		select_name, value
	);

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to set select value: {}", e))?;

	if result.value().and_then(|v| v.as_bool()) != Some(true) {
		return Err(eyre!("Failed to find select element: {}", select_name));
	}

	Ok(())
}

/// Set code in a code editor (ACE editor or textarea with code-editor role)
async fn set_code_editor_content(page: &Page, input_name: &str, code: &str) -> Result<()> {
	// Escape special characters for JavaScript string
	let escaped_code = code
		.replace('\\', "\\\\")
		.replace('`', "\\`")
		.replace('$', "\\$")
		.replace('\n', "\\n")
		.replace('\r', "\\r")
		.replace('\t', "\\t");

	let script = format!(
		r#"
		(function() {{
			const inputName = "{}";
			const code = `{}`;

			// Find the textarea with this name
			const textarea = document.querySelector('textarea[name="' + inputName + '"]');
			if (!textarea) return false;

			// Try ACE editor first - look for editor instance
			if (typeof ace !== 'undefined') {{
				// Find the ACE editor container (usually a sibling or parent)
				const container = textarea.closest('.qvpl-editor-menu')?.parentElement ||
				                  textarea.closest('.formulation');
				if (container) {{
					const aceEditors = container.querySelectorAll('.ace_editor');
					for (const editorEl of aceEditors) {{
						const editor = ace.edit(editorEl);
						if (editor) {{
							editor.setValue(code, -1);
							// Also update the hidden textarea for form submission
							textarea.value = code;
							textarea.dispatchEvent(new Event('input', {{ bubbles: true }}));
							textarea.dispatchEvent(new Event('change', {{ bubbles: true }}));
							return true;
						}}
					}}
				}}
			}}

			// Fallback: set textarea value directly
			textarea.value = code;
			textarea.dispatchEvent(new Event('input', {{ bubbles: true }}));
			textarea.dispatchEvent(new Event('change', {{ bubbles: true }}));
			return true;
		}})()
		"#,
		input_name, escaped_code
	);

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to set code editor content: {}", e))?;

	if result.value().and_then(|v| v.as_bool()) != Some(true) {
		return Err(eyre!("Failed to find code editor element"));
	}

	Ok(())
}

/// Click the submit/next button on the quiz page
async fn click_submit(page: &Page) -> Result<()> {
	let script = r#"
		(function() {
			const selectors = [
				'input[type="submit"][name="next"]',
				'input[type="submit"]',
				'button[type="submit"]',
				'.submitbtns input[type="submit"]',
				'#responseform input[type="submit"]'
			];

			for (const selector of selectors) {
				const btn = document.querySelector(selector);
				if (btn) { btn.click(); return true; }
			}
			return false;
		})()
	"#;

	let result = page.evaluate(script).await.map_err(|e| eyre!("Failed to click submit: {}", e))?;

	if result.value().and_then(|v| v.as_bool()) != Some(true) {
		return Err(eyre!("Failed to find submit button"));
	}

	// Wait for page to process submission
	tokio::time::sleep(std::time::Duration::from_secs(2)).await;

	Ok(())
}

/// Wait for the page URL to change (indicating form submission)
async fn wait_for_page_change(page: &Page) -> Result<()> {
	let initial_url = page.url().await.map_err(|e| eyre!("Failed to get URL: {}", e))?;

	loop {
		tokio::time::sleep(std::time::Duration::from_millis(500)).await;

		let current_url = page.url().await.map_err(|e| eyre!("Failed to get URL: {}", e))?;

		if current_url != initial_url {
			// Wait a bit for page to fully load
			tokio::time::sleep(std::time::Duration::from_secs(1)).await;
			return Ok(());
		}
	}
}

/// Display an image in terminal using chafa
async fn display_image_chafa(page: &Page, url: &str, max_cols: u32) -> Result<()> {
	use std::process::Stdio;

	use tokio::process::Command;

	let fetch_script = format!(
		r#"
		(async function() {{
			try {{
				const response = await fetch("{}");
				if (!response.ok) return null;
				const blob = await response.blob();
				return new Promise((resolve) => {{
					const reader = new FileReader();
					reader.onloadend = () => resolve(reader.result);
					reader.readAsDataURL(blob);
				}});
			}} catch (e) {{ return null; }}
		}})()
		"#,
		url
	);

	let result = page.evaluate(fetch_script).await.map_err(|e| eyre!("Failed to fetch image via browser: {}", e))?;
	let data_url = result.value().and_then(|v| v.as_str()).ok_or_else(|| eyre!("Failed to fetch image: browser returned null"))?;
	let base64_data = data_url.split(",").nth(1).ok_or_else(|| eyre!("Invalid data URL format"))?;

	use base64::Engine;
	let bytes = base64::engine::general_purpose::STANDARD
		.decode(base64_data)
		.map_err(|e| eyre!("Failed to decode base64: {}", e))?;

	let temp_path = format!("/tmp/quiz_img_{}.tmp", std::process::id());
	tokio::fs::write(&temp_path, &bytes).await.map_err(|e| eyre!("Failed to write temp file: {}", e))?;

	let output = Command::new("chafa")
		.arg("--size")
		.arg(format!("{}x", max_cols))
		.arg(&temp_path)
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.output()
		.await
		.map_err(|e| eyre!("Failed to run chafa: {}", e))?;

	let _ = tokio::fs::remove_file(&temp_path).await;

	if output.status.success() {
		print!("{}", String::from_utf8_lossy(&output.stdout));
	} else {
		return Err(eyre!("chafa failed: {}", String::from_utf8_lossy(&output.stderr)));
	}

	Ok(())
}

/// Parse a VPL page to extract the code submission question
pub async fn parse_vpl_page(page: &Page) -> Result<Option<Question>> {
	let parse_script = r#"
		(function() {
			function extractImages(element) {
				if (!element) return [];
				const images = [];
				const imgElements = element.querySelectorAll('img');
				for (const img of imgElements) {
					const url = img.src || '';
					if (url) images.push({ url: url, alt: img.alt || null });
				}
				return images;
			}

			const urlParams = new URLSearchParams(window.location.search);
			const moduleId = urlParams.get('id') || '';

			let description = '';
			let images = [];
			const requiredFiles = [];

			const walkAndExtract = (node) => {
				let desc = '';
				if (node.nodeType === Node.TEXT_NODE) {
					desc += node.textContent;
				} else if (node.nodeType === Node.ELEMENT_NODE) {
					const tag = node.tagName.toLowerCase();
					if (tag === 'p') { desc += '\n\n'; for (const child of node.childNodes) desc += walkAndExtract(child); }
					else if (tag === 'br') { desc += '\n'; }
					else if (tag === 'li') { desc += '\n• '; for (const child of node.childNodes) desc += walkAndExtract(child); }
					else if (tag === 'ol' || tag === 'ul') { for (const child of node.childNodes) desc += walkAndExtract(child); }
					else if (tag === 'code') { desc += '`' + node.textContent + '`'; }
					else if (tag === 'span') {
						const style = node.getAttribute('style') || '';
						if (style.includes('courier') || style.includes('monospace')) desc += '`' + node.textContent + '`';
						else for (const child of node.childNodes) desc += walkAndExtract(child);
					}
					else if (tag === 'em' || tag === 'i') { desc += '_'; for (const child of node.childNodes) desc += walkAndExtract(child); desc += '_'; }
					else if (tag === 'strong' || tag === 'b') { desc += '**'; for (const child of node.childNodes) desc += walkAndExtract(child); desc += '**'; }
					else if (tag === 'div' && node.classList.contains('editor-indent')) { desc += '\n'; for (const child of node.childNodes) desc += walkAndExtract(child); }
					else { for (const child of node.childNodes) desc += walkAndExtract(child); }
				}
				return desc;
			};

			const generalBoxes = document.querySelectorAll('.generalbox');
			for (const box of generalBoxes) {
				const noOverflow = box.querySelector('.no-overflow');
				if (!noOverflow) continue;
				if (noOverflow.textContent.includes('Work state summary')) continue;
				const text = noOverflow.textContent.trim();
				if (text.length < 50) continue;
				if (text.includes('Responsable de la matière')) continue;

				const clone = noOverflow.cloneNode(true);
				const toRemove = clone.querySelectorAll('script, style, .ace_editor, pre[id^="codefile"]');
				for (const el of toRemove) el.remove();

				let desc = '';
				for (const child of clone.childNodes) desc += walkAndExtract(child);
				desc = desc.trim().replace(/\n{3,}/g, '\n\n');

				if (desc.length > 50) { description = desc; images = extractImages(noOverflow); break; }
			}

			if (!description) {
				const noOverflowDivs = document.querySelectorAll('.no-overflow');
				for (const div of noOverflowDivs) {
					if (div.textContent.includes('Work state summary')) continue;
					const text = div.textContent.trim();
					if (text.length < 100) continue;
					if (text.includes('Responsable de la matière')) continue;

					const clone = div.cloneNode(true);
					const toRemove = clone.querySelectorAll('script, style, .ace_editor, pre[id^="codefile"]');
					for (const el of toRemove) el.remove();

					let desc = '';
					for (const child of clone.childNodes) desc += walkAndExtract(child);
					desc = desc.trim().replace(/\n{3,}/g, '\n\n');

					if (desc.length > 50) { description = desc; images = extractImages(div); break; }
				}
			}

			const h4Elements = document.querySelectorAll('h4[id^="fileid"]');
			for (const h4 of h4Elements) {
				const fileName = h4.textContent.trim();
				if (!fileName) continue;

				const preId = 'code' + h4.id;
				const preElement = document.getElementById(preId);

				let fileContent = '';
				if (preElement) {
					const aceLines = preElement.querySelectorAll('.ace_line');
					if (aceLines.length > 0) {
						const lines = [];
						for (const line of aceLines) lines.push(line.textContent);
						fileContent = lines.join('\n');
					}
				}

				requiredFiles.push({ name: fileName, content: fileContent.trim() });
			}

			if (requiredFiles.length === 0) {
				const allPres = document.querySelectorAll('pre.ace_editor');
				for (const pre of allPres) {
					const aceLines = pre.querySelectorAll('.ace_line');
					if (aceLines.length > 0) {
						const lines = [];
						for (const line of aceLines) lines.push(line.textContent);
						const content = lines.join('\n');
						if (content.includes('# Ecrivez') || content.includes('if __name__')) {
							requiredFiles.push({ name: 'student.py', content: content.trim() });
							break;
						}
					}
				}
			}

			if (!description && requiredFiles.length === 0) return null;

			return JSON.stringify({ type: 'CodeSubmission', description: description, required_files: requiredFiles, module_id: moduleId, images: images });
		})()
	"#;

	let result = page.evaluate(parse_script).await.map_err(|e| eyre!("Failed to parse VPL page: {}", e))?;

	let json_str = match result.value().and_then(|v| v.as_str()) {
		Some(s) => s,
		None => return Ok(None),
	};

	let parsed: serde_json::Value = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse VPL JSON: {}", e))?;

	let description = parsed["description"].as_str().unwrap_or("").to_string();
	let module_id = parsed["module_id"].as_str().unwrap_or("").to_string();

	let images: Vec<Image> = parsed["images"]
		.as_array()
		.map(|arr| {
			arr.iter()
				.map(|img| Image {
					url: img["url"].as_str().unwrap_or("").to_string(),
					alt: img["alt"].as_str().map(|s| s.to_string()),
				})
				.collect()
		})
		.unwrap_or_default();

	let required_files: Vec<RequiredFile> = parsed["required_files"]
		.as_array()
		.map(|arr| {
			arr.iter()
				.map(|f| RequiredFile {
					name: f["name"].as_str().unwrap_or("").to_string(),
					content: f["content"].as_str().unwrap_or("").to_string(),
				})
				.collect()
		})
		.unwrap_or_default();

	Ok(Some(Question::CodeSubmission {
		description,
		required_files,
		module_id,
		images,
	}))
}

/// Save the current page's HTML to disk for debugging
/// Uses the page URL as the filename label
#[cfg(feature = "xdg")]
pub async fn save_page_html(page: &Page, session_id: &str) -> Result<PathBuf> {
	let html_dir = xdg_state_dir!("persist_htmls").join(session_id);
	std::fs::create_dir_all(&html_dir).map_err(|e| eyre!("Failed to create HTML dir: {}", e))?;

	let url = page.url().await.ok().flatten().unwrap_or_default();
	let label = url.replace("https://", "").replace("http://", "");

	let html = page.evaluate("document.documentElement.outerHTML").await.map_err(|e| eyre!("Failed to get page HTML: {}", e))?;
	let html_str = html.value().and_then(|v| v.as_str()).unwrap_or("<html></html>");

	let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
	let safe_label: String = label.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();

	let filename = format!("{}_{}.html", timestamp, safe_label);
	let filepath = html_dir.join(&filename);

	std::fs::write(&filepath, html_str).map_err(|e| eyre!("Failed to write HTML file: {}", e))?;

	log!("Saved page HTML to: {}", filepath.display());
	Ok(filepath)
}
