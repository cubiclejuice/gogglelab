# Free server, MCP and capture installation

The server and browser capture extension are optional Free components. The desktop app stays local and starts no listener automatically. Enabling its local server shares the running app's library state; a standalone process takes exclusive ownership of its configured library. Do not run both processes against the same catalog.

## Standalone local server

From the public Community checkout, install the repository's supported Rust and Bun tools, then run:

```sh
bun install --frozen-lockfile
bun run build:browser
cargo build --release --locked -p library-server --bin gogglelab-server
```

Create an operator-owned configuration directory outside the catalog. Use absolute paths in a JSON config:

```json
{
  "library": "/srv/gogglelab/library",
  "web": "/srv/gogglelab/dist-browser",
  "tokenFile": "/srv/gogglelab/config/server-token",
  "integrationSettings": "/srv/gogglelab/config/integrations.json",
  "allowExternalIntegrations": false,
  "networkExposure": "loopback",
  "allowedHosts": ["127.0.0.1:3745", "localhost:3745"],
  "allowedOrigins": ["http://127.0.0.1:3745", "http://localhost:3745"],
  "allowedRoots": [
    {"id": "models", "label": "Models", "path": "/srv/models", "access": "readOnly"}
  ],
  "capabilities": ["catalogRead", "catalogWrite", "mediaRead", "mediaWrite", "scan", "integration", "maintenance"]
}
```

Replace every example path with a real, deliberately selected directory. The configured model root is read-only by default; importing and catalog edits use the owned library. No browser or MCP request can expand the root allowlist.

`allowExternalIntegrations` defaults to `false` for old and new configs. When explicitly enabled by the service operator, authenticated Browser/MCP clients may call the configured AI provider or Spoolman through the guarded HTTP transport. Requests happen only when a client explicitly calls Generate suggestions or Spoolman sync. Turning access off blocks new requests, while a request already in progress may finish. AI `categorySharing` (`local` by default) is a separate choice controlling which tag-category fields accompany an AI request. The desktop app has the same opt-in in its Browser and MCP server settings; it resets to off when that server stops. No network integration call is needed to build or test the server.

### Optional backup-only destination

To enable consistent backups and selected exports over the browser/MCP API, create a dedicated directory outside the library and model roots, then add it as a `backupOnly` root and grant `backupWrite` alongside `maintenance`:

```json
{
  "allowedRoots": [
    {"id":"models","label":"Models","path":"/srv/models","access":"readOnly"},
    {"id":"backups","label":"Backups","path":"/srv/gogglelab/backups","access":"backupOnly"}
  ],
  "capabilities": ["catalogRead","mediaRead","maintenance","backupWrite"]
}
```

The backup directory must already exist and be writable by the service account. Mount it as a separate writable volume for Docker/NAS. Before acquiring the catalog lock or opening the database, runtime admission resolves canonical paths and rejects a `backupOnly` root that contains or is contained by the library root or any other admitted root, so recursive scans cannot enter the backup destination. Parent segments such as `..` are resolved, and a not-yet-created library path is checked before files are created. The `backupOnly` handle supports new backup/export files and can be read as a restore source; it cannot be used for trash, moves, purge, indexing or model-source writes. Restoring mapped model roots still requires the separate writable-root capability. Backup/export operations use one-use plans and refuse to overwrite existing filenames.

### Optional source Trash and moves

Source-file MCP actions are off by default. To enable them on a Unix host, configure only the selected model root as `writable`, mount that same directory read-write, and explicitly add `physicalSourceWrite` to the server capabilities. Keep `backupOnly` roots separate; they never grant source-write access. The default Docker config remains read-only.

The authenticated MCP tools operate on one loose indexed model at a time. `trash_file` and `move_files` require `modelId`, `expectedVersion`, and `confirm: true`; `move_files` also requires an admitted writable destination handle and a safe leaf filename. `undo_trash` uses the returned snapshot ID. Each planning call returns an expiring, principal-bound, one-use action token; execute it separately with `execute_management_action`. The tools accept no client source paths, reject ZIP members and stale hashes/versions, and do not overwrite a destination. Trash first writes and syncs a verified recovery copy under the library's `trash` directory. The original source is removed only after the recovery copy and source hash checks succeed.

These MCP source-write tools are not advertised on Windows yet because durable directory synchronization is not implemented there. Unix filesystem path operations also retain a narrow race against another local process that can concurrently replace directories inside the same writable root; use roots whose write access is controlled by the server operator.

Start with an absolute config path:

```sh
./target/release/gogglelab-server --config /srv/gogglelab/config/server.json
```

The server creates a random access key in the configured secret file if absent. It requires an ordinary file, outside the catalog, with Unix permissions `0600`; keep it separate from exports and backups. Open `http://127.0.0.1:3745/` and enter the key through the login form. Browser credentials remain in memory and are cleared on disconnect. Do not put keys in URLs, scripts committed to Git, model notes, or screenshots. Health at `/api/v1/health` reports liveness only; API, media, events and MCP data require authentication.

## HTTPS on a LAN

A LAN browser connection requires HTTPS. Use a reverse proxy and keep the backend loopback-only wherever possible. With Caddy running on the same host, set:

```json
{
  "networkExposure": "tlsProxy",
  "publicBaseUrl": "https://gogglelab.home.arpa",
  "trustedProxyPeers": ["127.0.0.1"],
  "allowedHosts": ["gogglelab.home.arpa"],
  "allowedOrigins": ["https://gogglelab.home.arpa"]
}
```

These fields replace their counterparts in the full config; retain library, web, token, integration, roots and capabilities. Resolve the selected hostname through your local DNS or hosts file. This optional Caddyfile uses a local certificate authority:

```caddyfile
gogglelab.home.arpa {
    tls internal
    reverse_proxy 127.0.0.1:3745 {
        header_up Host gogglelab.home.arpa
    }
}
```

Trust the local CA certificate on each client using its operating system's certificate tools. Distribute the public root certificate, never the CA private key. Do not disable certificate verification. Caddy's [TLS documentation](https://caddyserver.com/docs/caddyfile/directives/tls) describes the internal CA and explicit PEM certificate/key alternative; its [reverse proxy documentation](https://caddyserver.com/docs/caddyfile/directives/reverse_proxy) describes upstream headers. GoggleLab trusts the actual configured socket peer and configured public URL, rather than treating forwarded headers as authority.

For a real public domain, Caddy can obtain certificates through ACME by omitting `tls internal`. This requires the operator's separate DNS/firewall/certificate decisions; it is not enabled by this repository. The server still needs exact Host/Origin allowlists, trusted proxy peers and its access key. Restrict the upstream port to the proxy.

## Docker and NAS

`Dockerfile.server` builds only public code, without a GUI or private Pro dependency. The final image runs as UID/GID `10001:10001`. `compose.server.yml` publishes port 3745 only on host loopback, uses a named data volume, mounts the selected model directory and access-key file read-only, and sets resource limits and a read-only root filesystem.

Set `GOGGLELAB_MODELS_PATH` to the explicit model folder and `GOGGLELAB_TOKEN_FILE` to the separate secret file, then use:

```sh
docker compose -f compose.server.yml build
docker compose -f compose.server.yml up -d
docker compose -f compose.server.yml logs --tail 50
```

Create a random 32-byte key in the deliberately selected secret file without overwriting an existing key:

```sh
(umask 077; set -C; openssl rand -base64 32 > "$GOGGLELAB_TOKEN_FILE")
```

The container must be able to read that secret as UID 10001 while its mode remains `0600`. On Linux this means assigning that individual file to UID/GID 10001. Do not broaden it to world-readable permissions. If a NAS cannot represent that owner for bind mounts, use a separate operator-managed secret volume mounted read-only at `/run/secrets`; keep it separate from the writable catalog volume. Do not change ownership recursively across your model folders. The source mount may be owned by another user as long as UID 10001 has read/traverse permission.

Keep one server replica per catalog. Select storage that supports the runtime's exclusive OS lock. Do not delete ownership lock files to force concurrent processes. Stop the service before an external volume snapshot; use the application's consistent backup operation for an online backup. Preserve the named catalog volume when recreating the container. `docker compose down` keeps it; `down --volumes` deletes it and is not part of a normal upgrade.

For NAS LAN access, terminate HTTPS at a proxy as above, explicitly update host/origin/proxy configuration for that network, and expose only the proxy to clients. A same-host proxy can reach the loopback-published port. A container proxy needs a deliberately configured private network and an exact trusted peer policy. Do not copy the default container mode into a public port mapping.

## MCP

The same authenticated listener exposes `/mcp` using Streamable HTTP protocol `2025-11-25`. Configure an MCP client with the HTTPS or loopback endpoint and an `Authorization: Bearer …` header through that client's secret settings. Never append the key to the endpoint URL.

The client must initialize, acknowledge initialization, and send the returned session ID plus the negotiated protocol version on subsequent requests. A session ID does not replace authentication. Requests advertise JSON and event-stream acceptance. POST supports JSON-RPC; accepted notifications return 202; DELETE ends a session. Standalone GET SSE and JSON-RPC batch arrays are not supported. `tools/list` advertises only connected, authorized operations. Review destructive actions before consuming their one-use confirmation; model IDs, versions and source hashes prevent applying stale work.

The embedded desktop server also offers `list_slicers` and `open_in_slicer` to authorized principals with host-slicer and media-read capability. These tools return only stable slicer IDs/display names and accept a catalog model ID/version plus slicer ID; they do not accept client filesystem paths. Opening stages a verified copy in app-owned handoff storage and launches a freshly detected supported slicer. The standalone/headless server, including Docker/NAS, has no host slicer adapter and does not advertise these operations; clients can download a model or bundle and open it in their own slicer.

## Capture extension

Load the `browser-extension/` source directory as an unpacked Manifest V3 extension in an explicitly selected Chrome or Edge profile. It captures supported model pages and completed download metadata into a persistent queue while GoggleLab is closed. It does not automatically fetch model files or authenticate to catalog APIs.

Choose an inbox folder explicitly, or use the Downloads fallback. Import the resulting versioned JSON through GoggleLab. URL-only captures become bookmarks; matching downloaded bytes require an admitted source or explicit upload. Failed items remain retryable. Do not assume that a downloaded filename authorizes reading arbitrary host paths.

## Copy-paste AI install prompt

```text
Install GoggleLab's optional Free headless server, MCP endpoint and capture extension from the public Community repository in this local environment. Inspect README, docs/SERVER-INSTALL.md, docs/PHASE-6-SERVER-MCP-CAPTURE.md, the actual CLI configuration types, Dockerfile.server, compose.server.yml and browser-extension/README.md before making changes. Verify the checkout and preserve dirty files.

Default to localhost and read-only model mounts. Ask me to select the catalog directory and source model directories; do not discover or mount my live data implicitly. Build with the locked dependencies and browser assets. Use an isolated synthetic STL/3MF fixture first. Generate the server access key locally in a separate 0600 secret file owned by the service; never print it, add it to a URL, commit it, copy it into the catalog, or send it to a remote service. Do not alter permissions recursively.

Choose native standalone or the supplied local Docker Compose according to my environment. Run one owner process per catalog. Verify unauthenticated data requests fail, authenticated browser and MCP work, original source bytes stay unchanged, edits and previews persist after restart, the health probe succeeds, and source mounts remain read-only. Keep task-owned test artifacts separate from my data and delete only those I authorize.

Keep the desktop listener off unless I explicitly enable it. Configure LAN HTTPS, public DNS, certificates, firewall openings or reverse-proxy networking only if I explicitly request them, using exact host/origin and actual proxy peer allowlists. No cloud publication, provider API call or spending is authorized by this prompt. AI category sharing defaults to local; provider sharing must remain a user-controlled opt-in.

For the MV3 extension, explain its page/download queue and inbox behavior. Install only into the browser profile I select; do not modify my existing profile implicitly. Provide local usage, upgrade/backup, rollback and uninstall instructions with the paths and commands actually verified here. Treat missing operations or unavailable OS checks as limitations rather than claiming full parity.
```
