set shell := ["bash", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]
set working-directory := '..'

# Build + copy binary, manifest and (if present) UI bundle into Tabularis's plugin folder.
[linux]
install:
    @just ui build
    cargo build
    mkdir -p ~/.local/share/tabularis/plugins/firestore
    cp target/debug/firestore-plugin ~/.local/share/tabularis/plugins/firestore/
    cp manifest.json ~/.local/share/tabularis/plugins/firestore/
    @if [ -f ui/dist/index.js ]; then \
        mkdir -p ~/.local/share/tabularis/plugins/firestore/ui/dist; \
        cp ui/dist/index.js ~/.local/share/tabularis/plugins/firestore/ui/dist/; \
    fi
    @echo "Installed to ~/.local/share/tabularis/plugins/firestore"
    @echo "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

[macos]
install:
    @just ui build
    cargo build
    mkdir -p "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore"
    cp target/debug/firestore-plugin "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore/"
    cp manifest.json "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore/"
    @if [ -f ui/dist/index.js ]; then \
        mkdir -p "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore/ui/dist"; \
        cp ui/dist/index.js "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore/ui/dist/"; \
    fi
    @echo "Installed to ~/Library/Application Support/com.debba.tabularis/plugins/firestore"
    @echo "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

[windows]
install:
    @just ui build
    cargo build
    $dest = Join-Path $env:APPDATA "com.debba.tabularis\plugins\firestore"
    New-Item -ItemType Directory -Force -Path $dest | Out-Null
    Copy-Item "target\debug\firestore-plugin.exe" $dest
    Copy-Item "manifest.json" $dest
    if (Test-Path "ui\dist\index.js") {
        New-Item -ItemType Directory -Force -Path (Join-Path $dest "ui\dist") | Out-Null
        Copy-Item "ui\dist\index.js" (Join-Path $dest "ui\dist")
    }
    Write-Host "Installed to $dest"
    Write-Host "Restart Tabularis (or toggle the plugin in Settings) to pick up changes."

# Remove the installed plugin from Tabularis's plugin folder.
[linux]
uninstall:
    rm -rf ~/.local/share/tabularis/plugins/firestore

[macos]
uninstall:
    rm -rf "$HOME/Library/Application Support/com.debba.tabularis/plugins/firestore"

[windows]
uninstall:
    $dest = Join-Path $env:APPDATA "com.debba.tabularis\plugins\firestore"
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
