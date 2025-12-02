use ask_llm::{Client as LlmClient, Conversation, Model, Role};
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
};
use v_utils::{
	clientside, elog,
	io::{ConfirmAllResult, confirm_all},
	log,
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

	// Determine which site we're working with based on target URL
	let is_caseine = args.target_url.contains("caseine.org");
	let base_url = if is_caseine { "https://moodle.caseine.org/" } else { "https://moodle2025.uca.fr/" };

	log!("Detected site: {}", if is_caseine { "caseine.org" } else { "moodle2025.uca.fr" });

	// Create a new page and navigate directly to the login site
	let page = browser.new_page(base_url).await.map_err(|e| eyre!("Failed to create new page: {}", e))?;

	// Wait for page to load
	tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

	log!("Looking for login elements...");

	// Check if we need to click a login button first
	let login_button_exists = page.find_element("a[href*='login'], button:has-text('Log in'), a:has-text('Log in')").await.is_ok();

	if login_button_exists {
		log!("Clicking login button...");
		if let Ok(login_btn) = page.find_element("a[href*='login']").await {
			login_btn.click().await.map_err(|e| eyre!("Failed to click login button: {}", e))?;
			tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
		}
	}

	// Handle caseine.org OAuth flow
	if is_caseine {
		log!("Handling caseine.org OAuth flow...");

		// Look for "Autres comptes universitaires" button
		tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

		let oauth_script = r#"
			(function() {
				// Find the "Autres comptes universitaires" button
				const buttons = Array.from(document.querySelectorAll('button, a, div[role="button"]'));
				const oauthButton = buttons.find(btn =>
					btn.textContent.includes('Autres comptes universitaires') ||
					btn.textContent.includes('autres comptes')
				);

				if (oauthButton) {
					oauthButton.click();
					return true;
				}
				return false;
			})()
		"#;

		log!("Clicking 'Autres comptes universitaires'...");
		page.evaluate(oauth_script).await.map_err(|e| eyre!("Failed to click OAuth button: {e}"))?;

		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

		// Type in the university name in the dropdown
		log!("Typing university name in dropdown...");
		let dropdown_script = r#"
			(function() {
				// Find and focus the search input
				const searchInput = document.querySelector('input[type="text"], input[placeholder*="Search"], input[role="searchbox"]');
				if (searchInput) {
					searchInput.focus();
					searchInput.value = "Université Clermont Auvergne";

					// Trigger input event to make dropdown appear
					const event = new Event('input', { bubbles: true });
					searchInput.dispatchEvent(event);
					return true;
				}
				return false;
			})()
		"#;

		page.evaluate(dropdown_script)
			.await
			.map_err(|e| color_eyre::eyre::eyre!("Failed to interact with dropdown: {}", e))?;

		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

		// Click on the "Select" or university option
		log!("Selecting university from dropdown...");
		let select_script = r#"
			(function() {
				// Look for the selection button or the university option
				const options = Array.from(document.querySelectorAll('button, a, div[role="option"], li'));
				const selectButton = options.find(opt =>
					opt.textContent.includes('Université Clermont Auvergne') ||
					opt.textContent.includes('Select')
				);

				if (selectButton) {
					selectButton.click();
					return true;
				}
				return false;
			})()
		"#;

		page.evaluate(select_script).await.map_err(|e| color_eyre::eyre::eyre!("Failed to select university: {}", e))?;

		tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

		log!("OAuth provider selected, waiting for redirect to UCA login...");
	}

	// Wait for username field and fill it using JavaScript for reliability
	log!("Waiting for username field...");

	// Use JavaScript to fill the form (more reliable than typing)
	let fill_script = format!(
		r#"
		(function() {{
			const usernameField = document.querySelector('input[name="username"], input[id="username"]');
			const passwordField = document.querySelector('input[name="password"], input[id="password"], input[type="password"]');

			if (usernameField && passwordField) {{
				usernameField.value = "{}";
				passwordField.value = "{}";
				return true;
			}}
			return false;
		}})()
		"#,
		config.username, config.password
	);

	log!("Filling login form...");
	let _result = page.evaluate(fill_script).await.map_err(|e| color_eyre::eyre::eyre!("Failed to evaluate fill script: {}", e))?;

	log!("Form filled successfully");

	// Submit the form via JavaScript
	log!("Submitting login form...");
	let submit_script = r#"
		(function() {
			const submitButton = document.querySelector('button[type="submit"], input[type="submit"]');
			if (submitButton) {
				submitButton.click();
				return true;
			}
			// Try to submit the form directly
			const form = document.querySelector('form');
			if (form) {
				form.submit();
				return true;
			}
			return false;
		})()
	"#;

	page.evaluate(submit_script).await.map_err(|e| color_eyre::eyre::eyre!("Failed to submit form: {}", e))?;

	// Wait for login to complete
	log!("Waiting for login to complete...");
	tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

	// Verify login by checking URL or looking for logout button
	let current_url = page.url().await.map_err(|e| color_eyre::eyre::eyre!("Failed to get current URL: {}", e))?;

	log!("Current URL after login: {:?}", current_url);

	// Check if login was successful by looking for user menu or logout link
	let logout_exists = page.find_element("a[href*='logout'], .usermenu, #user-menu-toggle").await.is_ok();

	if logout_exists {
		log!("Login successful! User menu found.");
	} else {
		elog!("Warning: Could not verify login success. User menu not found.");
	}

	// Navigate to target URL
	log!("Navigating to target URL: {}", args.target_url);
	page.goto(&args.target_url)
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to navigate to target URL: {}", e))?;

	// Wait for the quiz page to load
	tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

	let final_url = page.url().await.map_err(|e| color_eyre::eyre::eyre!("Failed to get final URL: {}", e))?;

	log!("Successfully navigated to: {:?}", final_url);

	// Check if this is a VPL (code submission) page
	if is_vpl_url(&args.target_url) {
		log!("Detected VPL (Virtual Programming Lab) page");
		handle_vpl_page(&page, args.ask_llm).await?;
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
async fn handle_vpl_page(page: &chromiumoxide::Page, ask_llm: bool) -> Result<()> {
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
	match ask_llm_for_code(&question).await {
		Ok(files) => {
			eprintln!("\nGenerated code:");
			for (filename, content) in &files {
				eprintln!("\n=== {} ===", filename);
				eprintln!("{}", content);
			}
			eprintln!();

			// For now, just display the code - submission will be implemented in next phase
			log!("Code generated. Manual submission required for now.");
		}
		Err(e) => {
			elog!("Failed to generate code: {}", e);
		}
	}

	Ok(())
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

/// Fetch an image via the browser and return its base64 data and media type
async fn fetch_image_as_base64(page: &chromiumoxide::Page, url: &str) -> Result<(String, String)> {
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
						// reader.result is "data:image/png;base64,XXXX..."
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

/// Ask the LLM to answer a question
async fn ask_llm_for_answer(page: &chromiumoxide::Page, question: &Question) -> Result<LlmAnswerResult> {
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
		let answer: LlmMultiAnswer = serde_json::from_str(json_str).map_err(|e| color_eyre::eyre::eyre!("Failed to parse LLM JSON response: {} - raw: '{}'", e, json_str))?;

		// Validate all indices
		for &num in &answer.response_numbers {
			if num == 0 || num > choices.len() {
				return Err(color_eyre::eyre::eyre!("LLM returned invalid answer index: {} (expected 1-{})", num, choices.len()));
			}
		}

		let indices: Vec<usize> = answer.response_numbers.iter().map(|n| n - 1).collect();
		Ok(LlmAnswerResult::Multi { indices, texts: answer.responses })
	} else {
		let answer: LlmSingleAnswer = serde_json::from_str(json_str).map_err(|e| color_eyre::eyre::eyre!("Failed to parse LLM JSON response: {} - raw: '{}'", e, json_str))?;

		if answer.response_number == 0 || answer.response_number > choices.len() {
			return Err(color_eyre::eyre::eyre!(
				"LLM returned invalid answer index: {} (expected 1-{})",
				answer.response_number,
				choices.len()
			));
		}

		Ok(LlmAnswerResult::Single {
			idx: answer.response_number - 1,
			text: answer.response,
		})
	}
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

			// Helper function to extract text with HTML preserved for code blocks
			function extractDescription(element) {
				if (!element) return '';

				// Clone to avoid modifying DOM
				const clone = element.cloneNode(true);

				// Convert <pre> and <code> tags to preserve formatting
				const codeBlocks = clone.querySelectorAll('pre, code');
				for (const block of codeBlocks) {
					// Keep code content as-is with markers
					const content = block.textContent;
					const marker = block.tagName === 'PRE' ? '\n```\n' : '`';
					block.replaceWith(document.createTextNode(marker + content + marker));
				}

				// Convert <br> to newlines
				const brs = clone.querySelectorAll('br');
				for (const br of brs) {
					br.replaceWith(document.createTextNode('\n'));
				}

				// Convert <p> to double newlines
				const ps = clone.querySelectorAll('p');
				for (const p of ps) {
					const text = p.textContent;
					p.replaceWith(document.createTextNode('\n\n' + text));
				}

				// Convert lists
				const lis = clone.querySelectorAll('li');
				for (const li of lis) {
					const text = li.textContent;
					li.replaceWith(document.createTextNode('\n• ' + text));
				}

				return clone.textContent.trim();
			}

			// Get module ID from URL
			const urlParams = new URLSearchParams(window.location.search);
			const moduleId = urlParams.get('id') || '';

			// Find the description - VPL uses various containers
			let description = '';
			let images = [];

			// Try different description selectors used by VPL
			const descSelectors = [
				'.box.generalbox .no-overflow',
				'.box.generalbox',
				'#intro',
				'.activity-description',
				'[role="main"] .box'
			];

			for (const selector of descSelectors) {
				const descEl = document.querySelector(selector);
				if (descEl && descEl.textContent.trim().length > 50) {
					description = extractDescription(descEl);
					images = extractImages(descEl);
					break;
				}
			}

			// If no description found, try the full page content area
			if (!description) {
				const mainContent = document.querySelector('[role="main"]');
				if (mainContent) {
					description = extractDescription(mainContent);
					images = extractImages(mainContent);
				}
			}

			// Find required files section
			const requiredFiles = [];

			// Look for "Requested files" or "Required files" section
			const headers = document.querySelectorAll('h3, h4, .card-header, legend');
			for (const header of headers) {
				const text = header.textContent.toLowerCase();
				if (text.includes('requested') || text.includes('required') || text.includes('fichiers')) {
					// Find the associated file list
					let container = header.nextElementSibling || header.parentElement;
					if (container) {
						// Look for file items - could be in a list or table
						const fileItems = container.querySelectorAll('li, tr, .file-item, span[title]');
						for (const item of fileItems) {
							const fileName = item.textContent.trim();
							if (fileName && fileName.includes('.')) {
								requiredFiles.push({
									name: fileName,
									content: ''
								});
							}
						}
					}
					break;
				}
			}

			// Also check for files in the VPL file manager area
			const fileManagerFiles = document.querySelectorAll('.vpl_ide_file, .vpl-file-name, [data-filename]');
			for (const file of fileManagerFiles) {
				const fileName = file.getAttribute('data-filename') || file.textContent.trim();
				if (fileName && !requiredFiles.find(f => f.name === fileName)) {
					requiredFiles.push({
						name: fileName,
						content: ''
					});
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

/// Ask the LLM to generate code for a VPL submission
async fn ask_llm_for_code(question: &Question) -> Result<Vec<(String, String)>> {
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

Make sure the code is complete, correct, and ready to submit. Include all necessary imports/includes."#
	);

	let mut conv = Conversation::new();
	conv.add(Role::User, prompt);

	let client = LlmClient::new().model(Model::Medium).max_tokens(4096).force_json();

	let response = client.conversation(&conv).await?;

	tracing::debug!("LLM code response: {}", response.text);

	let json_str = response.text.trim();
	let answer: LlmCodeAnswer = serde_json::from_str(json_str).map_err(|e| eyre!("Failed to parse LLM code response: {} - raw: '{}'", e, json_str))?;

	Ok(answer.files.into_iter().map(|f| (f.filename, f.content)).collect())
}
