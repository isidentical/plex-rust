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

Open the app. The first time, it asks you to sign in and opens plex.tv in
your browser for it.

```sh
open target/plex-rust.app
```

## Keys while browsing

| Key | Action |
| --- | --- |
| Arrows | Move from item to item |
| Return, Space | Open the item the keyboard is on |
| Up, from Home's first shelf | Onto the large item at the top |
| Left, Right, on that item | The item before and after it |
| Return, on that item | Play or resume it |
| Return on an item's page | Play or resume it |
| Command-[ and Command-] | Back and forward |
| Command-1, Command-2, ... | Home and the libraries |
| Command-F | Search |
| Down, in the search field | Into the results |
| Command-R | Load the page again |
| Shift-Command-U | Mark as watched or unwatched |

A secondary click on an item offers to play it or mark it, and on an
episode to go to its show or its season.

A shelf's heading with a chevron after it leads to the whole list. On an
episode's page the show's name leads to the show and the season's name to
that season's episodes. A member of the cast leads to everything else of
theirs in the library. Each library keeps the order and the filter it was
last given.

## Downloads

The page of a film or an episode has a Download button. It fetches the
original file into `~/Movies/plex-rust`, with a poster and what the server
says about the item beside it. Downloads, in the sidebar, lists what is there
and what is on its way. A download that was cut short, by quitting or by the
network, carries on from where it stopped when it is resumed.

A downloaded item plays from disk. When the server cannot be reached the app
opens on Downloads, and how far you got in what you watch is kept on the Mac
and told to the server the next time it answers.

## Keys in the player

| Key | Action |
| --- | --- |
| Space, K, click on the picture | Pause and resume |
| Left, Right | Back and forward 3 seconds. Held, they scan |
| Shift with Left, Right | Back and forward 10 seconds, as the buttons do |
| Up, Down | Volume |
| M | Mute |
| S, D | Slower and faster by 0.1x |
| R | Normal speed |
| A | Next audio track |
| C | Next subtitle track, then off |
| Return | Skip the intro or credits, or play the next episode once it is offered |
| N | Next episode |
| P | Start of the episode. Pressed again there, the previous episode |
| F, double click | Full screen |
| Escape | Leave full screen, then the player |
| Play/Pause media key | Pause and resume |
| Next and Previous media keys | Next episode, and the start of this one or the one before. In a film they do nothing |

The same goes for the buttons on headphones, and for Control Center,
which also shows what is playing with its poster, jumps 10 seconds
back and forward, and has a scrubber of its own.

Low Power in the plex-rust menu draws the picture more cheaply, with plain
scaling and no dithering. It is off until you turn it on, changes what is
playing at once, and stays as you left it. The picture never depends on
whether the Mac itself is in Low Power Mode.

## Layout

| Crate | What it is |
| --- | --- |
| `crates/plex` | Client for plex.tv and Plex Media Server. No UI. |
| `crates/player` | Safe wrapper over libmpv: playback and OpenGL rendering. |
| `crates/app` | The app: windows, views and the command line. |

`SPEC.md` records the design decisions and what each was based on.

## License

GPL-2.0-or-later, as required by the statically linked mpv.
