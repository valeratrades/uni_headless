use std::sync::atomic::{AtomicUsize, Ordering};

use chromiumoxide::browser::{Browser, BrowserConfig};
use clap::Parser;
use color_eyre::{Result, eyre::eyre};
use futures::StreamExt;
#[cfg(feature = "xdg")]
use uni_headless::runner::save_page_html;
use uni_headless::{
	config::{AppConfig, SettingsFlags},
	is_vpl_url,
	login::{Site, login_and_navigate},
	runner::{handle_quiz_page, handle_vpl_page},
};
#[cfg(feature = "xdg")]
use v_utils::xdg_state_dir;
use v_utils::{clientside, elog, log};

#[derive(Debug, Parser)]
#[command(name = "uni_headless")]
#[command(about = "Automated Moodle login and navigation", long_about = None)]
struct Args {
	/// Target URL to navigate to after login
	target_url: String,

	/// Additional URLs to process after the first one succeeds (for VPL: only if 100% grade)
	#[arg(short = 'd', long = "do-after")]
	do_after: Vec<String>,

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
	#[cfg(feature = "xdg")]
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
		BrowserConfig::builder().with_head().build().map_err(|e| eyre!("Failed to build browser config: {e}"))?
	} else {
		BrowserConfig::builder().build().map_err(|e| eyre!("Failed to build browser config: {e}"))?
	};

	// Launch browser
	let (mut browser, mut handler) = Browser::launch(browser_config).await.map_err(|e| eyre!("Failed to launch browser: {}", e))?;

	// Spawn a task to handle browser events
	let handle = tokio::spawn(async move {
		while let Some(_event) = handler.next().await {
			// Silently consume events
		}
	});

	// Build URL queue: first the target, then do_after URLs
	let mut urls: Vec<String> = vec![args.target_url.clone()];
	urls.extend(args.do_after.clone());

	// Process URLs
	let mut processing_error: Option<color_eyre::Report> = None;

	for (idx, target_url) in urls.iter().enumerate() {
		if idx > 0 {
			log!("\n========== Processing next URL ({}/{}) ==========", idx + 1, urls.len());
		}

		match process_url(&mut browser, target_url, &mut config, args.ask_llm, args.debug_from_html).await {
			Ok((success, _page)) => {
				// For VPL pages, only continue to next URL if we got 100%
				if is_vpl_url(target_url) && !success {
					log!("Stopping - did not get perfect grade on VPL");
					break;
				}
			}
			Err(e) => {
				// Error HTML is saved in process_url
				processing_error = Some(e);
				break;
			}
		}
	}

	// If there was an error and visible mode, keep browser open for debugging
	if let Some(ref err) = processing_error {
		if args.visible {
			elog!("Error occurred: {}", err);
			log!("Keeping browser open for debugging. Press Ctrl+C to exit...");

			static SIGINT_COUNT: AtomicUsize = AtomicUsize::new(0);

			unsafe {
				libc::signal(libc::SIGINT, sigint_handler_err as *const () as libc::sighandler_t);
			}

			extern "C" fn sigint_handler_err(_: libc::c_int) {
				std::process::exit(130);
			}

			while SIGINT_COUNT.load(Ordering::SeqCst) == 0 {
				tokio::time::sleep(std::time::Duration::from_millis(100)).await;
			}

			handle.abort();
			let _ = tokio::time::timeout(std::time::Duration::from_secs(2), browser.close()).await;
		} else {
			handle.abort();
			let _ = tokio::time::timeout(std::time::Duration::from_secs(2), browser.close()).await;
		}

		return Err(processing_error.unwrap());
	}

	// Keep browser open in visible mode
	if args.visible {
		log!("Browser is visible. Press Ctrl+C to exit...");

		static SIGINT_COUNT: AtomicUsize = AtomicUsize::new(0);

		unsafe {
			libc::signal(libc::SIGINT, sigint_handler as *const () as libc::sighandler_t);
		}

		extern "C" fn sigint_handler(_: libc::c_int) {
			let count = SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
			if count >= 1 {
				std::process::exit(130);
			}
		}

		while SIGINT_COUNT.load(Ordering::SeqCst) == 0 {
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}

		log!("Shutting down... (press Ctrl+C again to force exit)");
		handle.abort();
		let _ = tokio::time::timeout(std::time::Duration::from_secs(2), browser.close()).await;
	} else {
		tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
		log!("Task completed successfully!");
		handle.abort();
		let _ = tokio::time::timeout(std::time::Duration::from_secs(2), browser.close()).await;
	}

	Ok(())
}

/// Process a single URL - returns (success, page) where success indicates if VPL got 100%
async fn process_url(browser: &mut Browser, target_url: &str, config: &mut AppConfig, ask_llm: bool, debug_from_html: bool) -> Result<(bool, chromiumoxide::Page)> {
	// Create/navigate to page
	let page = if debug_from_html {
		let file_url = format!("file://{}", target_url);
		log!("Debug mode: opening local file {}", file_url);
		let page = browser.new_page(&file_url).await.map_err(|e| eyre!("Failed to open file: {}", e))?;
		tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
		page
	} else {
		let site = Site::detect(target_url);
		log!("Detected site: {}", site.name());

		let start_url = match site {
			Site::Caseine => target_url.to_string(),
			Site::UcaMoodle => "https://moodle2025.uca.fr/".to_string(),
		};

		let page = browser.new_page(&start_url).await.map_err(|e| eyre!("Failed to create new page: {}", e))?;
		page.wait_for_navigation().await.map_err(|e| eyre!("Failed waiting for initial page load: {}", e))?;

		login_and_navigate(&page, site, target_url, config).await?;
		page
	};

	let final_url = page.url().await.map_err(|e| eyre!("Failed to get final URL: {}", e))?;
	log!("Successfully navigated to: {:?}", final_url);

	// Save the page HTML for debugging
	#[cfg(feature = "xdg")]
	{
		let url_label = final_url.as_deref().unwrap_or("unknown").replace("https://", "").replace("http://", "");
		if let Err(e) = save_page_html(&page, &url_label).await {
			elog!("Failed to save page HTML: {}", e);
		}
	}

	// Check if this is a VPL page
	let is_vpl = if debug_from_html {
		target_url.contains("vpl") || target_url.contains("VPL")
	} else {
		is_vpl_url(target_url)
	};

	let result = if is_vpl {
		log!("Detected VPL (Virtual Programming Lab) page");
		handle_vpl_page(&page, ask_llm, config).await
	} else {
		handle_quiz_page(&page, ask_llm, config).await.map(|_| true) // Quiz pages don't have a "success" metric
	};

	match result {
		Ok(success) => Ok((success, page)),
		Err(e) => {
			// Save error page HTML before returning error
			#[cfg(feature = "xdg")]
			if let Err(save_err) = save_page_html(&page, "errored_on").await {
				elog!("Failed to save error page HTML: {}", save_err);
			}
			Err(e)
		}
	}
}
