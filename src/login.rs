use chromiumoxide::Page;
use color_eyre::{Result, eyre::eyre};
use v_utils::log;

use crate::config::AppConfig;

/// Detected site type
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Site {
	Caseine,
	UcaMoodle,
}

impl Site {
	pub fn detect(url: &str) -> Self {
		if url.contains("caseine.org") { Site::Caseine } else { Site::UcaMoodle }
	}

	pub fn name(&self) -> &'static str {
		match self {
			Site::Caseine => "caseine.org",
			Site::UcaMoodle => "moodle2025.uca.fr",
		}
	}
}

/// Perform login for the detected site and navigate to target URL
pub async fn login_and_navigate(page: &Page, site: Site, target_url: &str, config: &AppConfig) -> Result<()> {
	match site {
		Site::Caseine => login_caseine(page, target_url, config).await,
		Site::UcaMoodle => login_uca_moodle(page, target_url, config).await,
	}
}

/// Login flow for caseine.org
/// Goes directly to target URL, handles enrollment redirect, then OAuth login
async fn login_caseine(page: &Page, target_url: &str, config: &AppConfig) -> Result<()> {
	let current_url = page.url().await.ok().flatten().unwrap_or_default();

	// Check if already logged in (landed on target or VPL page)
	if current_url.contains("/mod/vpl/") && !current_url.contains("login") && !current_url.contains("enrol") {
		log!("Already logged in, at target page");
		return Ok(());
	}

	// Step 1: If on enrollment page, click Continue
	if current_url.contains("enrol/index.php") {
		log!("On enrollment page, clicking Continue...");
		page.evaluate(
			r#"
			(function() {
				const buttons = document.querySelectorAll('button, input[type="submit"], a.btn');
				for (const btn of buttons) {
					const text = btn.textContent || btn.value || '';
					if (text.trim() === 'Continue' || text.trim() === 'Continuer') {
						btn.click();
						return true;
					}
				}
				return false;
			})()
		"#,
		)
		.await
		.map_err(|e| eyre!("Failed to click Continue: {}", e))?;
		tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
	}

	// Step 2: If on login page, click the federation login button
	let current_url = page.url().await.ok().flatten().unwrap_or_default();
	if current_url.contains("moodle.caseine.org/login/index.php") {
		log!("On login page, clicking login button...");
		page.evaluate(r#"document.querySelector('a.btn:nth-child(3)').click()"#)
			.await
			.map_err(|e| eyre!("Failed to click login button: {}", e))?;
		tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
	}

	// Step 3: Select university from dropdown (if on federation page)
	let current_url = page.url().await.ok().flatten().unwrap_or_default();
	if current_url.contains("discovery.renater.fr") || current_url.contains("wayf") {
		log!("Selecting university from dropdown...");
		page.wait_for_navigation().await.map_err(|e| eyre!("Failed waiting for federation page: {}", e))?;
		tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
		select_university_from_dropdown(page).await?;
	}

	// Step 4: Fill UCA CAS login form (if on CAS page)
	let current_url = page.url().await.ok().flatten().unwrap_or_default();
	if current_url.contains("ent.uca.fr/cas") {
		log!("Filling CAS login form...");
		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
		fill_and_submit_login_form(page, config).await?;
		tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
	}

	// Step 5: Click "Accept" button on SAML consent page (if present)
	let current_url = page.url().await.ok().flatten().unwrap_or_default();
	if current_url.contains("idp.uca.fr") {
		log!("On SAML consent page, clicking Accept...");
		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
		page.evaluate(
			r#"
			(function() {
				const btn = document.querySelector('input[name="_eventId_proceed"]');
				if (btn) btn.click();
			})()
		"#,
		)
		.await
		.ok();
		tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
	}

	let final_url = page.url().await.ok().flatten().unwrap_or_default();
	log!("Login complete, now at: {}", final_url);

	// Check we ended up at the target (compare base path, ignoring query params)
	let target_base = target_url.split('?').next().unwrap_or(target_url);
	let final_base = final_url.split('?').next().unwrap_or(&final_url);
	if final_base != target_base {
		return Err(eyre!("Login failed: expected to be at {}, but at {}", target_url, final_url));
	}

	Ok(())
}

/// Login flow for moodle2025.uca.fr
/// Navigated to target URL, gets redirected to CAS login, fills form, gets redirected back to target
async fn login_uca_moodle(page: &Page, target_url: &str, config: &AppConfig) -> Result<()> {
	let current_url = page.url().await.ok().flatten().unwrap_or_default();

	// Check if already at target (already logged in)
	let target_base = target_url.split('?').next().unwrap_or(target_url);
	let current_base = current_url.split('?').next().unwrap_or(&current_url);
	if current_base == target_base {
		log!("Already logged in, at target page");
		return Ok(());
	}

	// Handle CAS login (ent.uca.fr/cas)
	if current_url.contains("ent.uca.fr/cas") {
		log!("On CAS login page, filling form...");
		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
		fill_and_submit_login_form(page, config).await?;
		tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
	}

	// After login, should be redirected back to target
	let final_url = page.url().await.ok().flatten().unwrap_or_default();
	let final_base = final_url.split('?').next().unwrap_or(&final_url);

	if final_base == target_base {
		log!("Login successful, at target page");
	} else {
		return Err(eyre!("Login failed: expected to be at {}, but at {}", target_url, final_url));
	}

	Ok(())
}

/// Select "Université Clermont Auvergne" from the federation dropdown
async fn select_university_from_dropdown(page: &Page) -> Result<()> {
	// Open the select2 dropdown using jQuery API
	let open_script = r#"
		(function() {
			if (typeof $ !== 'undefined') {
				$('select').select2('open');
				return 'opened';
			}
			return 'jquery not found';
		})()
	"#;
	page.evaluate(open_script).await.map_err(|e| eyre!("Failed to open dropdown: {}", e))?;
	tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

	// Type in the search field
	let type_script = r#"
		(function() {
			const searchInput = document.querySelector('input.select2-search__field');
			if (searchInput) {
				searchInput.focus();
				searchInput.value = "Université Clermont Auvergne";
				searchInput.dispatchEvent(new Event('input', { bubbles: true }));
				return 'typed';
			}
			return 'search field not found';
		})()
	"#;
	page.evaluate(type_script).await.map_err(|e| eyre!("Failed to type: {}", e))?;
	tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

	// Press Enter to select the option
	page.evaluate(r#"document.querySelector('input.select2-search__field').dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', keyCode: 13, bubbles: true}))"#)
		.await
		.map_err(|e| eyre!("Failed to press Enter: {}", e))?;
	tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

	// Click the "Select" button
	let btn_result = page
		.evaluate(
			r#"
		(function() {
			const btns = document.querySelectorAll('button, input[type="submit"]');
			for (const btn of btns) {
				const text = (btn.textContent || btn.value || '').toLowerCase();
				if (text.includes('select') || text.includes('sélectionner')) {
					btn.click();
					return 'clicked: ' + text;
				}
			}
			// Fallback: click any button
			if (btns.length > 0) {
				btns[0].click();
				return 'clicked first button';
			}
			return 'no button found';
		})()
	"#,
		)
		.await
		.map_err(|e| eyre!("Failed to click Select button: {}", e))?;
	log!("Select button result: {:?}", btn_result.value());
	tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

	Ok(())
}

/// Fill username/password and submit the login form
async fn fill_and_submit_login_form(page: &Page, config: &AppConfig) -> Result<()> {
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
	page.evaluate(fill_script).await.map_err(|e| eyre!("Failed to fill login form: {}", e))?;

	// Submit
	let submit_script = r#"
		(function() {
			const submitButton = document.querySelector('button[type="submit"], input[type="submit"]');
			if (submitButton) {
				submitButton.click();
				return true;
			}
			const form = document.querySelector('form');
			if (form) {
				form.submit();
				return true;
			}
			return false;
		})()
	"#;
	page.evaluate(submit_script).await.map_err(|e| eyre!("Failed to submit login form: {}", e))?;

	Ok(())
}
