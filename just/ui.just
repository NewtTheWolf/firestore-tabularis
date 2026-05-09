set shell := ["bash", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]
set working-directory := '..'

# Build the UI extension if `ui/package.json` exists; no-op otherwise.
[unix]
build:
    @if [ -f ui/package.json ]; then \
        echo "Building UI extension..."; \
        (cd ui && npm install --no-audit --no-fund && npm run build); \
    fi

[windows]
build:
    if (Test-Path ui/package.json) {
        Write-Host "Building UI extension..."
        Push-Location ui
        try {
            npm install --no-audit --no-fund
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
            npm run build
            if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        } finally {
            Pop-Location
        }
    }
