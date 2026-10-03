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
| Loading | amber pulsing ring, seconds in the middle | llama-swap is starting the model |
| Idle | solid green ring, TTL bar | Loaded, no requests in flight |
| Busy | spinning blue arc, request count | At least one request in flight |
| Stalled | red ring with `!` | Loading/unloading took too long, or a request stopped producing output |
| Unloading | fading grey ring | llama-swap is stopping the model |

Busy and request-level Stalled detection need llama-swap's live event stream (`/api/events`).
The header badge shows `live` when it's connected and `polling` when it isn't.

## Window

- On first launch the window opens centred at 481 x 770; on a screen whose work area is too
  short, the height shrinks to fit. It is always resizable, and your size, position and
  maximized state are remembered between launches. If the saved position is on a monitor that
  is no longer connected, the window opens centred instead. After startup the app never resizes
  or moves the window by itself.
- Cards are one per row at a fixed height, and the header, toolbar, tiles and History columns
  keep their size as data changes, so nothing jumps around. Status banners and error toasts
  overlay the bottom of the window rather than pushing content down.
- The pin button in the header (left of the gear) keeps the window above other windows. The
  choice is saved in the settings file and re-applied at startup. Always-on-top is a window
  manager feature: on Linux it works on X11, but some Wayland compositors ignore it, in which
  case the button simply has no effect.

## Install

Download the installer for your OS from the [Releases](../../releases) page.

These builds are **not code-signed**, so each OS warns on first launch:

- **Windows:** SmartScreen says "Windows protected your PC". Click **More info → Run anyway**.
- **macOS:** right-click the app → **Open** → **Open**. If macOS says the app is damaged, run
  `xattr -dr com.apple.quarantine "/Applications/llama-swap Monitor.app"`.
- **Linux:** use the `.deb`, or `chmod +x` the `.AppImage` and run it. The API key is stored
  through the Secret Service (GNOME Keyring or KWallet), so one of those must be running.

## Connect

1. Enter llama-swap's URL, e.g. `http://192.168.1.20:8080`. Paste the OpenAI-style `.../v1`
   URL if that's what you have; the `/v1` is removed.
2. If your llama-swap config has `apiKeys:`, paste one of those keys. It is stored in your OS
   keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service), never in a
   file, and the UI never sees it again.
3. **Save** tests the connection first and only saves if it works.

   The saved key is only ever sent to the URL it was saved for. If you change the URL, paste
   the key again.

Stall thresholds (loading timeout, no-first-token timeout, and so on) are under
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
installers into a draft release.
