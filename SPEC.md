# plex-rust design document

plex-rust is a native macOS client for Plex, written in Rust. The interface
is AppKit through the objc2 crates and playback is a statically linked mpv.

plex-rust is an unofficial third-party client. It is not affiliated with,
endorsed by or sponsored by Plex. Plex is a trademark of Plex, Inc. The app
uses no Plex logo or artwork, and its icon is its own.

This document records what the app does, why it is built the way it is, and
what each decision rests on. Section numbers are stable: source comments and
commit messages cite them. Where a claim comes from somewhere else, the
source is named next to it. Where something has not been confirmed, the
sentence says so, and section 12 collects those.

## 1. Context

Plex's own desktop app for macOS is built on Qt, with Qt Quick and
QtWebEngine among the frameworks in its bundle (seen in version 1.115.0).
plex-rust is a client built from the platform's own parts instead. Its aims:
browse without waiting, always play the original file, buffer far ahead,
stay smooth at high playback speed, and use as little energy as that allows.

## 2. Decisions

| Topic | Decision |
| --- | --- |
| Language | Rust only, stable, 1.100 or later, edition 2024 |
| UI | AppKit through objc2 0.6.5 and objc2-app-kit 0.3.2. No Swift, no web view |
| Playback | libmpv 0.42.0 embedded in the app's own NSView through mpv's OpenGL render API |
| Scope | Movies and TV, offline downloads of original files |
| Out of scope | Music, Live TV / DVR, Photos, transcoding, relay connections, Picture in Picture |
| Network | Direct connections only: on the local network, to an address given by hand such as one on a VPN, or to the server's remote HTTPS address. Plex's relay is never used |
| Platform | Apple Silicon, macOS 26 or later, no sandbox, ad hoc signed |
| Packaging | One self-contained plex-rust.app holding one executable that links only macOS system libraries. Nothing to install to run it |
| Quality vs power | Quality is always first. The app never reads the macOS Low Power Mode state. One in-app Low Power switch, off by default |

### What these decisions cost

- mpv's render API offers three backends: OpenGL, Direct3D 11 and software
  ([`render.h`](https://github.com/mpv-player/mpv/blob/v0.42.0/include/mpv/render.h)).
  Its OpenGL backend draws with `gl_video`, the renderer of `vo=gpu`
  ([`libmpv_gpu.c`](https://github.com/mpv-player/mpv/blob/v0.42.0/video/out/gpu/libmpv_gpu.c)),
  not with `gpu-next`. So there is no Dolby Vision reshaping (section 7),
  and no `target-colorspace-hint`, which mpv's manual says requires
  `--vo=gpu-next`
  ([options](https://mpv.io/manual/stable/#options-target-colorspace-hint)).
- mpv's manual describes `--wid`, which attaches mpv to an existing window,
  for X11, Windows and Android, and not for macOS
  ([options](https://mpv.io/manual/stable/#options-wid)). The render API is
  the way to put mpv's picture in an AppKit view.
- Two upstream pull requests would bring the newer renderer to the render
  API: [#16818](https://github.com/mpv-player/mpv/pull/16818) and
  [#18607](https://github.com/mpv-player/mpv/pull/18607). Both were open
  and unmerged when this was written. Nothing here depends on them.
- Picture in Picture takes its content from an `AVPlayerLayer` or an
  `AVSampleBufferDisplayLayer`
  ([`AVPictureInPictureController.ContentSource`](https://developer.apple.com/documentation/avkit/avpictureinpicturecontroller/contentsource-swift.class)),
  and this render path has neither.
- macOS tracks the Local Network permission by code signature, and Apple
  says that reliable tracking needs an Apple-issued signing identity, which
  an ad hoc signature is not
  ([TN3179](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)).
  If the prompt keeps coming back for the bundled app, signing it with an
  Apple Development certificate is the fix.

## 3. Engineering rules

These are the project's contribution standard. Every change is held to
them.

The Zen of Python is the north star. The code should read as if a Rust core
developer wrote it.

- One obvious way to do each thing. No second code path for the same job.
- Explicit over implicit: no hidden global state, no macros of the project's
  own, types that say what they hold (`RatingKey`, `Millis`, `ServerUrl`,
  not bare strings and integers).
- Flat over nested: early returns, small modules, shallow call trees.
- Errors never pass silently. Fallible calls return a typed error
  (`thiserror`) that is handled or shown in the UI. No `unwrap`/`expect`
  outside tests and startup invariants. No ignored `Result` without a comment
  saying why.
- No guessing. A server response that does not match the expected shape is an
  error carrying the endpoint and the raw field, never a default value.
- Modules are namespaces with one job each.
- If a piece cannot be explained in two sentences, it is redesigned or cut.
- No complex functionality. No plugin system, no settings sprawl, no
  abstraction with a single implementation.
- `unsafe` only in the AppKit glue and the mpv render glue. Each block has a
  `// SAFETY:` line naming the invariant.
- Tests only where they protect real logic: response parsing against recorded
  server JSON, connection ordering, download resume math, marker lookup. No
  test that restates the code. No UI tests.
- Docs only where the reader needs them: why, invariants, units, links to the
  Plex or mpv doc a decision came from. One README. No comment that repeats
  the code.
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` pass on
  every commit.
- History like a real open source project. One commit is one feature, fix or
  implementation step that builds and runs. Imperative subject, body says why.
  Work happens on a branch that is pushed before merging to `main`.

## 4. Building from source

Needed: an Apple Silicon Mac on macOS 26 or later, Rust 1.100 or later with
rustfmt and clippy, Apple's developer tools for clang and swiftc (mpv has
Swift sources, and the build finds the compiler through `xcrun`), and
pkg-config, meson, ninja, nasm and cmake on the `PATH`. Those are build
tools only. None is needed to run the app.

```sh
brew install meson ninja nasm cmake
scripts/build-libmpv.sh    # once: mpv and what it needs, into vendor/
scripts/bundle.sh          # builds target/plex-rust.app
```

`scripts/build-libmpv.sh` downloads pinned releases, checks each archive
against a SHA-256 written in the script, and builds static libraries into
`vendor/libmpv/`: dav1d 1.5.4, FreeType 2.15.0, FriBidi 1.0.18, HarfBuzz
15.1.0, libass 0.17.5, FFmpeg 9.0.2, libplacebo 7.387.0 (cloned at its tag,
because the release archive lacks the submodules the build needs) and mpv
0.42.0. `vendor/` is gitignored, so the repository holds no mpv or FFmpeg
source. Each library is built once, and deleting `vendor/libmpv` starts
over.

- The feature set is minimal. FFmpeg is built for playback only: no
  encoders, muxers, devices or command line tools. mpv is built without
  Lua, JavaScript, VapourSynth, rubberband, Vulkan, libarchive, libbluray,
  lcms2, zimg, uchardet, JPEG, CDDA and DVD support. mpv does its HTTP through the
  libcurl that ships with macOS.
- Four patches in `scripts/patches/` are applied to the unpacked sources,
  three to mpv and one to FFmpeg. `scripts/patches/README.md` says what
  each changes, where it came from and under what license it is
  distributed. A patch added later reaches a tree that was built before it:
  the script applies what is missing and rebuilds that library.
- Homebrew's mpv is not used. Its formula depends on libraries the player
  has no use for, among them vapoursynth, yt-dlp, luajit and a Vulkan stack
  ([formula](https://formulae.brew.sh/formula/mpv)), and all of them would
  have to ship with the app.
- mpv's installed static archive comes out empty, because mpv compiles its
  Swift sources into an archive that `ar` cannot nest. The script assembles
  `libmpv.a` with `libtool` instead.

The `player` crate's build script links what is in `vendor/libmpv` through
pkg-config and fails with a message naming `scripts/build-libmpv.sh` when it
is not there. `cargo build`, `cargo test` and `cargo clippy` over the whole
workspace therefore need the vendor build first. `cargo test -p plex` does
not.

## 5. Architecture

One Cargo workspace, three crates. Dependencies point one way: `app` uses
`plex` and `player`. `plex` and `player` know nothing about each other or
about AppKit.

```
Cargo.toml
crates/
  plex/        pure Rust, no UI, no mpv
    src/auth.rs        PIN login against plex.tv, the account's identifier
    src/discovery.rs   resources list, connection probing and choice
    src/client.rs      one reqwest Client and the headers on every request
    src/server.rs      one server: its requests, and their answers kept
    src/cache.rs       a folder of kept answers, held under a size
    src/library.rs     hubs, sections, items, children, search
    src/playback.rs    part URL, timeline, stream choice, scrobble
    src/download.rs    the original file, in one go or carried on
    src/image.rs       photo transcode requests
    src/model.rs       typed serde models and newtypes
    src/error.rs
    tests/fixtures/    JSON recorded from a real server, titles replaced
  player/      safe wrapper over libmpv
    src/core.rs        handle, options, commands, typed properties, events
    src/render.rs      OpenGL render context
    src/cadence.rs     which display refresh each frame appears at
    src/clock.rs       the clock frame times are measured on
    src/offscreen.rs   drawing where nobody sees it, for checks
    src/probe.rs       timing what mpv does, for `play --seeks`
    src/error.rs
  app/         the AppKit binary
    src/main.rs        the command line
    src/background.rs  tokio runtime + the one bridge back to the main thread
    src/account.rs     sign-in, token, client identifier, the saved server
    src/settings.rs    what is remembered between launches, as JSON files
    src/images.rs      memory + disk artwork cache, ImageIO decode
    src/source.rs      a library item as something the player can load
    src/downloads.rs   downloads as they run
    src/store.rs       downloads on disk
    src/reach.rs       whether the server is there, by what was last heard
    src/video_layer.rs the layer that shows the video, and its frame clock
    src/play.rs        `plex-rust play`, with progress.rs and seeks.rs
    src/pacing.rs      `plex-rust pacing`
    src/timing.rs      timestamps for `plex-rust snapshot walk`
    src/ui/shell.rs    the window: navigation, loading, refresh, history
    src/ui/shell/walk.rs  the scripted walk that times them
    src/ui/            one module a screen or a piece of one: sidebar,
                       toolbar, menus, hero, posters, poster, episode,
                       detail, downloads, library_bar, heading, notice,
                       stale, failure, confirm, artwork, backdrop, formats,
                       text, label, layout, hover, callback, theme, snapshot
    src/ui/player/     the player screen: controls, chips, sliders, menus,
                       input, media keys (remote), up next, zoom, info,
                       failure, and a theme of its own
scripts/build-libmpv.sh  builds the static libmpv into vendor/
scripts/patches/       three patches to mpv 0.42.0, one to FFmpeg 9.0.2
scripts/bundle.sh      builds plex-rust.app, writes Info.plist, ad hoc signs
```

Crates used, at the versions in Cargo.lock: objc2 0.6.5, objc2-foundation /
app-kit / quartz-core / core-graphics / core-video / core-foundation /
image-io / media-player / open-gl 0.3.2, block2 0.6.2, dispatch2 0.3.1,
libmpv2-sys 4.0.1 (the raw bindings; the safe wrapper over them is the
`player` crate's own), libc, reqwest 0.13.5 with native-tls, tokio 1.54.1,
futures-util, url, serde 1.0.229, serde_json 1.0.154, rustix, thiserror.
There is no database.

Threading has three rules. AppKit lives on the main thread. Network and
disk run on a tokio runtime, and `Background::run(main_thread, future, then)`
runs the future off the main thread and hands its result to a closure on the
main queue. It returns a task that can be cancelled, after which the
closure is not called. Video frames are drawn on one more thread, the frame
clock in `video_layer.rs`, so that the interface cannot delay a frame: a
busy main thread used to. Whoever draws holds the lock of the renderer's
OpenGL context. Besides those, each open player has one thread that waits
for mpv's events and passes them to the main queue.

What the app keeps, and where:

| Where | What |
| --- | --- |
| `~/Library/Application Support/plex-rust/` | `token` and `client-identifier`; `server.json`, the server last used with its address, token, machine identifier and the account's identifier; `settings.json`, speed, volume and Low Power; `listings.json`, the order and filter of each library; `place.json`, where the viewer was |
| `~/Library/Caches/plex-rust/responses/` | The server's answers to library requests, up to 64 MiB |
| `~/Library/Caches/plex-rust/images/` | Artwork as the server sent it, up to 512 MiB |
| `~/Movies/plex-rust/` | Downloads (section 9) |

Both cache folders are a `plex::Cache`. A file is named by a hash of the
server's machine identifier, the account's identifier at plex.tv and the
question, so one server has one cache at every address it is reached at,
and two accounts on one server never read each other's answers. A file is
written under a temporary name and renamed. Reading one marks it as used,
and once an eighth of the limit has been written the folder is swept down
to three quarters of it, least recently used first. An answer is kept only
if it decoded, and a kept picture that no longer decodes is fetched again.

## 6. Plex API surface

Source, unless another is named: Plex's OpenAPI description of the media
server, version 1.2.3, at https://developer.plex.tv/pms/.

**Headers on every request:** `X-Plex-Client-Identifier` (a UUID generated
once), `X-Plex-Token`, `X-Plex-Product`, `X-Plex-Version`, `X-Plex-Platform`,
`X-Plex-Device-Name`, `Accept: application/json`.

**Login:** legacy strong PIN flow.
- `POST https://plex.tv/api/v2/pins?strong=true`, open
  `https://app.plex.tv/auth#?clientID=..&code=..` in the browser, poll
  `GET /api/v2/pins/{id}` once per second until `authToken` is set.
- The token is stored in `~/Library/Application Support/plex-rust/token`, readable
  only by its owner. Not the Keychain: it ties access to the exact binary, so
  every rebuild of an ad hoc signed app asks for the login password again.
  `GET https://plex.tv/api/v2/user` gives the account's `uuid`, which keeps
  one account's cache apart from another's.
- The newer JWT flow is not used. Plex's documentation recommends it, but
  developers on Plex's forum report that the media server only accepts
  legacy tokens. The project did not test the JWT flow itself. The PIN flow
  is the one both sources agree works.

**Discovery:** `GET https://clients.plex.tv/api/v2/resources?includeHttps=1`.
Each server is asked with its own `accessToken`. Candidates are its
non-relay `connections` plus one optional address given by hand, for a
network plex.tv does not know about, such as a VPN. Only
`plex-rust servers <address>` passes one today: the app has no field for it.
A remote address without TLS is not a candidate, since it would carry the
token in clear. All candidates are probed in parallel with `GET /identity`,
each given 3 seconds, and one counts only if the machine identifier in its
answer is the server's. The first to answer in this order wins: local https,
local http, manual address, remote https. No candidate reachable is an error
screen that names what was tried. The server found is saved, and the next
launch asks it directly, going back to plex.tv only if it does not answer
within 3 seconds.

**Browsing:** `/hubs`, `/library/sections/all`,
`/library/sections/{id}/all` (paged with `X-Plex-Container-Start` and
`X-Plex-Container-Size`, total from `totalSize`), `/library/metadata/{id}`,
its `/children` and `/allLeaves`, `/hubs/metadata/{id}/related`,
`/hubs/search?query=`, and the paths the server itself hands out for the
whole of a shelf.

**Images:** `/photo/:/transcode?url=..&width=..&height=..&minSize=1&format=jpg`
requested at the pixel size each kind of card is drawn at.

**Playback:** direct play only. The stream URL is the server address plus
`Part.key` (`/library/parts/{partId}/{changestamp}/{file}`). The token is sent
as a header, never in the URL.

**Progress:** `POST /:/timeline` with `ratingKey`, `key`, `state`, `time`,
`duration` and `X-Plex-Session-Identifier`, on every state change and every
10 seconds while playing. `PUT /:/scrobble` and `/:/unscrobble` for the manual
watched toggle.

**Markers:** `includeMarkers=1` on the item request. The parameter is not in
Plex's description. It is in the community-maintained description and in
[python-plexapi](https://github.com/pkkid/python-plexapi). Marker type is
matched as `intro`, and both `credit` and `credits`, because Plex's
description is not consistent on that name.

**Stream choice:** `PUT /library/parts/{partId}?audioStreamID=&subtitleStreamID=`
so the choice is remembered server side.

**Downloads:** `GET {Part.key}?download=1`. An account can be denied
downloads, and a server that refuses is asked again without the parameter,
as a player asks.

## 7. Playback

Options are from the [mpv manual](https://mpv.io/manual/stable/), read at
0.42. File names are in the mpv 0.42.0 source.

**Pipeline:** `vo=libmpv`, `hwdec=videotoolbox`. With the OpenGL render API on
macOS the VideoToolbox frames reach the renderer without a copy, bound as
textures with `CGLTexImageIOSurface2D` (video/out/hwdec/hwdec_mac_gl.c).
The video view hosts a `CAOpenGLLayer` subclass that draws only when mpv has
a new frame. A display link times each frame to the display refresh nearest
the moment mpv wants it shown. The link ticks only while frames are coming,
and pauses about a second after the last one, so a paused player wakes
nothing.

**Quality:** always the original file, no server transcode. mpv scaler
defaults stay (`scale=lanczos`, `dscale=hermite`). If a file cannot be played
the viewer sees mpv's error, not a silent fallback.

**HDR:** HDR10 and HLG are shown the way [IINA](https://github.com/iina/iina)
shows them: when the picture is PQ or HLG and the screen reports EDR
headroom, the layer sets `wantsExtendedDynamicRangeContent` with a PQ
colorspace and mpv gets `target-trc=pq` and `target-prim`. `target-peak`
stays at mpv's default.

**Dolby Vision:** the OpenGL renderer applies none of it. `init_video` in
mpv's video/out/gpu/video.c puts the picture's own colour tags back, and
`mp_get_csp_matrix` in video/csputils.c marks the Dolby Vision matrix "Not
supported". mpv applies it only in `gpu-next`, which the render API does
not offer, and FFmpeg's `libplacebo` filter depends on Vulkan (`configure`:
`libplacebo_filter_deps="libplacebo vulkan"`), which this build leaves out.
So `vf=format:dolbyvision=no` takes the metadata off the frames, which stay
in the hardware decoder's buffers, and what mpv then reports in
`video-out-params` is the picture that is drawn. That picture decides
between HDR and SDR output.
- Profile 8.1 plays as the HDR10 underneath it. Measured once, on one
  frame of a public sample: the rendered frame was within 0.002 on average,
  in PQ code values from 0 to 1, of the same frame decoded by AVFoundation,
  and it was the same with and without the filter.
- Profile 8.4 plays as the HLG underneath it, converted to PQ by mpv.
- Profile 7 has HDR10 underneath and is expected to play as that. This has
  not been checked: no sample was at hand. mpv decodes its enhancement
  layer with a second decoder even though this renderer has no use for it
  (filters/f_enhancement_pair.c).
- Profile 5 has no picture underneath that stands on its own, and comes out
  in the wrong colours. When
  `current-tracks/video/dolby-vision-profile` is 5 the player says so in a
  notice for 8 seconds after the file opens.
- `plex-rust play ... --unattended` reports `dolby_vision_profile`.

**Buffering:** `cache=yes`, `demuxer-max-bytes=2GiB`,
`demuxer-max-back-bytes=512MiB`, `demuxer-hysteresis-secs=10`. The manual
says of the last one that it "can provide significant power savings", by
having the demuxer read ahead in chunks, and that 10 seconds "probably
works well for most usecases". The buffer is in RAM, not on disk. The
scrubber draws the buffered range from `demuxer-cache-state`.

**Speed:** `speed` from 0.1 to 4.0. The keys move it by 0.1 and the menu
offers fixed speeds from 0.5 to 3.0. mpv inserts `scaletempo2` for pitch
correction by default, and that filter mutes the sound below 0.25x and
above 8x unless told otherwise (manual, audio filters). `video-sync=audio`
and `framedrop=vo` stay at defaults. No interpolation.

**Power:**
- Hardware decode, zero-copy, draw only on new frames.
- One timer ticks 4 times a second while a player is open, and the 10
  second timeline rides on it. With the controls up, each tick reads the
  position. With them hidden, the position is read once a second, and only
  for an item with an intro or credits to skip or an episode to follow.
  Paused with the controls away, or after a failure, the timer is put off
  until something happens.
- The system Low Power Mode is never consulted, so picture quality is the
  same whether it is on or off.
- The plex-rust menu has one Low Power item, off by default and checked
  while on. On, it applies mpv's built in `fast` profile: bilinear scaling,
  no dither, no HDR peak computation (`[fast]` in mpv's etc/builtin.conf).
  It takes effect on the playing file immediately. A menu item and not a
  Settings window, because one switch does not fill a window.
- Display sleep is held off with `NSProcessInfo.beginActivity` only while
  playing, released on pause.

**Seeking:** a first press of an arrow key is an exact relative seek. While
the key is held, and while the scrubber's knob is held, seeks go keyframe to
keyframe, because an exact seek takes longer than the gap between repeats.
Resume starts on the keyframe before the saved position. Three patches in
`scripts/patches/` cut the stall after a jump. Two are to mpv 0.42.0: the
picture restarts before the sound, which joins in sync once it can be heard,
and a short jump forward decodes on from the current frame without going
back to the keyframe. One is to FFmpeg 9.0.2: the hardware decoder is handed
the next frame without a wait for the one before. An exact seek decodes
from the keyframe before its target, so its length is that decode. On one
machine, an M5 Max, a 4K HEVC frame took 2.35 ms to decode before the patch
and 1.2 ms with it. Each patch's header and the commit that added it carry
the measurements and their conditions.
`plex-rust play item:<key> --seeks 3x10,-3x10` measures all of it.

**Subtitles and audio:** mpv renders embedded text and image subtitles
itself. Subtitles Plex keeps in files of their own, which are the streams it
lists with a `key`, are added with `sub-add` by their address on the server
once the video is open, with the same token header. Track menus come from
`track-list`, named as the server names each stream. A download keeps those
files next to its video, and played from disk it gets them from there.

**Skip intro / credits:** a button shown while the position is inside a
marker. **Next episode:** on end of file, play the episode that follows in
the show, into the next season when a season ends. Specials stay apart.

## 8. UI

A sidebar, shelves, poster grids and detail pages, with a current macOS
look and a dark appearance.

**Design language.** Everything is the platform's own kit:
- **Liquid Glass**, the macOS 26 design. Standard AppKit components such as
  sidebars and toolbars take the material by themselves
  ([Adopting Liquid Glass](https://developer.apple.com/documentation/technologyoverviews/adopting-liquid-glass)).
  The player controls and the skip button sit on `NSGlassEffectView`, which
  objc2-app-kit 0.3.2 exposes.
- **Icons:** SF Symbols through `NSImage(systemSymbolName:)`. They ship with
  macOS, scale with text and match the system weight.
- **Type:** the system font (SF Pro), with fixed sizes and weights defined
  once in `ui/theme.rs` along with spacing, corner radii and colours. No
  literal sizes or colours anywhere else.
- **Colour from the artwork:** detail pages and the home hero use the
  `UltraBlurColors` four-corner gradient that the server returns per item.
- **Motion:** system animations only: poster hover lift, shelf scrolling,
  cross fade between pages, controls fade. Nothing custom runs while idle.
- Poster cells: rounded corners, progress bar for partly watched, a tick once
  watched, the number of episodes left on a show, title and year below.

- **Window:** `NSSplitViewController`. Sidebar: Home, one row per Movie/TV
  library, Downloads. Toolbar: back, search field. The sidebar has no
  `NSVisualEffectView` under it: it is the system's glass, laid over the
  content.
- **Home:** `NSCollectionView` with a compositional layout, one horizontally
  scrolling shelf per hub: Continue Watching, On Deck, Recently Added.
- **Library:** poster grid, paged while scrolling, sort menu.
- **Detail:** backdrop, poster, title, year, rating, summary, Play / Resume,
  watched toggle, download button, audio and subtitle pickers. Shows add a
  season picker and episode list.
- **Search:** results grouped by hub.
- **Player:** video fills the window, controls fade out: scrubber with
  buffered range and chapter ticks, time, speed, audio/subtitle menus, skip
  button, fullscreen. Space, arrows, `S` and `D` for speed, `R` for normal
  speed, `F` for fullscreen: the README lists every key.
  Media keys through `MPRemoteCommandCenter`, and what is playing through
  `MPNowPlayingInfoCenter`, both held only while a player is open and
  someone is watching: a snapshot or other check never takes them.

Collection views use the classic `NSCollectionViewDataSource` protocol.
`NSCollectionViewDiffableDataSource` has no usable initializer in
objc2-app-kit 0.3.2.

**Why it feels fast:** every screen draws first from the response cache,
then asks the server and updates in place where the answer differs. Posters
are requested at the size they are drawn, decoded off the main thread with
ImageIO, kept in a memory cache and a disk cache. Cells are reused.

**Between and behind the screens:**
- A launch starts where the last one ended: the same window, and from Home
  on to the library, list, page or Downloads that was on screen.
- Back and Forward show a place as it was left, scrolled as it was, and
  then ask the server about it again.
- When the pointer or the keyboard has been on a card for 120 ms, what its
  page will ask is asked ahead and its backdrop fetched, so the page opens
  from saved answers.
- Leaving a place gives up what it was still asking. The search field
  searches at every letter, and the search for the letters before is given
  up, so an earlier answer cannot replace a later one.
- A grid is refreshed with every page it holds, not its first.
- Coming back to the app after more than 30 seconds asks about the place on
  screen again. After the player closes, the place behind it is asked about
  once the server has heard where the viewer stopped.
- When the server does not answer, what was saved stays, with a line at the
  bottom that says so and a button to look for the server again. An error
  page takes the screen only when nothing was saved for it, or when the
  sign-in is over. An item's page is never nothing: it stands as the card
  it was opened from drew it, and for a downloaded item as the record next
  to its file has it.
- Whether the server is there is one state, `reach.rs`, moved by what every
  request the app makes anyway comes to: any answer says it is, no
  connection or no reply says it is not. Nothing polls. The line, the strip
  on Downloads and whether a download plays from disk without asking all
  go by it. Without any server known, search is greyed out and says why.

## 9. Offline downloads

- Original file to `~/Movies/plex-rust/`, written as `.part`, resumed with an HTTP
  `Range` request, renamed when the byte count equals `Part.size`. A size
  mismatch is an error, not a retry loop.
- Of an item in several versions, the one picked on its page. A version
  split across files is refused: a download is one file, as is what plays.
- The folder is made by the first download. A download that cannot start
  says why in a sheet, and one that stops says why in its row, a full
  disk and a folder that may not be written to in so many words. Downloads
  shows how much its files take and how much more the disk has room for.
- The item's metadata JSON, with the downloaded version first, its poster
  and the subtitle files the server keeps beside the video are stored next
  to it so Downloads works with no server. Those files are the only record
  of a download.
- Offline playback hands mpv the local path. Position is stored next to the
  file, when there is no server and whenever the server does not take a
  report, and sent as one timeline call with `state=stopped` and
  `offline=1` when the server is next reached.
  - `offline` is in Plex's OpenAPI description 1.2.3 for `POST /:/timeline`,
    as an integer that is 0 or 1: "Also used by sync clients, used to
    indicate that a timeline is being synced from being offline, as opposed
    to being \"live\"". What the server does differently on seeing it is not
    written there and has not been tried, since trying it means a write to
    a real server's watch state.
  - The same description has `updated` next to it, an integer: "Used when a
    sync client comes online and is syncing media timelines, holds the time
    at which the playback state was last updated". It gives no unit, and
    its example, 14200000, reads as neither seconds nor milliseconds since
    1970. It is not sent, so the server takes the viewing to have ended
    when it hears of it. python-plexapi's `updateTimeline` sends neither
    parameter.

## 10. Packaging: one self-contained app

- A Mac GUI app has to be a `.app` bundle to carry its Info.plist and icon,
  so the deliverable is `plex-rust.app`, not a bare file. Inside it there is one
  executable and no bundled dylibs: libmpv and everything under it are linked
  statically. `otool -L` on the executable must list only `/usr/lib` and
  `/System/Library` paths. That is the acceptance check.
- The app is meant to run on another Apple Silicon Mac as copied, with no
  Homebrew and no mpv installed there.
- `scripts/bundle.sh` builds the release binary, makes the `.icns` from
  `assets/icon.png`, writes an Info.plist (`LSMinimumSystemVersion` 26.0,
  `NSHighResolutionCapable`, `NSLocalNetworkUsageDescription`) and signs
  the bundle ad hoc with `codesign --sign -`. It is not notarized and not
  sandboxed.
- mpv 0.42.0 as released does not compile with Vulkan disabled (upstream
  issue [#18817](https://github.com/mpv-player/mpv/issues/18817)). The fix
  from upstream pull request
  [#18818](https://github.com/mpv-player/mpv/pull/18818) is applied as a
  patch file in `scripts/patches/`.
- **License:** plex-rust is distributed under GPL-2.0-or-later. The
  executable statically links mpv built with `-Dgpl=true` and FFmpeg built
  with `--enable-gpl`, and both are under the GPL, version 2 or later, when
  built that way (mpv's `Copyright` file, FFmpeg's `LICENSE.md`). A binary
  that contains them has to be distributed under the same terms, so the
  whole project is.

## 11. Testing without a person

The binary carries its own checks, as subcommands of `plex-rust`. They let
a change be checked from a terminal, with nobody watching a window. All
but the unit tests need a signed-in account (`plex-rust login`) and a Plex
Media Server to ask. `play` and `pacing` also take a plain file or URL and
then need neither.

| Need | Command | What it captures |
| --- | --- | --- |
| The Plex client is right | `cargo test` | Response parsing against the recorded fixtures, connection ordering, download resume, the cache, and the app's own logic where it has any |
| What the servers say | `plex-rust servers [<address>]`, `plex-rust api [--post] <path>` | Each server of the account with the address it answers at, and the first server's raw answer to a request |
| The interface looks right | `plex-rust snapshot <screen> <png> [<tweak>...]` | A PNG of the window showing Home, a library, an item, a search, a list, Downloads, the sign-in or the player |
| Nothing makes the viewer wait | `plex-rust snapshot walk <png> <step>...` | A line on stderr with the milliseconds since launch for each step taken and each thing shown, then a PNG |
| Video plays right | `plex-rust play <file-url-or-item> --unattended <seconds> [--low-power]` | One line of JSON after that long: position, buffer, hardware decoder, dropped frames, colours, Dolby Vision profile, whether the layer asked for HDR, drawn size, scaler settings, tracks, `cpu_share`, the seconds to the first picture, and whether the window was visible |
| Jumps are short | `plex-rust play <file-url-or-item> --seeks <seconds>x<presses>[@<ms>],... [speed=<rate>] [subtitles=<id>\|off] [paused]` | For each step, as p50, p90 and max over its presses: milliseconds from the press to the picture shown, to mpv's restart, to sound handed over, and to the picture moving, plus the A/V offset after, jumps that missed their target and jumps that did not finish |
| Frames are even | `plex-rust pacing <file-url-or-item> <seconds> <speed>\|paused...` | One JSON report for each stage: the display's refresh intervals, the gaps between drawn frames against the gaps in the content, how late frames were, processor time in all and by thread, and mpv's own drop and delay counters. `paused` measures that nothing is drawn and no processor time is used while the picture stands still, and how soon the picture of a jump made then is drawn |

How they work, and where they stop:

- **Snapshots.** The window is put on screen in the window server's sense
  at a place no display reaches, and the PNG is the window server's own
  picture of it, glass, sidebar and toolbar included. A process may read
  back its own windows, so no Screen Recording permission is asked for.
  The window takes no keyboard focus and gets no Dock icon. Tweaks put the
  screen in a state first: narrow or wide, scrolled, hovered, keys pressed,
  a sort order, a failure (`fault=...`), the server away, or the system's
  Accessibility settings as if they were on. A snapshot leaves the server
  as it found it. It shows what the server holds at that moment, so two
  snapshots can only be compared against the same library. Nothing
  compares pictures: someone, or something, has to look at the PNG.
- **Walks.** Steps are taken 1.5 seconds apart, so that what one started
  has answered before the next. The times are those of one run against one
  server on one network.
- **`play --unattended`.** The run is muted, takes no focus, reports
  nothing to the server so that a check never moves anyone's place in a
  film, prints its JSON and exits. macOS does not draw a window that is
  covered or on another Space, and mpv counts every frame that was not
  drawn as dropped, so the drop count means something only when
  `window_visible` is true.
- **`play --seeks`.** The video is drawn off screen, because a covered
  window is not drawn at all and that changes the timings. `3x20` presses
  3 seconds forward 20 times and waits for each to finish. `-3x30@80`
  holds 3 seconds back: 30 presses, 80 ms apart. The run is muted, so when
  sound is heard rests on the audio device's own report of its latency.
- **`pacing`.** It times frames against the refreshes the display link
  reports and records whether the window was visible. Every figure is a
  distribution over the frames of the measured stretch.

The checks measure processor time, never watts. `top -o power` and
`powermetrics` are the system's tools for that. The second needs
administrator rights and calls its own figures estimates, so only a
comparison on one Mac under the same conditions says anything.

Runs started from a terminal do not meet the Local Network prompt: macOS
allows command line tools run from Terminal or over SSH, and their child
processes, by itself (TN3179, linked in section 2). The bundled app, opened
from Finder, does meet it the first time it reaches an address on the local
network.

The code that exists only for checks is `snapshot` with its walk, the
checks of `play` (`--unattended` and `--seeks`), and `pacing`.

## 12. Status and known limits

**What works.** Signing in, finding a server and choosing an address.
Home, libraries with sorting and filtering, detail pages, search. Playing
the original file with hardware decoding, HDR10 and HLG output, track
choice, subtitles in the file and beside it, markers, next episode, speed,
media keys. Downloads, playing them with no server, and telling the server
afterwards how far the viewer got. One self-contained app bundle.

**Not built.**
- Music, Live TV / DVR, Photos.
- Transcoding. A file that mpv cannot play does not play.
- Relay connections. A server reachable only through Plex's relay is not
  reachable here.
- A field in the app for a server address given by hand. Discovery takes
  one (section 6), but only `plex-rust servers <address>` passes it.
- A choice among several servers. The app uses the first server the
  account lists.
- A Settings window.
- Notarization, a sandbox, and a build for Intel Macs.

**Not possible on this render path** (reasons in sections 2 and 7).
- Dolby Vision is not applied. Profile 5 plays in the wrong colours, and
  the player says so. Profiles 8.1 and 8.4 play as the HDR10 or HLG
  underneath them.
- Picture in Picture.
- `gpu-next` and what only it has, `target-colorspace-hint` among them.

**Not confirmed.**
- Dolby Vision profile 7 is expected to play as its HDR10 layer. No sample
  was tried.
- The measurements in the repository's history were made on H.264 and HEVC
  files. No commit records an AV1 file being played. The build includes
  dav1d to decode it.
- What a server does on `offline=1` in a timeline report, beyond accepting
  the parameter its description lists (section 9).
- Whether the JWT sign-in works against a media server (section 6).
- Whether an ad hoc signed bundle keeps the Local Network permission
  across rebuilds (section 2).
- Energy use. Processor time is measured by the checks in section 11. No
  measurement in watts, and no comparison with another player, is
  recorded.

Every measurement quoted in this document or in a commit message was taken
on one machine, with the file kind, the audio output and the number of
runs given where it is quoted. They describe that run and promise nothing
about another machine.

## 13. Verifying a change

On every commit, after `scripts/build-libmpv.sh` has run once:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Then, by what the change touches, with the commands of section 11:

- **The `plex` crate.** A response the client reads gets a test against a
  recorded fixture. `plex-rust api <path>` prints what a server really
  answers. Fixtures are recorded from a real server and their titles
  replaced before they are committed.
- **A screen.** A snapshot of it before and after, in the states the
  change can reach: narrow and wide, a failure, the server away. Screens
  that draw from saved answers are checked with the server away as well.
- **Anything a viewer waits for.** A walk through it, with a cold cache
  and with a warm one. The times go in the commit message with what was
  walked.
- **The player.** `play --unattended` on a file of each kind the change can
  affect: the report has to name the hardware decoder, show no dropped
  frames while `window_visible` is true, and show the colours expected. A
  change near seeking runs `--seeks` forward, backward and held. A change
  near drawing runs `pacing` at normal speed, at a high speed and paused.
- **The mpv or FFmpeg build.** A change to a patch, a version or a build
  option is followed by the player checks above, and its patch file is
  described in `scripts/patches/README.md`.
- **Packaging.** `scripts/bundle.sh`, then `otool -L` on
  `target/plex-rust.app/Contents/MacOS/plex-rust`, which must list only
  `/usr/lib` and `/System/Library` paths, then the play report from the
  bundled binary.

A measurement in a commit message says what was played, on what class of
machine, through which audio output, how many runs, and what was not
measured.
