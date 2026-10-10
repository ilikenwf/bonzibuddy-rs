# BonziBuddy-rs

A safe, offline, cross-platform (Windows / macOS / Linux) reimplementation of the
1999 [BonziBuddy](https://en.wikipedia.org/wiki/BonziBuddy) desktop companion,
written in Rust, using the **original character art, animations, sounds and
joke/fact content** extracted from the archived installer.

The original was famous as spyware. This reimplementation contains
**zero network code** — no phone-home, no telemetry, no ads, no gating.

![Bonzi walking](docs/walk.png)

## What the original did (and what was removed)

Findings from reverse-engineering `BonziBuddy432.exe` (v4.32, from the
[Internet Archive](https://archive.org/details/BonziBUDDY_201812)); raw
evidence in `analysis/` (`bonzi_exe_urls.txt`, `installer.strings.txt`):

| Original behavior | Evidence | This project |
|---|---|---|
| Phone-home / IM beacon to `im.bonziworld.com` | strings + `MSWinsockLib.Winsock` ActiveX imports | **removed** — no sockets anywhere |
| Logged your browsing history (`Browse.nbd`) and search queries (`Search.nbd`) | on-disk spy logs, odd-byte+1 "encryption" | **never loaded** (see `src/content.rs`) |
| Ad links (`sites.nbd`) and daily content feed (`Daily.nbd`) fetched from bonzi.com | nbd payloads, updater strings | **never loaded** |
| Email modules for referral spam (`emsmtp.dll`, `empop3.dll`) | installer file list | **not reimplemented** |
| "Bonzi's Download Manager" — fetch & run remote payloads, IE hook | `Apps.nbd`, installer strings | **not reimplemented** |
| Paid upsell gating ("Bonzi Unlimited") | EULA/registration strings | **everything is free, forever** |
| Registry persistence / autostart | installer strings | **none** — single binary, no install |

`cargo tree` (247 crates) contains no HTTP/TLS/socket dependency; `src/`
contains no `TcpStream`/`UdpSocket`/`connect` call. The only network-adjacent
string in the tree is the crate `scoped-tls` (thread-local storage, not TLS).

## Where the assets come from

1. `analysis/BonziBuddy432.exe` — archived installer (Internet Archive).
2. CAB payloads (`payload.cab`, `data_fixed.cab`) unpacked with 7z.
3. `analysis/acs_extract.py` — parser for the MS Agent `.acs` character format:
   decodes the pose/animation database into 146 PNG sprite animations
   (`assets/bonzi/sprites/`), frame timings and return-animations
   (`animations.json`), plus the original WAV sounds.
4. `.nbd` content files ("encryption" = add 1 to every odd byte) decoded into
   `assets/content/nbd_decoded.json`: ~600 original jokes, facts and songs,
   with the original `<sp=...>` / `\pau=2000\` TTS markup stripped at load.

## Features

- Original Bonzi sprite animations (idle, blink, walk, dance, hide, think…),
  driven by the original per-frame timings and return-animation graph.
- Speech bubbles with the original joke/fact/song corpus; optional local TTS
  (`--tts`: `say` on macOS, PowerShell System.Speech on Windows, espeak-ng on Linux).
- Click to pet, right-click menu (joke / fact / song / trick / dance / walk / hide / about / quit).
- Draggable always-on-top borderless window; walks along the bottom of the work area and turns at screen edges.
- Original WAV sound effects (`CHORD.WAV`, `Snd1/2.wav`, `Intro2.wav`).
- Scroll-wheel over the buddy resizes it (0.5×–4×), persisted across restarts
  (`$XDG_CONFIG_HOME/bonzibuddy-rs.json`); `--scale N` sets it at launch and HiDPI
  scale factors are honored automatically.
- Zero network code, zero registry writes, zero install steps.

## Building & running

```sh
cargo run --release
```

Flags:

| Flag | Effect |
|---|---|
| `--scale N` | sprite/UI scale multiplier (0.5–4.0) |
| `--matte` | solid Win95-teal backdrop instead of transparency (for WMs without a compositor) |
| `--no-audio` | start silent |
| `--tts` | speak bubbles with the OS voice |
| `--assets PATH` | asset directory (default search: `$BONZI_ASSETS`, `./assets`, `../assets`, next to the binary, `/usr/share/bonzibuddy-rs/assets`) |

### Cross-compiling / CI

```sh
cargo check --target x86_64-pc-windows-msvc          # passes from Linux
cargo check --target aarch64-apple-darwin --no-default-features
```

`coreaudio-sys` (via rodio→cpal) needs the macOS SDK at build time, so full
audio builds for macOS must happen on macOS. The `audio` feature (default on)
can be disabled (`--no-default-features`) for headless or SDK-less builds.
Linux transparency needs a compositor; without one use `--matte`.

## Tests

```sh
cargo test        # content-pool + TTS-markup-stripping regression tests
```

## Layout

```
src/main.rs      event loop, window, input, menu, walk physics
src/pet.rs       state machine: Action queue → animation/bubble/effects
src/assets.rs    .acs-derived sprite/anim/audio loading + caches
src/content.rs   .nbd joke corpus + TTS markup stripper
src/render.rs    CPU canvas: alpha blend, rounded rects, text wrap (ab_glyph)
src/tts.rs       OS voice subprocess (only process spawned; local only)
analysis/        RE tooling + raw evidence (kept for auditability)
assets/          extracted original assets (see below)
```

## License & asset provenance

Code is MIT. The BonziBuddy character, art, sounds and text are © BONZI.COM
Software / CNET Industries and are used here strictly as an abandonware
preservation artifact, extracted from a publicly archived installer; they are
not licensed for redistribution by this project. Download and run at your own
discretion.

## Roadmap

- Menu +/- size items alongside scroll-wheel resize.
- Optional local-LLM "brain": an OpenAPI-compatible client that pushes the
  same `pet::Action` values the offline engine already speaks (`src/pet.rs`).
