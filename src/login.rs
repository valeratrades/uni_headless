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
pub async fn login_and_navigate(page: &Page, site: Site, target_url: &str, config: &AppConfig, semi_manual_login: bool) -> Result<()> {
	match site {
		Site::Caseine => login_caseine(page, target_url, config, semi_manual_login).await,
		Site::UcaMoodle => login_uca_moodle(page, target_url, config, semi_manual_login).await,
	}
}

/// Known page types during login flow
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoginPage {
	/// Already at target/VPL page
	Target,
	/// Enrollment page with Continue button
	Enrollment,
	/// Caseine login page
	CaseineLogin,
	/// University federation selection page
	FederationWayf,
	/// UCA CAS login form
	UcaCas,
	/// SAML consent page
	SamlConsent,
	/// Unknown/unexpected page
	Unknown,
}

fn detect_login_page(url: &str, target_url: &str) -> LoginPage {
	let target_base = target_url.split('?').next().unwrap_or(target_url);
	let url_base = url.split('?').next().unwrap_or(url);

	if url_base == target_base {
		return LoginPage::Target;
	}
	if url.contains("/mod/vpl/") && !url.contains("login") && !url.contains("enrol") {
		return LoginPage::Target;
	}
	if url.contains("enrol/index.php") {
		return LoginPage::Enrollment;
	}
	if url.contains("moodle.caseine.org/login/index.php") {
		return LoginPage::CaseineLogin;
	}
	if url.contains("discovery.renater.fr") || url.contains("wayf") {
		return LoginPage::FederationWayf;
	}
	if url.contains("ent.uca.fr/cas") {
		return LoginPage::UcaCas;
	}
	if url.contains("idp.uca.fr") {
		return LoginPage::SamlConsent;
	}
	LoginPage::Unknown
}

/// Login flow for caseine.org
/// Goes directly to target URL, handles enrollment redirect, then OAuth login
async fn login_caseine(page: &Page, target_url: &str, config: &AppConfig, semi_manual_login: bool) -> Result<()> {
	loop {
		let current_url = page.url().await.ok().flatten().unwrap_or_default();
		let page_type = detect_login_page(&current_url, target_url);

		match page_type {
			LoginPage::Target => {
				log!("Reached target page");
				return Ok(());
			}
			LoginPage::Enrollment => {
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
			LoginPage::CaseineLogin => {
				log!("On login page, clicking login button...");
				page.evaluate(r#"document.querySelector('a.btn:nth-child(3)').click()"#)
					.await
					.map_err(|e| eyre!("Failed to click login button: {}", e))?;
				tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
			}
			LoginPage::FederationWayf => {
				log!("Selecting university from dropdown...");
				page.wait_for_navigation().await.map_err(|e| eyre!("Failed waiting for federation page: {}", e))?;
				tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
				select_university_from_dropdown(page).await?;
			}
			LoginPage::UcaCas => {
				log!("Filling CAS login form...");
				tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
				fill_and_submit_login_form(page, config).await?;
				tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
			}
			LoginPage::SamlConsent => {
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
			LoginPage::Unknown => {
				if semi_manual_login {
					log!("Unknown page detected at: {}", current_url);
					log!("Waiting for manual intervention (e.g., enter access password)...");
					tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
					// Loop will continue, checking if page changed
				} else {
					return Err(eyre!("Login failed: unexpected page at {}", current_url));
				}
			}
		}
	}
}

/// Known page types during UCA Moodle login flow
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UcaMoodlePage {
	/// Home page (need to click login)
	Home,
	/// Login page with form
	LoginForm,
	/// Logged in (has logout link)
	LoggedIn,
	/// Target page reached
	Target,
	/// Unknown/unexpected page
	Unknown,
}

fn detect_uca_moodle_page(url: &str, target_url: &str, has_logout: bool) -> UcaMoodlePage {
	let target_base = target_url.split('?').next().unwrap_or(target_url);
	let url_base = url.split('?').next().unwrap_or(url);

	if url_base == target_base {
		return UcaMoodlePage::Target;
	}
	if url.contains("/login/") {
		return UcaMoodlePage::LoginForm;
	}
	if has_logout {
		return UcaMoodlePage::LoggedIn;
	}
	if url.contains("moodle2025.uca.fr") && !url.contains("/mod/") {
		return UcaMoodlePage::Home;
	}
	UcaMoodlePage::Unknown
}

/// Login flow for moodle2025.uca.fr
/// Standard Moodle login with username/password
async fn login_uca_moodle(page: &Page, target_url: &str, config: &AppConfig, semi_manual_login: bool) -> Result<()> {
	loop {
		let current_url = page.url().await.ok().flatten().unwrap_or_default();
		let has_logout = page.find_element("a[href*='logout'], .usermenu, #user-menu-toggle").await.is_ok();
		let page_type = detect_uca_moodle_page(&current_url, target_url, has_logout);

		match page_type {
			UcaMoodlePage::Target => {
				log!("Reached target page");
				return Ok(());
			}
			UcaMoodlePage::Home => {
				log!("On home page, clicking login...");
				let login_script = r#"
					(function() {
						const loginBtn = document.querySelector('a[href*="login"]');
						if (loginBtn) {
							loginBtn.click();
							return true;
						}
						return false;
					})()
				"#;
				page.evaluate(login_script).await.ok();
				tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
			}
			UcaMoodlePage::LoginForm => {
				log!("Filling login form...");
				fill_and_submit_login_form(page, config).await?;
				tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
			}
			UcaMoodlePage::LoggedIn => {
				log!("Login successful! Navigating to target URL: {}", target_url);
				page.goto(target_url).await.map_err(|e| eyre!("Failed to navigate to target: {}", e))?;
				tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
			}
			UcaMoodlePage::Unknown => {
				if semi_manual_login {
					log!("Unknown page detected at: {}", current_url);
					log!("Waiting for manual intervention (e.g., enter access password)...");
					tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
					// Loop will continue, checking if page changed
				} else {
					return Err(eyre!("Login failed: unexpected page at {}", current_url));
				}
			}
		}
	}
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
