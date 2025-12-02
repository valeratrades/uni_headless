## Windows Installation

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
