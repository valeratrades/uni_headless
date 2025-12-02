use std::path::PathBuf;

use chromiumoxide::browser::{Browser, BrowserConfig};
use clap::Parser;
use color_eyre::{
	Result,
	eyre::{bail, eyre},
};
use futures::StreamExt;
use uni_headless::{
	Choice, Image, Question, RequiredFile,
	config::{AppConfig, SettingsFlags},
	is_vpl_url,
	llm::{LlmAnswerResult, ask_llm_for_answer, ask_llm_for_code},
	login::{Site, login_and_navigate},
};
use v_utils::{
	clientside, elog,
	io::{ConfirmAllResult, confirm, confirm_all},
	log, xdg_state_dir,
};

#[derive(Debug, Parser)]
#[command(name = "uni_headless")]
#[command(about = "Automated Moodle login and navigation", long_about = None)]
struct Args {
	/// Target URL to navigate to after login
	target_url: String,

	/// Run with visible browser window (non-headless mode)
	#[arg(long)]
	visible: bool,

	/// Use LLM to answer multi-choice questions
	#[arg(short, long)]
	ask_llm: bool,

	/// Debug mode: interpret target_url as path to local HTML file (skips browser)
	#[arg(long)]
	debug_from_html: bool,

	#[command(flatten)]
	settings: SettingsFlags,
}

#[tokio::main]
async fn main() -> Result<()> {
	clientside!();
	let args = Args::parse();
	let mut config = AppConfig::try_build(args.settings)?;

	log!("Starting Moodle login automation...");
	log!("Visible mode: {}", args.visible);

	// Clean up old HTML logs on startup (unless in debug mode)
	if !args.debug_from_html {
		let html_dir = xdg_state_dir!("persist_htmls");
		if html_dir.exists() {
			if let Err(e) = std::fs::remove_dir_all(&html_dir) {
				elog!("Failed to clean HTML logs: {}", e);
			}
		}
	}

	// Configure browser based on visibility flag
	let browser_config = if args.visible {
		BrowserConfig::builder()
			.with_head() // Visible browser with UI
			.build()
			.map_err(|e| eyre!("Failed to build browser config: {e}"))?
	} else {
		BrowserConfig::builder()
			.build() // Headless mode
			.map_err(|e| eyre!("Failed to build browser config: {e}"))?
	};

	// Launch browser
	let (mut browser, mut handler) = Browser::launch(browser_config).await.map_err(|e| eyre!("Failed to launch browser: {}", e))?;

	// Spawn a task to handle browser events (suppress errors as they're mostly noise)
	let handle = tokio::spawn(async move {
		while let Some(_event) = handler.next().await {
			// Silently consume events to prevent the browser from hanging
		}
	});

	// Debug mode: open local HTML file directly, skip login
	let page = if args.debug_from_html {
		let file_url = format!("file://{}", args.target_url);
		log!("Debug mode: opening local file {}", file_url);
		let page = browser.new_page(&file_url).await.map_err(|e| eyre!("Failed to open file: {}", e))?;
		tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
		page
	} else {
		// Determine which site we're working with
		let site = Site::detect(&args.target_url);
		log!("Detected site: {}", site.name());

		// Create page - for caseine go directly to target, for UCA go to base
		let start_url = match site {
			Site::Caseine => args.target_url.clone(),
			Site::UcaMoodle => "https://moodle2025.uca.fr/".to_string(),
		};

		let page = browser.new_page(&start_url).await.map_err(|e| eyre!("Failed to create new page: {}", e))?;
		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

		// Perform site-specific login and navigate to target
		login_and_navigate(&page, site, &args.target_url, &config).await?;
		page
	};

	let final_url = page.url().await.map_err(|e| eyre!("Failed to get final URL: {}", e))?;
	log!("Successfully navigated to: {:?}", final_url);

	// Save the page HTML for debugging
	let url_label = final_url.as_deref().unwrap_or("unknown").replace("https://", "").replace("http://", "");
	if let Err(e) = save_page_html(&page, &url_label).await {
		elog!("Failed to save page HTML: {}", e);
	}

	// Check if this is a VPL (code submission) page
	// In debug mode, check the file path; otherwise check the URL
	let is_vpl = if args.debug_from_html {
		args.target_url.contains("vpl") || args.target_url.contains("VPL")
	} else {
		is_vpl_url(&args.target_url)
	};

	if is_vpl {
		log!("Detected VPL (Virtual Programming Lab) page");
		handle_vpl_page(&page, args.ask_llm, &mut config).await?;
	} else {
		// Parse and answer questions in a loop (quiz mode)
		handle_quiz_page(&page, args.ask_llm, &mut config).await?;
	}

	// Keep browser open in visible mode
	if args.visible {
		log!("Browser is visible. Press Ctrl+C to exit...");
		tokio::signal::ctrl_c().await?;
	} else {
		// In headless mode, wait a bit to ensure page is fully loaded
		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
		log!("Task completed successfully!");
	}

	// Clean up
	drop(page);
	browser.close().await.map_err(|e| color_eyre::eyre::eyre!("Failed to close browser: {}", e))?;
	drop(browser);
	handle.abort();

	Ok(())
}

/// Handle a VPL (Virtual Programming Lab) code submission page
async fn handle_vpl_page(page: &chromiumoxide::Page, ask_llm: bool, config: &mut AppConfig) -> Result<()> {
	let question = parse_vpl_page(page).await?;

	let Some(question) = question else {
		log!("No VPL question found on this page.");
		return Ok(());
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
		return Ok(());
	}

	// Ask LLM to generate code
	log!("Asking LLM to generate code solution...");
	let files = match ask_llm_for_code(&question).await {
		Ok(files) => {
			eprintln!("\nGenerated code:");
			for (filename, content) in &files {
				eprintln!("\n=== {} ===", filename);
				eprintln!("{}", content);
			}
			eprintln!();
			files
		}
		Err(e) => {
			elog!("Failed to generate code: {}", e);
			return Ok(());
		}
	};

	if files.is_empty() {
		elog!("No code files generated");
		return Ok(());
	}

	// Ask for confirmation before pasting (skip if auto_submit is enabled)
	if !config.auto_submit && !confirm("Paste generated code into editor?").await {
		log!("Cancelled by user");
		return Ok(());
	}

	// Navigate to the Edit page
	log!("Navigating to VPL editor...");
	if !click_vpl_edit_button(page).await? {
		elog!("Could not find Edit button on VPL page");
		return Ok(());
	}

	// Wait for editor to load
	tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

	// Save the editor page HTML
	if let Err(e) = save_page_html(page, "vpl_editor").await {
		elog!("Failed to save editor page HTML: {}", e);
	}

	log!("Pasting code into editor...");
	for (filename, content) in &files {
		// Prepend empty line - VPL panics without it
		let content = format!("\n{}", content);
		if let Err(e) = set_vpl_file_content(page, filename, &content).await {
			elog!("Failed to set content for {}: {}", filename, e);
		}
	}

	log!("Saving code...");
	tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
	if !click_vpl_button(page, "save").await? {
		bail!("Could not find Save button - aborting");
	}
	tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

	log!("Running evaluation...");
	if !click_vpl_button(page, "evaluate").await? {
		bail!("Could not find Evaluate button - aborting");
	}

	log!("Waiting for evaluation results...");
	tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

	let eval_result = parse_vpl_evaluation_result(page).await?;
	if let Some(result) = eval_result {
		eprintln!("\n=== Evaluation Result ===");
		eprintln!("{}", result);
	} else {
		log!("No evaluation result found (may still be running)");
	}

	Ok(())
}

/// Click the Edit button on a VPL page to open the editor
async fn click_vpl_edit_button(page: &chromiumoxide::Page) -> Result<bool> {
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
/// VPL creates buttons dynamically with IDs like vpl_ide_save, vpl_ide_evaluate
/// Uses chromiumoxide's native click to emulate a real mouse click
async fn click_vpl_button(page: &chromiumoxide::Page, action: &str) -> Result<bool> {
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

/// Set the content of a file in the VPL editor
async fn set_vpl_file_content(page: &chromiumoxide::Page, filename: &str, content: &str) -> Result<()> {
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
			// First, try to find the ACE editor instance
			if (typeof ace !== 'undefined') {{
				// Get all editor instances
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
async fn parse_vpl_evaluation_result(page: &chromiumoxide::Page) -> Result<Option<String>> {
	let script = r#"
		(function() {
			// VPL shows results in various places
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

			// Look for any element containing grade/result info
			const allElements = document.querySelectorAll('*');
			for (const el of allElements) {
				const text = el.textContent;
				if (text && (text.includes('Grade:') || text.includes('Result:') ||
				    text.includes('Passed') || text.includes('Failed') ||
				    text.includes('Score:') || text.includes('Points:'))) {
					// Get just this element's direct text, not children
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

/// Handle a quiz (multi-choice) page
async fn handle_quiz_page(page: &chromiumoxide::Page, ask_llm: bool, config: &mut AppConfig) -> Result<()> {
	let mut question_num = 0;
	let mut consecutive_failures = 0;
	const MAX_CONSECUTIVE_FAILURES: u32 = 5;
	let mut first_page = true;

	loop {
		// Print page separator
		let current_url = page.url().await.ok().flatten().unwrap_or_default();
		let page_num = current_url.split("page=").nth(1).and_then(|s| s.split('&').next()).and_then(|s| s.parse::<u32>().ok());

		if !first_page {
			if let Some(num) = page_num {
				log!("\n==================== Page {} ====================", num);
			} else {
				log!("\n================================================");
			}
		}
		first_page = false;

		let questions = parse_questions(page).await?;

		if questions.is_empty() {
			log!("No more questions found.");
			break;
		}

		// Display all questions on this page
		for (i, question) in questions.iter().enumerate() {
			let type_marker = if question.is_multi() { "[multi]" } else { "[single]" };
			let header = format!("--- Question {} {} ---", question_num + i + 1, type_marker);
			tracing::info!("{}", header);
			eprintln!("{}", header);

			let text = question.question_text();
			tracing::info!("{}", text);
			eprintln!("{}", text);

			// Display question images
			for img in question.images() {
				if let Err(e) = display_image_chafa(page, &img.url, 60).await {
					elog!("Failed to display image: {}", e);
					eprintln!("  [Image: {}]", img.alt.as_deref().unwrap_or(&img.url));
				}
			}

			let choices = question.choices();
			for (j, choice) in choices.iter().enumerate() {
				let selected_marker = if choice.selected { " [SELECTED]" } else { "" };
				let line = format!("  {}. {}{}", j + 1, choice.text, selected_marker);
				tracing::info!("{}", line);
				eprintln!("{}", line);

				// Display choice images (smaller)
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

			match ask_llm_for_answer(page, question).await {
				Ok(answer_result) => {
					consecutive_failures = 0; // Reset on success

					// Collect answer display for later
					let type_marker = if question.is_multi() { "[multi]" } else { "[single]" };
					answer_logs.push(format!("Question {} {} answer:", question_num, type_marker));
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
					}

					answers_to_select.push((question, answer_result));
				}
				Err(e) => {
					consecutive_failures += 1;
					elog!(
						"Failed to get LLM answer for question {}: {} ({}/{})",
						question_num,
						e,
						consecutive_failures,
						MAX_CONSECUTIVE_FAILURES
					);
					if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
						bail!("Exceeded {} consecutive LLM failures", MAX_CONSECUTIVE_FAILURES);
					}
					// Skip this question but continue with others
				}
			}
		}

		// Display all answers at once with newlines around
		if !answer_logs.is_empty() {
			let mut output = String::from("\n");
			for line in &answer_logs {
				tracing::info!("{}", line);
				output.push_str(line);
				output.push('\n');
			}
			output.push('\n');
			print!("{}", output);
		}

		if answers_to_select.is_empty() {
			log!("No answers to submit on this page.");
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
					let choices = question.choices();
					match answer_result {
						LlmAnswerResult::Single { idx, .. } => {
							let choice = &choices[*idx];
							select_answer(page, &choice.input_name, &choice.input_value).await?;
						}
						LlmAnswerResult::Multi { indices, .. } =>
							for idx in indices {
								let choice = &choices[*idx];
								select_answer(page, &choice.input_name, &choice.input_value).await?;
							},
					}
				}
				// Submit once for all questions on this page
				click_submit(page).await?;
				log!("All {} answer(s) submitted!", answers_to_select.len());
			}
			Some(false) => {
				// Already submitted by user, continue to next page
			}
			None => {
				// User said no, wait for them to submit manually
				log!("Waiting for manual submission...");
				wait_for_page_change(page).await?;
				log!("Page changed, continuing...");
			}
		}
	}

	Ok(())
}

/// Parse questions from the quiz page
async fn parse_questions(page: &chromiumoxide::Page) -> Result<Vec<Question>> {
	let parse_script = r#"
		(function() {
			// Helper function to extract images from an element
			function extractImages(element) {
				if (!element) return [];
				const images = [];
				const imgElements = element.querySelectorAll('img');
				for (const img of imgElements) {
					const url = img.src || '';
					if (url) {
						images.push({
							url: url,
							alt: img.alt || null
						});
					}
				}
				return images;
			}

			// Helper function to extract text with LaTeX preserved
			// MathJax renders math and keeps source in annotation or data attributes
			function extractTextWithLatex(element) {
				if (!element) return '';

				// Clone the element to avoid modifying the DOM
				const clone = element.cloneNode(true);

				// Find all MathJax containers and replace with LaTeX source
				// MathJax 3.x uses mjx-container with data attributes
				const mjxContainers = clone.querySelectorAll('mjx-container');
				for (const container of mjxContainers) {
					// Try to get LaTeX from various sources
					let latex = null;

					// Check for assistive MathML with annotation
					const annotation = container.querySelector('annotation[encoding="application/x-tex"]');
					if (annotation) {
						latex = annotation.textContent;
					}

					// Check data attribute (sometimes used)
					if (!latex && container.dataset.latex) {
						latex = container.dataset.latex;
					}

					// Check aria-label which often contains the LaTeX
					if (!latex && container.getAttribute('aria-label')) {
						// aria-label might have the formatted version, not ideal but better than nothing
					}

					// Look for the original script tag with math
					const mathScript = container.querySelector('script[type="math/tex"]');
					if (!latex && mathScript) {
						latex = mathScript.textContent;
					}

					if (latex) {
						// Replace the container with the LaTeX wrapped in \( \) or \[ \]
						const isDisplay = container.getAttribute('display') === 'true' ||
						                  container.classList.contains('MJXc-display');
						const wrapper = isDisplay ? ['\\[', '\\]'] : ['\\(', '\\)'];
						const textNode = document.createTextNode(wrapper[0] + latex + wrapper[1]);
						container.replaceWith(textNode);
					} else {
						// Fallback: just remove the MathJax visual elements to avoid duplication
						// Keep just the accessible text
						const accessibleText = container.querySelector('.MJX_Assistive_MathML, mjx-assistive-mml');
						if (accessibleText) {
							container.replaceWith(document.createTextNode(accessibleText.textContent || ''));
						}
					}
				}

				// Also handle MathJax 2.x style (span.MathJax)
				const mj2Spans = clone.querySelectorAll('.MathJax, .MathJax_Preview, .MathJax_Display');
				for (const span of mj2Spans) {
					// Try to find the script sibling with the source
					const script = span.nextElementSibling;
					if (script && script.tagName === 'SCRIPT' && script.type && script.type.includes('math/tex')) {
						const latex = script.textContent;
						const isDisplay = script.type.includes('mode=display');
						const wrapper = isDisplay ? ['\\[', '\\]'] : ['\\(', '\\)'];
						span.replaceWith(document.createTextNode(wrapper[0] + latex + wrapper[1]));
						script.remove();
					} else {
						// Just remove the preview/duplicate
						span.remove();
					}
				}

				// Remove any remaining script tags with math
				const mathScripts = clone.querySelectorAll('script[type*="math/tex"]');
				for (const script of mathScripts) {
					const latex = script.textContent;
					const isDisplay = script.type.includes('mode=display');
					const wrapper = isDisplay ? ['\\[', '\\]'] : ['\\(', '\\)'];
					script.replaceWith(document.createTextNode(wrapper[0] + latex + wrapper[1]));
				}

				// Get the text content, normalize whitespace
				return clone.textContent.replace(/\s+/g, ' ').trim();
			}

			const questions = [];

			// Find all question formulations
			const formulations = document.querySelectorAll('.formulation.clearfix');

			for (const formulation of formulations) {
				// Get the question text from .qtext
				const qtextEl = formulation.querySelector('.qtext');
				const questionText = extractTextWithLatex(qtextEl);
				const questionImages = extractImages(qtextEl);

				// Find all answer options
				const answerDiv = formulation.querySelector('.answer');
				if (!answerDiv) continue;

				// Check for radio buttons (single choice) or checkboxes (multi choice)
				const radioInputs = answerDiv.querySelectorAll('input[type="radio"]');
				const checkboxInputs = answerDiv.querySelectorAll('input[type="checkbox"]');

				const choices = [];
				let questionType = 'SingleChoice';

				if (radioInputs.length > 0) {
					questionType = 'SingleChoice';
					for (const radio of radioInputs) {
						const labelEl = radio.closest('div')?.querySelector('label, .ml-1, .flex-fill');
						const labelText = extractTextWithLatex(labelEl);
						const choiceImages = extractImages(labelEl);

						choices.push({
							input_name: radio.name || '',
							input_value: radio.value || '',
							text: labelText,
							selected: radio.checked,
							images: choiceImages
						});
					}
				} else if (checkboxInputs.length > 0) {
					questionType = 'MultiChoice';
					for (const checkbox of checkboxInputs) {
						const labelEl = checkbox.closest('div')?.querySelector('label, .ml-1, .flex-fill');
						const labelText = extractTextWithLatex(labelEl);
						const choiceImages = extractImages(labelEl);

						choices.push({
							input_name: checkbox.name || '',
							input_value: checkbox.value || '',
							text: labelText,
							selected: checkbox.checked,
							images: choiceImages
						});
					}
				}

				if (choices.length > 0) {
					questions.push({
						type: questionType,
						question_text: questionText,
						choices: choices,
						images: questionImages
					});
				}
			}

			return JSON.stringify(questions);
		})()
	"#;

	let result = page.evaluate(parse_script).await.map_err(|e| color_eyre::eyre::eyre!("Failed to parse questions: {}", e))?;

	let json_str = result.value().and_then(|v| v.as_str()).unwrap_or("[]");

	// Parse the JSON into our Question structs
	let parsed: Vec<serde_json::Value> = serde_json::from_str(json_str).map_err(|e| color_eyre::eyre::eyre!("Failed to parse JSON: {}", e))?;

	let mut questions = Vec::new();

	for item in parsed {
		let question_text = item["question_text"].as_str().unwrap_or("").to_string();
		let question_type = item["type"].as_str().unwrap_or("SingleChoice");
		let choices_json = item["choices"].as_array();
		let images_json = item["images"].as_array();

		// Parse question images
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

	Ok(questions)
}

/// Select an answer by clicking the input (radio button or checkbox)
async fn select_answer(page: &chromiumoxide::Page, input_name: &str, input_value: &str) -> Result<()> {
	let script = format!(
		r#"
		(function() {{
			const input = document.querySelector('input[name="{}"][value="{}"]');
			if (input) {{
				input.click();
				return true;
			}}
			return false;
		}})()
		"#,
		input_name, input_value
	);

	let result = page.evaluate(script).await.map_err(|e| color_eyre::eyre::eyre!("Failed to select answer: {}", e))?;

	if result.value().and_then(|v| v.as_bool()) != Some(true) {
		return Err(color_eyre::eyre::eyre!("Failed to find input element"));
	}

	Ok(())
}

/// Click the submit button on the quiz page
async fn click_submit(page: &chromiumoxide::Page) -> Result<()> {
	let script = r#"
		(function() {
			// Try common submit button selectors for Moodle
			const selectors = [
				'input[type="submit"][name="next"]',
				'input[type="submit"]',
				'button[type="submit"]',
				'.submitbtns input[type="submit"]',
				'#responseform input[type="submit"]'
			];

			for (const selector of selectors) {
				const btn = document.querySelector(selector);
				if (btn) {
					btn.click();
					return true;
				}
			}
			return false;
		})()
	"#;

	let result = page.evaluate(script).await.map_err(|e| color_eyre::eyre::eyre!("Failed to click submit: {}", e))?;

	if result.value().and_then(|v| v.as_bool()) != Some(true) {
		return Err(color_eyre::eyre::eyre!("Failed to find submit button"));
	}

	// Wait for page to process submission
	tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

	Ok(())
}

/// Display an image in terminal using chafa
/// Uses the browser to fetch the image (to preserve session cookies), then renders with chafa
async fn display_image_chafa(page: &chromiumoxide::Page, url: &str, max_cols: u32) -> Result<()> {
	use std::process::Stdio;

	use tokio::process::Command;

	// Use browser's fetch to get image as base64 (preserves cookies/session)
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
			}} catch (e) {{
				return null;
			}}
		}})()
		"#,
		url
	);

	let result = page.evaluate(fetch_script).await.map_err(|e| eyre!("Failed to fetch image via browser: {}", e))?;

	let data_url = result.value().and_then(|v| v.as_str()).ok_or_else(|| eyre!("Failed to fetch image: browser returned null"))?;

	// Parse data URL: "data:image/png;base64,XXXX..."
	let base64_data = data_url.split(",").nth(1).ok_or_else(|| eyre!("Invalid data URL format"))?;

	// Decode base64
	use base64::Engine;
	let bytes = base64::engine::general_purpose::STANDARD
		.decode(base64_data)
		.map_err(|e| eyre!("Failed to decode base64: {}", e))?;

	// Create temp file
	let temp_path = format!("/tmp/quiz_img_{}.tmp", std::process::id());
	tokio::fs::write(&temp_path, &bytes).await.map_err(|e| eyre!("Failed to write temp file: {}", e))?;

	// Run chafa with size constraint
	let output = Command::new("chafa")
		.arg("--size")
		.arg(format!("{}x", max_cols))
		.arg(&temp_path)
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.output()
		.await
		.map_err(|e| eyre!("Failed to run chafa: {}", e))?;

	// Clean up temp file
	let _ = tokio::fs::remove_file(&temp_path).await;

	if output.status.success() {
		// Print the chafa output directly
		print!("{}", String::from_utf8_lossy(&output.stdout));
	} else {
		let stderr = String::from_utf8_lossy(&output.stderr);
		return Err(eyre!("chafa failed: {}", stderr));
	}

	Ok(())
}

/// Wait for the page URL to change (indicating form submission)
async fn wait_for_page_change(page: &chromiumoxide::Page) -> Result<()> {
	let initial_url = page.url().await.map_err(|e| color_eyre::eyre::eyre!("Failed to get URL: {}", e))?;

	loop {
		tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

		let current_url = page.url().await.map_err(|e| color_eyre::eyre::eyre!("Failed to get URL: {}", e))?;

		if current_url != initial_url {
			// Wait a bit for page to fully load
			tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
			return Ok(());
		}
	}
}

/// Parse a VPL (Virtual Programming Lab) page to extract the code submission question
async fn parse_vpl_page(page: &chromiumoxide::Page) -> Result<Option<Question>> {
	let parse_script = r#"
		(function() {
			// Helper function to extract images from an element
			function extractImages(element) {
				if (!element) return [];
				const images = [];
				const imgElements = element.querySelectorAll('img');
				for (const img of imgElements) {
					const url = img.src || '';
					if (url) {
						images.push({
							url: url,
							alt: img.alt || null
						});
					}
				}
				return images;
			}

			// Get module ID from URL
			const urlParams = new URLSearchParams(window.location.search);
			const moduleId = urlParams.get('id') || '';

			let description = '';
			let images = [];
			const requiredFiles = [];

			// === DESCRIPTION EXTRACTION ===
			// Caseine VPL: The description is in a <div class="no-overflow"> element
			// The correct div is the one whose FIRST CHILD is a <p> containing the exercise text
			// Not the outer container that has "Work state summary" floating inside
			const noOverflowDivs = document.querySelectorAll('.no-overflow');
			for (const div of noOverflowDivs) {
				// Get the first child element
				const firstChild = div.firstElementChild;
				if (!firstChild || firstChild.tagName.toLowerCase() !== 'p') {
					continue;
				}

				// The first child must be a <p> with substantial content (not empty, not just links)
				const firstPText = firstChild.textContent.trim();
				if (firstPText.length < 50) {
					continue;
				}

				// Check if this paragraph contains exercise description text
				if (!firstPText.includes('exercice') && !firstPText.includes('fonction') &&
				    !firstPText.includes('dictionnaire') && !firstPText.includes('Ecrire')) {
					continue;
				}

				// This is our description div
				const text = div.textContent || '';
				if (text.includes('Dans cet exercice') || text.includes('Ecrire une fonction')) {
					// Clone and clean up
					const clone = div.cloneNode(true);

					// Remove script, style, and ACE editor elements
					const toRemove = clone.querySelectorAll('script, style, .ace_editor, pre[id^="codefile"]');
					for (const el of toRemove) {
						el.remove();
					}

					// Build description preserving structure
					let desc = '';
					const walk = (node) => {
						if (node.nodeType === Node.TEXT_NODE) {
							desc += node.textContent;
						} else if (node.nodeType === Node.ELEMENT_NODE) {
							const tag = node.tagName.toLowerCase();
							if (tag === 'p') {
								desc += '\n\n';
								for (const child of node.childNodes) walk(child);
							} else if (tag === 'br') {
								desc += '\n';
							} else if (tag === 'li') {
								desc += '\n• ';
								for (const child of node.childNodes) walk(child);
							} else if (tag === 'ol' || tag === 'ul') {
								for (const child of node.childNodes) walk(child);
							} else if (tag === 'span') {
								// Check for monospace font (code)
								const style = node.getAttribute('style') || '';
								if (style.includes('courier') || style.includes('monospace')) {
									desc += '`' + node.textContent + '`';
								} else {
									for (const child of node.childNodes) walk(child);
								}
							} else if (tag === 'em' || tag === 'i') {
								desc += '_';
								for (const child of node.childNodes) walk(child);
								desc += '_';
							} else if (tag === 'strong' || tag === 'b') {
								desc += '**';
								for (const child of node.childNodes) walk(child);
								desc += '**';
							} else {
								for (const child of node.childNodes) walk(child);
							}
						}
					};

					for (const child of clone.childNodes) walk(child);
					description = desc.trim().replace(/\n{3,}/g, '\n\n');
					images = extractImages(div);
					break;
				}
			}

			// === REQUIRED FILES EXTRACTION ===
			// Caseine VPL: Files are listed after <h2>Required files</h2>
			// Each file has <h4 id="fileid1">student.py</h4> followed by <pre id="codefileid1" class="ace_editor">
			const h4Elements = document.querySelectorAll('h4[id^="fileid"]');
			for (const h4 of h4Elements) {
				const fileName = h4.textContent.trim();
				if (!fileName) continue;

				// Find the corresponding pre element with ACE editor content
				const preId = 'code' + h4.id;
				const preElement = document.getElementById(preId);

				let fileContent = '';
				if (preElement) {
					// ACE editor stores code in .ace_line divs within .ace_text-layer
					const aceLines = preElement.querySelectorAll('.ace_line');
					if (aceLines.length > 0) {
						const lines = [];
						for (const line of aceLines) {
							lines.push(line.textContent);
						}
						fileContent = lines.join('\n');
					}
				}

				requiredFiles.push({
					name: fileName,
					content: fileContent.trim()
				});
			}

			// Fallback: if no files found via h4, try the old method
			if (requiredFiles.length === 0) {
				const allPres = document.querySelectorAll('pre.ace_editor');
				for (const pre of allPres) {
					const aceLines = pre.querySelectorAll('.ace_line');
					if (aceLines.length > 0) {
						const lines = [];
						for (const line of aceLines) {
							lines.push(line.textContent);
						}
						const content = lines.join('\n');
						// Check if it looks like Python
						if (content.includes('# Ecrivez') || content.includes('if __name__')) {
							requiredFiles.push({
								name: 'student.py',
								content: content.trim()
							});
							break;
						}
					}
				}
			}

			if (!description && requiredFiles.length === 0) {
				return null;
			}

			return JSON.stringify({
				type: 'CodeSubmission',
				description: description,
				required_files: requiredFiles,
				module_id: moduleId,
				images: images
			});
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
async fn save_page_html(page: &chromiumoxide::Page, label: &str) -> Result<PathBuf> {
	let html_dir = xdg_state_dir!("persist_htmls");
	std::fs::create_dir_all(&html_dir).map_err(|e| eyre!("Failed to create HTML dir: {}", e))?;

	// Get the page HTML
	let html = page.evaluate("document.documentElement.outerHTML").await.map_err(|e| eyre!("Failed to get page HTML: {}", e))?;

	let html_str = html.value().and_then(|v| v.as_str()).unwrap_or("<html></html>");

	// Create a filename from the label and timestamp
	let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();

	// Sanitize label for filename
	let safe_label: String = label.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();

	let filename = format!("{}_{}.html", timestamp, safe_label);
	let filepath = html_dir.join(&filename);

	std::fs::write(&filepath, html_str).map_err(|e| eyre!("Failed to write HTML file: {}", e))?;

	log!("Saved page HTML to: {}", filepath.display());
	Ok(filepath)
}
