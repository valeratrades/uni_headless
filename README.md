# uni_headless
![Minimum Supported Rust Version](https://img.shields.io/badge/nightly-1.93+-ab6000.svg)
[<img alt="crates.io" src="https://img.shields.io/crates/v/uni_headless.svg?color=fc8d62&logo=rust" height="20" style=flat-square>](https://crates.io/crates/uni_headless)
[<img alt="docs.rs" src="https://img.shields.io/badge/docs.rs-66c2a5?style=for-the-badge&labelColor=555555&logo=docs.rs&style=flat-square" height="20">](https://docs.rs/uni_headless)
![Lines Of Code](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/uni_headless-loc.json)
![Build time](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/uni_headless-build.json)
![Test time](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/uni_headless-test.json)
<br>
[![errors](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/uni_headless-errors.json)](https://github.com/valeratrades/uni_headless/actions/workflows/errors.yml?query=branch%3Amain)
[![warnings](https://img.shields.io/endpoint?url=https://gist.githubusercontent.com/valeratrades/b48e6f02c61942200e7d1e3eeabf9bcb/raw/uni_headless-warnings.json)](https://github.com/valeratrades/uni_headless/actions/workflows/warnings.yml?query=branch%3Amain)

Headless browser automation for Moodle quizzes with optional LLM-powered answer selection.
<!-- markdownlint-disable -->
<details>
<summary>
<h2>Installation</h2>
</summary>

<!-- markdownlint-disable -->
<details>
<summary>
<h3>Windows</h3>
</summary>

### Windows Installation

Run directly without installing (downloads to temp, executes, then cleans up):

```powershell
$t="$env:TEMP\uni_headless.exe"; Invoke-WebRequest -Uri "https://github.com/valeratrades/uni_headless/releases/download/latest-windows/uni_headless.exe" -OutFile $t; & $t; Remove-Item $t
```

Or download first, then run:

```powershell
Invoke-WebRequest -Uri "https://github.com/valeratrades/uni_headless/releases/download/latest-windows/uni_headless.exe" -OutFile "uni_headless.exe"
./uni_headless.exe
```

With curl:

```bash
curl -LO https://github.com/valeratrades/uni_headless/releases/download/latest-windows/uni_headless.exe
./uni_headless.exe
```

To build from source on Windows, use `cargo build --release --no-default-features` to exclude the xdg dependency.

</details>
<!-- markdownlint-restore -->


</details>
<!-- markdownlint-restore -->

## Usage
```sh
# Basic usage - login and navigate to quiz
uni_headless -u <username> -p <password> -t <quiz_url>

# With visible browser window
uni_headless -u <username> -p <password> -t <quiz_url> --visible

# With LLM answering questions
uni_headless -u <username> -p <password> -t <quiz_url> --ask-llm
```



<br>

<sup>Links for devs:</sup>
- <sup><a href="./docs/ARCHITECTURE.md">architecture</a></sup>
- <sup><a href="https://github.com/EV-Main/.github/blob/main/code/STYLE.md">style</a></sup>
- <sup><a href="https://github.com/EV-Main/.github/tree/main/code/conventions">conventions</a></sup>
- <sup><a href="https://github.com/EV-Main/.github/blob/main/code/conventions/git">contributing</a></sup>

#### License

<sup>
	Licensed under <a href="LICENSE">Blue Oak 1.0.0</a>
</sup>

<br>

<sub>
	Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be licensed as above, without any additional terms or conditions.
</sub>

