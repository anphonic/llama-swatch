# llama-swap Monitor

A small desktop app that shows what your [llama-swap](https://github.com/mostlygeek/llama-swap)
instance is doing: whether it's reachable, which models are loaded, which are busy or stuck,
and a summary of throughput.

Works with llama-swap running anywhere you can reach over HTTP: the same machine, a box
on your LAN, or behind a reverse proxy.

## What the states mean

| State | Looks like | Meaning |
|---|---|---|
| Not loaded | dimmed grey ring | Configured in llama-swap, not running |
| Loading | amber pulsing ring, seconds in the middle | llama-swap is starting the model. Past the slow-load warning (default 120 s) the card adds a `slow load` hint; llama-swap's own health-check timeout decides whether the load fails |
| Idle | solid green ring, TTL bar | Loaded, no requests in flight |
| Busy | spinning blue arc, request count | At least one request is in flight and remains within its applicable timeout |
| Stalled | red ring with `!` | Unloading took too long, or the model stopped producing output |
| Unloading | fading grey ring | llama-swap is stopping the model |

A busy card shows how many requests are streaming output and how many are still queued for a
free slot, plus how long ago output last arrived, e.g. `3 streaming · 2 queued · last output 4 s ago`.
Stalls are judged for the whole model, not per request: while any request is streaming, the
model is Stalled only when every streaming request has been quiet longer than the stream stall
timeout, so a request waiting its turn never marks an active model as Stalled. With nothing
streaming yet, it is Stalled once the longest wait passes the no-first-byte timeout. A request
queued for more than 10 times that timeout while others stream adds a `queued long` hint.

Busy and output-stall detection need llama-swap's live event stream (`/api/events`).
The header badge shows `live` when it's connected and `polling` when it isn't.

## Window

- On first launch the window opens centred at 481 x 770; on a screen whose work area is too
  short, the height shrinks to fit. It is always resizable, and your size, position and
  maximized state are remembered between launches. If the saved position is on a monitor that
  is no longer connected, the saved size is kept and the window opens at the default position
  on the main screen instead. After startup the app never resizes or moves the window by itself.
- Cards are one per row at a fixed height, and the header, toolbar, tiles and History columns
  keep their size as data changes, so nothing jumps around. Status banners and error toasts
  overlay the bottom of the window rather than pushing content down.
- The pin button in the header (left of the gear) keeps the window above other windows. The
  choice is saved in the settings file and re-applied at startup. Always-on-top is a window
  manager feature: on Linux it works on X11, but some Wayland compositors ignore it, in which
  case the button simply has no effect.

## Install

Releases are published on the [Releases](../../releases) page, and only when every platform
build succeeds. Pick the file for your OS:

| OS | File |
|---|---|
| Windows | `*-setup.exe` (installer) or `*.msi` |
| macOS | `*.dmg` (universal: Apple silicon and Intel). Ignore `*.app.tar.gz` and any `.sig` files |
| Linux | `*.deb`, `*.rpm`, or `*.AppImage` |

Each release also has a `SHA256SUMS.txt`. To verify a download, put it in the same folder
as the file and compare:

```powershell
# Windows (PowerShell): compare the output with the matching line in SHA256SUMS.txt
(Get-FileHash ".\<file>" -Algorithm SHA256).Hash.ToLower()
```

```bash
# Linux
sha256sum -c SHA256SUMS.txt --ignore-missing
# macOS
grep "<file>" SHA256SUMS.txt | shasum -a 256 -c
```

These builds are **not code-signed or notarized**, so each OS warns on first launch:

- **Windows:** SmartScreen says "Windows protected your PC". Click **More info → Run anyway**.
- **macOS:** right-click the app → **Open** → **Open**. If macOS says the app is damaged, run
  `xattr -dr com.apple.quarantine "/Applications/llama-swap Monitor.app"`.
- **Linux:** use the `.deb` or `.rpm`, or `chmod +x` the `.AppImage` and run it. The API key is
  stored through the Secret Service (GNOME Keyring or KWallet), so one of those must be running.

Prefer not to trust a binary? Build it yourself, see [Develop](#develop).

## Connect

The API key is optional: you only need one if your llama-swap config sets `apiKeys:`.

1. Enter llama-swap's URL, e.g. `http://llama-swap.example:8080`. Paste the OpenAI-style `.../v1`
   URL if that's what you have; the `/v1` is removed.
2. If your llama-swap config has `apiKeys:`, paste one of those keys. It is stored in your OS
   keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service), never in a
   file, and the UI never sees it again.
3. **Save** tests the connection first and only saves if it works.

   The saved key is only ever sent to the URL it was saved for. If you change the URL, paste
   the key again.

Thresholds (slow-load warning, no-first-byte timeout, and so on) are under
**Stall detection** in settings.

## Develop

Requires Node 22+, Rust stable, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/)
for your OS.

```bash
npm install
npm run tauri dev
```

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Releases: push a `v*` tag. GitHub Actions builds Windows, macOS (universal) and Linux
installers, publishes them as a release once all builds succeed (a tag containing a hyphen, like `v1.0.0-rc1`, is
marked pre-release), and attaches `SHA256SUMS.txt`.

To build installers locally, run `npm run tauri build`.

## License

Licensed under the Apache License, Version 2.0. See LICENSE and NOTICE.

Third-party dependencies are under their own licenses (MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0 and similar).
