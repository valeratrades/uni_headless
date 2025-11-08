use chromiumoxide::browser::{Browser, BrowserConfig};
use clap::Parser;
use color_eyre::Result;
use futures::StreamExt;

#[derive(Parser, Debug)]
#[command(name = "uni_headless")]
#[command(about = "Automated Moodle login and navigation", long_about = None)]
struct Args {
	/// Run with visible browser window (non-headless mode)
	#[arg(long)]
	visible: bool,

	/// Username for Moodle login
	#[arg(short, long)]
	username: String,

	/// Password for Moodle login
	#[arg(short, long)]
	password: String,

	/// Target URL to navigate to after login
	#[arg(short, long)]
	target_url: String,
}

#[tokio::main]
async fn main() -> Result<()> {
	color_eyre::install()?;
	let args = Args::parse();

	println!("Starting Moodle login automation...");
	println!("Visible mode: {}", args.visible);

	// Configure browser based on visibility flag
	let config = if args.visible {
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
	let (mut browser, mut handler) = Browser::launch(config)
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to launch browser: {}", e))?;

	// Spawn a task to handle browser events (suppress errors as they're mostly noise)
	let handle = tokio::spawn(async move {
		while let Some(_event) = handler.next().await {
			// Silently consume events to prevent the browser from hanging
		}
	});

	// Create a new page
	let page = browser
		.new_page("about:blank")
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to create new page: {}", e))?;

	println!("Navigating to Moodle login page...");

	// Navigate to Moodle main page
	page.goto("https://moodle2025.uca.fr/")
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to navigate: {}", e))?;

	// Wait for page to load
	tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

	println!("Looking for login elements...");

	// Check if we need to click a login button first
	let login_button_exists = page
		.find_element("a[href*='login'], button:has-text('Log in'), a:has-text('Log in')")
		.await
		.is_ok();

	if login_button_exists {
		println!("Clicking login button...");
		if let Ok(login_btn) = page.find_element("a[href*='login']").await {
			login_btn
				.click()
				.await
				.map_err(|e| color_eyre::eyre::eyre!("Failed to click login button: {}", e))?;
			tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
		}
	}

	// Wait for username field and fill it using JavaScript for reliability
	println!("Waiting for username field...");

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
		args.username, args.password
	);

	println!("Filling login form...");
	let _result = page
		.evaluate(fill_script)
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to evaluate fill script: {}", e))?;

	println!("Form filled successfully");

	// Submit the form via JavaScript
	println!("Submitting login form...");
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

	page.evaluate(submit_script)
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to submit form: {}", e))?;

	// Wait for login to complete
	println!("Waiting for login to complete...");
	tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

	// Verify login by checking URL or looking for logout button
	let current_url = page
		.url()
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to get current URL: {}", e))?;

	println!("Current URL after login: {:?}", current_url);

	// Check if login was successful by looking for user menu or logout link
	let logout_exists = page
		.find_element("a[href*='logout'], .usermenu, #user-menu-toggle")
		.await
		.is_ok();

	if logout_exists {
		println!("✓ Login successful! User menu found.");
	} else {
		println!("⚠ Warning: Could not verify login success. User menu not found.");
	}

	// Navigate to target URL
	println!("Navigating to target URL: {}", args.target_url);
	page.goto(&args.target_url)
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to navigate to target URL: {}", e))?;

	// Wait for the quiz page to load
	tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

	let final_url = page
		.url()
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to get final URL: {}", e))?;

	println!("✓ Successfully navigated to: {:?}", final_url);

	// Keep browser open in visible mode
	if args.visible {
		println!("\nBrowser is visible. Press Ctrl+C to exit...");
		tokio::signal::ctrl_c().await?;
	} else {
		// In headless mode, wait a bit to ensure page is fully loaded
		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
		println!("✓ Task completed successfully!");
	}

	// Clean up
	drop(page);
	browser
		.close()
		.await
		.map_err(|e| color_eyre::eyre::eyre!("Failed to close browser: {}", e))?;
	drop(browser);
	handle.abort();

	Ok(())
}
