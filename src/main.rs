use ask_llm::{Conversation, Model, Role};
use chromiumoxide::browser::{Browser, BrowserConfig};
use clap::Parser;
use color_eyre::Result;
use futures::StreamExt;
use uni_headless::{
	Choice, Question,
	config::{AppConfig, SettingsFlags},
};
use v_utils::{clientside, elog, log};

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
	let config = AppConfig::try_build(args.settings)?;

	log!("Starting Moodle login automation...");
	log!("Visible mode: {}", args.visible);

	// Configure browser based on visibility flag
	let browser_config = if args.visible {
		BrowserConfig::builder()
			.with_head() // Visible browser with UI
			.build()
			.map_err(|e| color_eyre::eyre::eyre!("Failed to build browser config: {}", e))?
	} else {
		BrowserConfig::builder()
			.build() // Headless mode
			.map_err(|e| color_eyre::eyre::eyre!("Failed to build browser config: {}", e))?
	};

	// Launch browser
	let (mut browser, mut handler) = Browser::launch(browser_config).await.map_err(|e| color_eyre::eyre::eyre!("Failed to launch browser: {}", e))?;

	// Spawn a task to handle browser events (suppress errors as they're mostly noise)
	let handle = tokio::spawn(async move {
		while let Some(_event) = handler.next().await {
			// Silently consume events to prevent the browser from hanging
		}
	});

	// Create a new page
	let page = browser.new_page("about:blank").await.map_err(|e| color_eyre::eyre::eyre!("Failed to create new page: {}", e))?;

	log!("Navigating to target URL...");

	// Determine which site we're working with based on target URL
	let is_caseine = args.target_url.contains("caseine.org");
	let base_url = if is_caseine { "https://moodle.caseine.org/" } else { "https://moodle2025.uca.fr/" };

	log!("Detected site: {}", if is_caseine { "caseine.org" } else { "moodle2025.uca.fr" });

	// Navigate to the site
	page.goto(base_url).await.map_err(|e| color_eyre::eyre::eyre!("Failed to navigate: {}", e))?;

	// Wait for page to load
	tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

	log!("Looking for login elements...");

	// Check if we need to click a login button first
	let login_button_exists = page.find_element("a[href*='login'], button:has-text('Log in'), a:has-text('Log in')").await.is_ok();

	if login_button_exists {
		log!("Clicking login button...");
		if let Ok(login_btn) = page.find_element("a[href*='login']").await {
			login_btn.click().await.map_err(|e| color_eyre::eyre::eyre!("Failed to click login button: {}", e))?;
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
		page.evaluate(oauth_script).await.map_err(|e| color_eyre::eyre::eyre!("Failed to click OAuth button: {}", e))?;

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

	// Parse questions from the page
	log!("Parsing questions from the page...");
	let questions = parse_questions(&page).await?;

	for (i, question) in questions.iter().enumerate() {
		log!("--- Question {} ---", i + 1);
		log!("Text: {}", question.question_text());
		if let Some(choices) = question.choices() {
			for (j, choice) in choices.iter().enumerate() {
				let selected_marker = if choice.selected { " [SELECTED]" } else { "" };
				log!("  {}. {}{}", j + 1, choice.text, selected_marker);
			}
		}

		if args.ask_llm {
			match ask_llm_for_answer(question).await {
				Ok((answer_idx, answer_text)) => {
					log!("Selected: {}. {}", answer_idx + 1, answer_text);
				}
				Err(e) => {
					elog!("Failed to get LLM answer: {}", e);
				}
			}
		}
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

/// Parse multiple choice questions from the quiz page
async fn parse_questions(page: &chromiumoxide::Page) -> Result<Vec<Question>> {
	let parse_script = r#"
		(function() {
			const questions = [];

			// Find all question formulations
			const formulations = document.querySelectorAll('.formulation.clearfix');

			for (const formulation of formulations) {
				// Get the question text from .qtext
				const qtextEl = formulation.querySelector('.qtext');
				const questionText = qtextEl ? qtextEl.innerText.trim() : '';

				// Find all answer options (radio buttons for single-choice)
				const answerDiv = formulation.querySelector('.answer');
				if (!answerDiv) continue;

				const choices = [];
				const radioInputs = answerDiv.querySelectorAll('input[type="radio"]');

				for (const radio of radioInputs) {
					// Get the label text for this radio
					const labelEl = radio.closest('div')?.querySelector('label, .ml-1, .flex-fill');
					const labelText = labelEl ? labelEl.innerText.trim() : '';

					choices.push({
						input_name: radio.name || '',
						input_value: radio.value || '',
						text: labelText,
						selected: radio.checked
					});
				}

				if (choices.length > 0) {
					questions.push({
						type: 'MultiChoice',
						question_text: questionText,
						choices: choices
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
		let choices_json = item["choices"].as_array();

		if let Some(choices_arr) = choices_json {
			let choices: Vec<Choice> = choices_arr
				.iter()
				.map(|c| Choice {
					input_name: c["input_name"].as_str().unwrap_or("").to_string(),
					input_value: c["input_value"].as_str().unwrap_or("").to_string(),
					text: c["text"].as_str().unwrap_or("").to_string(),
					selected: c["selected"].as_bool().unwrap_or(false),
				})
				.collect();

			questions.push(Question::MultiChoice { question_text, choices });
		}
	}

	Ok(questions)
}

/// LLM response structure for multi-choice questions
#[derive(Debug, serde::Deserialize)]
struct LlmAnswer {
	response: String,
	response_number: usize,
}

/// Ask the LLM to answer a multi-choice question
async fn ask_llm_for_answer(question: &Question) -> Result<(usize, String)> {
	let Question::MultiChoice { question_text, choices } = question;

	let mut options_text = String::new();
	for (i, choice) in choices.iter().enumerate() {
		options_text.push_str(&format!("{}. {}\n", i + 1, choice.text));
	}

	let prompt = format!(
		r#"You are answering a multiple-choice question. Pick the correct answer.

Question:
{question_text}

Options:
{options_text}
Respond with JSON only, no markdown, in this exact format:
{{"response": "<the text of the correct answer>", "response_number": <the number of the correct answer>}}"#
	);

	let mut conv = Conversation::new();
	conv.add(Role::User, prompt);

	let response = ask_llm::conversation(&conv, Model::Medium, Some(128), None).await?;

	tracing::debug!("LLM raw response: {}", response.text);

	let answer: LlmAnswer = serde_json::from_str(response.text.trim()).map_err(|e| color_eyre::eyre::eyre!("Failed to parse LLM JSON response: {} - raw: '{}'", e, response.text))?;

	if answer.response_number == 0 || answer.response_number > choices.len() {
		return Err(color_eyre::eyre::eyre!(
			"LLM returned invalid answer index: {} (expected 1-{})",
			answer.response_number,
			choices.len()
		));
	}

	Ok((answer.response_number - 1, answer.response))
}
