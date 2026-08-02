# Shipyard

Shipyard is a lightweight HTTP server that acts as a [Tauri v2 updater](https://tauri.app/plugin/updater/) backend, using GitHub Releases as the source of truth. It translates GitHub release metadata into the JSON manifest format that Tauri's built-in updater plugin expects, and proxies binary downloads via short-lived GitHub-signed URLs.

## How it works

1. Your Tauri app polls Shipyard's `/v1/update/:app/:channel/:platform` endpoint.
2. Shipyard fetches the GitHub Releases list (cached), finds the highest-version release matching the channel, and returns a Tauri-compatible manifest with a download URL pointing back to Shipyard.
3. When the Tauri updater follows that URL (`/v1/download/:app/:version/:platform`), Shipyard redirects to a GitHub-signed asset URL. No binary data passes through Shipyard.

## Configuration

Copy `config.example.toml` to `config.toml` and adjust it:

```toml
[server]
bind = "0.0.0.0:8080"

[github]
cache_ttl_seconds = 60

[[apps]]
slug = "myapp"
github_repo = "owner/myapp"
display_name = "My App"
```

The GitHub Personal Access Token is read at startup from the `GITHUB_TOKEN`
environment variable. It is never stored in the config file. The token needs
`repo` scope for private repositories; public repos only require a token to
avoid the unauthenticated rate limit (60 req/hour).

The public base URL that appears inside update manifests defaults to
`http://<bind address>`. Override it with:

```
BASE_URL=https://shipyard.example.com
```

## Running locally

```bash
cp config.example.toml config.toml
# edit config.toml — set your github_repo slugs

GITHUB_TOKEN=ghp_... \
BASE_URL=http://localhost:8080 \
RUST_LOG=shipyard=debug \
cargo run
```

Pass an explicit config path as the first argument if needed:

```bash
GITHUB_TOKEN=ghp_... cargo run -- /etc/shipyard/config.toml
```

## Asset naming conventions

Shipyard maps Tauri platforms to asset filename suffixes:

| Platform         | Binary suffix                   | Signature suffix                    |
|------------------|---------------------------------|-------------------------------------|
| `windows-x86_64` | `_x64-setup.nsis.zip`           | `_x64-setup.nsis.zip.sig`           |
| `darwin-x86_64`  | `_x64.app.tar.gz`               | `_x64.app.tar.gz.sig`               |
| `darwin-aarch64` | `_aarch64.app.tar.gz`           | `_aarch64.app.tar.gz.sig`           |
| `linux-x86_64`   | `_amd64.AppImage.tar.gz`        | `_amd64.AppImage.tar.gz.sig`        |

These match the default output of `tauri build --bundles` / `tauri-action`.

## Tauri client configuration

In `src-tauri/tauri.conf.json`, configure the updater plugin to point at Shipyard:

```json
{
  "plugins": {
    "updater": {
      "endpoints": [
        "https://shipyard.example.com/v1/update/myapp/stable/{{target}}"
      ],
      "dialog": true,
      "pubkey": "YOUR_TAURI_UPDATER_PUBLIC_KEY"
    }
  }
}
```

Tauri replaces `{{target}}` with the runtime platform string
(`windows-x86_64`, `darwin-aarch64`, etc.) automatically.

For beta builds, switch the channel segment to `beta`:

```
https://shipyard.example.com/v1/update/myapp/beta/{{target}}
```

## API reference

### `GET /health`

Liveness check. Returns `200 OK` with `{"status":"ok"}`.

### `GET /v1/update/:app/:channel/:platform`

| Parameter         | Values                                                      |
|-------------------|-------------------------------------------------------------|
| `:app`            | slug from config                                            |
| `:channel`        | `stable` or `beta`                                          |
| `:platform`       | `windows-x86_64`, `darwin-x86_64`, `darwin-aarch64`, `linux-x86_64` |
| `?current_version`| (optional) SemVer string — returns `204` when up-to-date  |

**Response `200 OK`:**
```json
{
  "version": "1.2.3",
  "notes": "Release notes from GitHub",
  "pub_date": "2026-05-10T12:00:00Z",
  "platforms": {
    "linux-x86_64": {
      "signature": "<contents of the .sig file>",
      "url": "https://shipyard.example.com/v1/download/myapp/1.2.3/linux-x86_64"
    }
  }
}
```

**Response `204 No Content`:** client is already up-to-date (or no matching release exists).

**Error responses:** `400`, `404`, `502` — all with `{"error":"..."}` body.

### `GET /v1/download/:app/:version/:platform`

Returns `302 Found` with a `Location` header pointing to a GitHub-signed
download URL. The binary is not proxied through Shipyard.

## Deployment

### systemd service

```ini
[Unit]
Description=Shipyard Tauri update server
After=network.target

[Service]
Type=simple
User=shipyard
WorkingDirectory=/opt/shipyard
ExecStart=/opt/shipyard/shipyard /etc/shipyard/config.toml
Environment=GITHUB_TOKEN=ghp_...
Environment=BASE_URL=https://shipyard.example.com
Environment=RUST_LOG=shipyard=info
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Store `GITHUB_TOKEN` in a protected environment file rather than directly in
the unit file for production use:

```ini
EnvironmentFile=/etc/shipyard/secrets.env
```

### Caddy reverse proxy

```caddyfile
shipyard.example.com {
    reverse_proxy localhost:8080
}
```

### Build for production

```bash
cargo build --release
strip target/release/shipyard
# binary is ~5 MB, no runtime dependencies
```

## Logging

Shipyard uses `tracing` with `tracing-subscriber`. Control verbosity via `RUST_LOG`:

```bash
RUST_LOG=shipyard=debug   # verbose — shows cache hits/misses and GitHub calls
RUST_LOG=shipyard=info    # default — one line per HTTP request
RUST_LOG=shipyard=warn    # quiet — only errors and warnings
```

Every HTTP request is logged with method, path, status code, and duration in
milliseconds.
