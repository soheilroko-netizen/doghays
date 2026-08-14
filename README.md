# I've removed the Tauri GUI and restored the CLI-only version.

## What Changed
- Stripped all Tauri UI code
- Removed index.html, settings.html, src-tauri/
- Clean CLI cargo project with embedded sing-box support
- Updated workflow to download and bundle sing-box as a static asset

## Usage
```bash
# Build (with bundled sing-box)
cargo build --release

# Run for the first time (downloads sing-box if not bundled)
./target/release/stls.exe

# With profile.toml (next to the .exe)
./target/release/stls.exe --profile profile.toml

# Write profile
target/release/stls.exe --write-profile profile.toml
```

## New Build Workflow
1. Downloads sing-box from GitHub releases (Windows-only)
2. Bundles it into `bin/sing-box.exe`
3. Builds `stls.exe` with the bundled binary
4. Releases both executables together

Now it's a self-contained CLI client that ships with its dependencies.