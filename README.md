# plex-rust

A native macOS client for Plex, written in Rust. It plays the original file
from your server with hardware decoding, in HDR on displays that can show it,
with a large read-ahead buffer and precise speed control.

The interface is AppKit through the [objc2](https://github.com/madsmtm/objc2)
crates. Playback is [mpv](https://mpv.io), linked statically, so the app
depends on nothing but macOS.

plex-rust is a personal, third-party client. It is not affiliated with Plex.

## Build

Requires an Apple Silicon Mac, a Rust toolchain, and Homebrew for the build
tools.

```sh
brew install meson ninja nasm cmake
scripts/build-libmpv.sh    # once: builds mpv and what it needs into vendor/
scripts/bundle.sh          # builds target/plex-rust.app
```

## Use

Sign in once from a terminal, then open the app.

```sh
target/plex-rust.app/Contents/MacOS/plex-rust login
open target/plex-rust.app
```

## Keys in the player

| Key | Action |
| --- | --- |
| Space, K, click | Pause and resume |
| Left, Right | Back and forward 3 seconds |
| Shift with Left, Right | Back and forward 10 seconds |
| Up, Down | Volume |
| M | Mute |
| S, D | Slower and faster by 0.1x |
| R | Normal speed |
| A | Next audio track |
| C | Next subtitle track, then off |
| Return | Skip the intro or credits |
| N | Next episode |
| F, double click | Full screen |
| Escape | Leave full screen, then the player |

## Layout

| Crate | What it is |
| --- | --- |
| `crates/plex` | Client for plex.tv and Plex Media Server. No UI. |
| `crates/player` | Safe wrapper over libmpv: playback and OpenGL rendering. |
| `crates/app` | The app: windows, views and the command line. |

`SPEC.md` records the design decisions and what each was based on.

## License

GPL-2.0-or-later, as required by the statically linked mpv.
