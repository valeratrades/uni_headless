# Uni Headless - Moodle Login Automation

Automated Moodle login and navigation tool built with Rust and chromiumoxide.

## Features

- Automated login to Moodle (https://moodle2025.uca.fr)
- Login verification by checking for user menu elements
- Automatic navigation to target quiz/course pages
- Headless mode for automation or visible mode for debugging
- Configurable credentials and target URLs via CLI

## Prerequisites

- Rust toolchain (1.93+ recommended)
- Chrome/Chromium browser installed on your system

## Building

```bash
cargo build --release
```

## Usage

### Basic usage (headless mode):
```bash
./target/release/uni_headless \
  --username YOUR_USERNAME \
  --password YOUR_PASSWORD \
  --target-url "https://moodle2025.uca.fr/mod/quiz/attempt.php?attempt=XXXXX&cmid=XXXXX&page=X"
```

### With visible browser (to see what's happening):
```bash
./target/release/uni_headless \
  --visible \
  --username YOUR_USERNAME \
  --password YOUR_PASSWORD \
  --target-url "https://moodle2025.uca.fr/course/view.php?id=123"
```

## CLI Options

- `-v, --visible` - Run with visible browser window (non-headless mode)
- `-u, --username <USERNAME>` - Username for Moodle login (required)
- `-p, --password <PASSWORD>` - Password for Moodle login (required)
- `-t, --target-url <TARGET_URL>` - Target URL to navigate to after login (required)
- `-h, --help` - Print help information

## How it works

1. Launches a Chromium browser instance (headless or visible based on flag)
2. Navigates to Moodle main page
3. Finds and fills the username field
4. Finds and fills the password field
5. Submits the login form
6. Verifies login success by checking for user menu elements
7. Navigates to the specified target URL
8. In visible mode, keeps browser open until Ctrl+C is pressed

## Troubleshooting

If the login fails:
1. Run with `--visible` flag to see what's happening
2. Check if the Moodle login page structure has changed
3. Verify your credentials are correct
4. Check your internet connection

## Technology Stack

- **chromiumoxide** - Rust library for controlling Chrome/Chromium (similar to Puppeteer for Node.js)
- **tokio** - Async runtime
- **clap** - Command-line argument parsing
- **color-eyre** - Better error handling and reporting
