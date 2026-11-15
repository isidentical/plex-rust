<p align="center">
  <img src="assets/icon.png" width="160" alt="The plex-rust icon: a pixel art crab holding a play sign">
</p>

<h1 align="center">plex-rust</h1>

<p align="center">
  A native macOS client for Plex Media Server, written in Rust.
</p>

plex-rust plays the original file from your server, with hardware decoding,
in HDR on displays that can show it, with a large read-ahead buffer and
precise speed control. The interface is AppKit, through the
[objc2](https://github.com/madsmtm/objc2) crates. Playback is
[mpv](https://mpv.io), linked statically, so the app depends on nothing but
macOS.

> plex-rust is an unofficial, third-party client. It is not affiliated with,
> endorsed by or supported by Plex, Inc. Plex is a trademark of Plex, Inc.

## What it does

- Browses your libraries: Home with what you are watching, movie and TV
  libraries, search, and pages for films, shows, seasons and episodes.
- Plays the original file with no server transcoding, decoded by
  VideoToolbox, with HDR10, HLG and Dolby Vision profile 8 shown in HDR.
- Resumes where you stopped, reports progress to the server, skips intros
  and credits, and goes on to the next episode, across seasons.
- Shuffle: a show, a season, or a whole movie or TV library in a random
  order, one after another, each item once.
- Downloads films and episodes, with their subtitle files, and plays them
  without the server.
- Activity: how much you watched, when, what and on what, over 7 days,
  30 days, 12 months or all time. Watch time by day or month, the hours
  of the week you watch most, your most played shows and films, genres,
  release decades, devices, data streamed and how much of each library
  you have seen. It reads the history and statistics your server keeps,
  the same ones Plex Web's Dashboard uses.
- Keyboard first, with full support for a trackpad and a touch screen:
  pinch to zoom the picture, media keys, Now Playing in Control Center.
- A Low Power switch that trades picture quality for battery when you ask
  for it. It never follows the Mac's own Low Power Mode on its own.

## Requirements

- A Mac with Apple silicon and macOS 26 or later.
- A Plex Media Server you can reach directly, on your network or through a
  VPN. Relayed connections are not used.

## Build

You need a Rust toolchain (1.100 or later) and the build tools from
Homebrew.

```sh
brew install meson ninja nasm cmake
scripts/build-libmpv.sh    # once: downloads and builds mpv, FFmpeg and their libraries into vendor/
scripts/bundle.sh          # builds target/plex-rust.app
```

`scripts/build-libmpv.sh` downloads pinned releases of mpv 0.42.0, FFmpeg
9.0.2, dav1d, libass and its text stack, and libplacebo, applies the
patches in [`scripts/patches`](scripts/patches/README.md), and builds them
as static libraries. Nothing of theirs is kept in this repository.

## Use

```sh
open target/plex-rust.app
```

The first time, it asks you to sign in and opens plex.tv in your browser.
The token Plex gives back is kept in
`~/Library/Application Support/plex-rust/token`, readable only by your
user. It is a plain file, not a Keychain item.

It opens where you left it: the same window, and the library or page that
was on screen. When the server cannot be reached, what was last seen stays,
with a line at the bottom that says so and a button to try again.

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

A secondary click on an item offers to play it or mark it, on a show to
shuffle it, and on an episode to go to its show or its season.

The shuffle button on a show's page plays its episodes in a random order,
the one in its season bar the episodes of that season, and Shuffle in a
library's bar every film or episode in it. Specials are left out but for
the season of specials itself. The order is drawn once, on the Mac, and
the player goes through it the way it goes through a show: Up Next offers
the next item, N and P go forward and back, and it stops after the last.

A shelf's heading with a chevron after it leads to the whole list. The two
arrows at the other end of a heading bring the next cards of its shelf
into view, and are there only while the shelf has more than fit across. On an
episode's page the show's name leads to the show and the season's name to
that season's episodes. A member of the cast leads to everything else of
theirs in the library. Each library keeps the order and the filter it was
last given.

A library listed by title has the first letters of its titles down its
far edge. A press on one goes to where those titles start, and so does a
press that slides along the letters and lets go on one.

## Downloads

The page of a film or an episode has a Download button. It fetches the
original file of the version picked on that page into `~/Movies/plex-rust`,
with a poster, the subtitle files the server keeps next to the video, and
what the server says about the item beside it. Downloads, in the sidebar, lists what is there
and what is on its way. A download that was cut short, by quitting or by the
network, carries on from where it stopped when it is resumed.

A downloaded item plays from disk, with those subtitles. When the server
cannot be reached the app opens on Downloads, the page of a downloaded item
is all there, and how far you got in what you watch is kept on the Mac and
told to the server the next time it answers. Downloads says at its foot how
much room its files take and how much the disk has left.

## Keys in the player

| Key | Action |
| --- | --- |
| Space, K, click on the picture | Pause and resume |
| Left, Right | Back and forward 5 seconds. Held, they scan |
| Shift with Left, Right | Back and forward 10 seconds, as the buttons do |
| Up, Down | Volume. Held, it keeps changing |
| M | Mute |
| S, D | Slower and faster by 0.1x. Held, they keep going |
| R | Normal speed |
| A | Next audio track |
| C | Next subtitle track, then off |
| - and = | Subtitles smaller and larger |
| [ and ] | Subtitles 0.1 seconds earlier and later |
| L | The list of chapters |
| , and . | The start of the chapter, then the chapter before, and the next chapter |
| Up, Down, in an open list | From row to row. Return chooses the one the keyboard is on |
| Return | Skip the intro or credits, play the next episode once it is offered, or try again after a failure |
| N | Next episode, or next item of a shuffle |
| P | Start of the episode. Pressed again there, the previous episode, or the previous item of a shuffle |
| T | Time left, or the full length |
| I | How the file is being played: picture, colour, decoding, sound, bit rate, dropped frames, buffer |
| Tab, Shift-Tab | Move from button to button. Space or Return presses the one the keyboard is on |
| Z | Fill the window with the picture, cropping what does not fit, and back |
| Command-plus, Command-minus | Picture larger and smaller |
| Command-0 | The whole picture again |
| F, double click | Full screen |
| O | The mini player, and back to the full window |
| Escape | Close an open list, turn down the next episode, leave full screen, then the player |

## Fingers in the player

| Gesture | Action |
| --- | --- |
| Pinch, on a trackpad or a touch screen | Picture larger and smaller, around the fingers, up to 8 times |
| Drag on an enlarged picture, or two fingers on a trackpad | Move the picture |
| Two fingers tapping twice on a trackpad | Fill the window, and back |
| One finger tapping twice on a touch screen | Fill the window, and back |
| Tap, click | Pause and resume |
| Play/Pause media key | Pause and resume |
| Next and Previous media keys | Next episode, and the start of this one or the one before. The same in a shuffle. In a film played on its own they do nothing |

The same goes for the buttons on headphones, and for Control Center,
which also shows what is playing with its poster, jumps 10 seconds
back and forward, and has a scrubber of its own.

The chapters, speed, audio and subtitle buttons each open a list above the
bar. The speed's has a minus and a plus that change it by 0.1x over the
speeds it offers, and the subtitles' has the same for their size and for
how much earlier or later than the film they are shown. A click beside a
list closes it.

Low Power in the plex-rust menu draws the picture more cheaply, with plain
scaling and no dithering. It is off until you turn it on, changes what is
playing at once, and stays as you left it.

## Mini player

O, or the button left of full screen, moves the video into a small window
in the corner of the display, and the player's window goes out of sight.
It floats above other apps, on every Space, keeps the shape of the
picture, is moved by dragging the picture and resized from its edges. With
the pointer over it, it shows three buttons: back to the full window,
pause and resume, and close, which stops playback as leaving the player
does. Space, K, Left, Right, M and the Shift jumps work in it as in the
player, and O or Escape go back to the full window, as does a click on the
app's Dock icon.

## Known limits

- Dolby Vision profile 5 plays with wrong colours, and the player says so.
  mpv's OpenGL renderer, the only one its embedding API offers, cannot
  apply the Dolby Vision reshaping.
- The system's Picture in Picture is not available on this rendering path.
  The mini player is the app's own window instead.
- Image subtitles (PGS) move with the picture when it is zoomed.
- The app is ad hoc signed. macOS may ask once for permission to reach
  devices on the local network.

[`SPEC.md`](SPEC.md) records the design, the decisions and what each was
based on.

## Layout

| Crate | What it is |
| --- | --- |
| `crates/plex` | Client for plex.tv and Plex Media Server. No UI. |
| `crates/player` | Safe wrapper over libmpv: playback and OpenGL rendering. |
| `crates/app` | The app: windows, views and the command line. |

## Contributing

Issues and pull requests are welcome. Section 3 of [`SPEC.md`](SPEC.md) is
the bar code is held to. Every commit should build, pass
`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test`, and do one thing.

## License

plex-rust is free software under the GNU General Public License, version 2
or any later version. See [`LICENSE`](LICENSE). The GPL is required because
the app links mpv statically, and mpv is built under the GPL. mpv, FFmpeg
and the other libraries it builds keep their own licenses; the patches
applied to them are listed with their origin in
[`scripts/patches/README.md`](scripts/patches/README.md).
