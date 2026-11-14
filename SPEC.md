# Spec: plex-rust, a native macOS Plex client in Rust

The app is named plex-rust at its owner's choice. It is a personal, third-party
client: it uses no Plex logo or artwork, and its icon is its own.

Every claim below is tagged where it matters:
- [V] verified against a primary source or on this Mac on 2026-11-13
- [U] not verified. Each [U] is closed by a named check in Milestone 0 before
  anything is built on top of it.

## 1. Context

The official desktop app at /Applications/Plex.app (1.115.0) ships Qt Quick and
QtWebEngine frameworks [V]. The goal is a personal replacement with a very
similar layout that browses faster, always plays the original file, buffers far
ahead, stays smooth at high playback speed, and uses as little battery as
possible.

/Users/batuhan/projects/native-plex-client is empty and not a git repo [V].

## 2. Decisions

| Topic | Decision |
| --- | --- |
| Language | Rust only, stable (rustc 1.100.0 via rustup, already installed) |
| UI | AppKit through objc2 0.6.5 and objc2-app-kit 0.3.2. No Swift, no web view |
| Playback | libmpv 0.42.0 embedded in our own NSView through the OpenGL render API |
| v1 scope | Movies + TV, offline downloads of original files |
| Not in v1 | Music, Live TV / DVR, Photos, transcoding, relay connections, Picture in Picture |
| Network | LAN or Tailscale. Direct connections only |
| Audience | Personal use, Apple Silicon, macOS 26+, no sandbox, ad hoc signed |
| Packaging | One self-contained plex-rust.app holding one executable that links only macOS system libraries. Nothing to install to run it |
| Quality vs power | Quality is always first. The app never reads the macOS Low Power Mode state. One in-app "Low power" switch, off by default |
| Execution | Built end to end without a human, after two one-time actions listed in section 11 |

### Known limits of these decisions (accepted)

- mpv's render API offers only OpenGL, Direct3D 11 and software, and its OpenGL
  backend runs the older `vo=gpu` renderer, not `gpu-next` [V]. So: no Dolby
  Vision reshaping, no `target-colorspace-hint`, and no direct-to-display
  presentation (that needs fullscreen + an opaque CAMetalLayer [V]).
- `--wid` embedding was removed on macOS [V].
- Upstream PRs #16818 and #18607 would bring the newer renderer to the same
  render API. Both are open, not merged [V]. Nothing in this spec depends on
  them.
- No credible watt-level comparison of mpv/IINA against AVPlayer exists [V].
  Power is therefore measured in Milestone 0, not assumed.
- Ad hoc signed apps may not keep the Local Network permission reliably [V].
  If the prompt keeps returning, the fix is an Apple Development certificate.
- Native PiP needs an AVSampleBufferDisplayLayer [V], which this path lacks.

## 3. Engineering rules (binding)

The Zen of Python is the north star. The code should read as if a Rust core
developer wrote it.

- One obvious way to do each thing. No second code path for the same job.
- Explicit over implicit: no hidden global state, no macros of our own, types
  that say what they hold (`RatingKey`, `Millis`, `ServerUrl`, not bare
  strings and integers).
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

## 4. Setup (first actions after approval)

1. Save the engineering rules to project memory (could not be done in plan
   mode).
2. `brew install meson ninja nasm cmake` (none are installed [V]). They are
   build tools only and are not needed to run the app.
3. `scripts/build-libmpv.sh` builds a static libmpv from pinned source
   tarballs: FFmpeg, dav1d, libass with its text stack, libplacebo, mpv
   0.42.0. Minimal feature set: no Lua, no JavaScript, no VapourSynth, no
   rubberband, no yt-dlp, no Vulkan. Output goes to `vendor/libmpv/`, which is
   gitignored and cached between builds.
   - Homebrew's mpv is not used: its formula depends on vapoursynth, yt-dlp,
     luajit, mujs, rubberband, libbluray, vulkan-loader and molten-vk [V], and
     that whole tree would have to ship with the app.
   - mpv is GPL by default [V]. Fine for personal use.
4. `git init`, private GitHub repo `plex-rust`, one branch per milestone, pushed
   before merging to `main`.
5. Already present [V]: rustfmt, clippy, rust-analyzer, clang 21, pkg-config,
   codesign, install_name_tool, plutil.

## 5. Architecture

One Cargo workspace, three crates. Dependencies point one way: `app` uses
`plex` and `player`. `plex` and `player` know nothing about each other or
about AppKit.

```
Cargo.toml
crates/
  plex/        pure Rust, no UI, no mpv
    src/auth.rs        PIN login against plex.tv
    src/discovery.rs   resources list, connection probing and choice
    src/client.rs      one reqwest Client, headers, JSON, pagination
    src/library.rs     hubs, sections, items, children, search
    src/playback.rs    part URL, timeline, scrobble, markers
    src/image.rs       photo transcode URLs
    src/model.rs       typed serde models and newtypes
    src/error.rs
    tests/fixtures/    recorded JSON from the real server, tokens removed
  player/      safe wrapper over libmpv
    src/core.rs        handle, options, commands, typed properties, events
    src/render.rs      OpenGL render context
    src/error.rs
  app/         the AppKit binary
    src/main.rs        NSApplication, delegate, menu
    src/runtime.rs     tokio runtime + the one bridge back to the main thread
    src/store.rs       SQLite: response cache, downloads
    src/account.rs     sign-in, token and client identifier storage
    src/images.rs      disk + memory image cache, ImageIO decode
    src/downloads.rs   resumable original-file downloads
    src/ui/            window, sidebar, home, library, detail, search,
                       downloads, player (video layer, controls, hdr)
scripts/bundle.sh      builds plex-rust.app, writes Info.plist, ad hoc signs
```

Crates used, all current versions confirmed on crates.io [V]: objc2 0.6.5,
objc2-foundation / app-kit / quartz-core / image-io / io-kit 0.3.2, block2
0.6.2, dispatch2 0.3.1, libmpv2 7.0.1 (LGPL-2.1, exposes the OpenGL render
API), reqwest 0.13.5, tokio 1.54.1, serde 1.0.229, serde_json 1.0.154,
rusqlite 0.42.0, thiserror.

Threading has two rules. AppKit lives on the main thread; network and disk
run on a tokio runtime, and `runtime::spawn(future, on_main)` runs the future
off the main thread and delivers its `Result` to a closure on the main queue.
Video frames are drawn on one more thread, the frame clock in
`video_layer.rs`, so that the interface cannot delay a frame: a busy main
thread used to. Whoever draws holds the lock of the renderer's OpenGL
context. No other cross-thread path exists.

## 6. Plex API surface

Source: Plex's official OpenAPI spec 1.2.3 at https://developer.plex.tv/pms/
[V] unless marked.

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
  `GET https://plex.tv/api/v2/user` returning 401 means log in again.
- The newer JWT flow is not used: Plex's doc recommends it, but developers on
  Plex's forum report that the media server only accepts legacy tokens. The
  PIN flow is the one both sources agree works.

**Discovery:** `GET https://clients.plex.tv/api/v2/resources?includeHttps=1`.
Use each server's `accessToken`. Candidates are its non-relay `connections`
plus one optional address typed by the user (for Tailscale). All candidates
are probed in parallel; the first reachable one in this order wins: local
https, local http, manual address, remote https. No candidate reachable is an
error screen listing what was tried.

**Browsing:** `/hubs`, `/hubs/continueWatching`, `/library/sections/all`,
`/library/sections/{id}/all` (paged with `X-Plex-Container-Start` and
`X-Plex-Container-Size`, total from `X-Plex-Container-Total-Size`),
`/library/metadata/{id}`, `/children`, `/hubs/search?query=`.

**Images:** `/photo/:/transcode?url=..&width=..&height=..&minSize=1&format=jpg`
requested at the exact pixel size of the cell.

**Playback:** direct play only. The stream URL is the server address plus
`Part.key` (`/library/parts/{partId}/{changestamp}/{file}`). The token is sent
as a header, never in the URL.

**Progress:** `POST /:/timeline` with `ratingKey`, `key`, `state`, `time`,
`duration` and `X-Plex-Session-Identifier`, on every state change and every
10 seconds while playing. `PUT /:/scrobble` and `/:/unscrobble` for the manual
watched toggle.

**Markers:** `includeMarkers=1` on the item request (present in the community
spec and python-plexapi, not the official spec). Marker type is matched as
`intro`, and both `credit` and `credits`, because Plex's own spec is
inconsistent on that name [V].

**Stream choice:** `PUT /library/parts/{partId}?audioStreamID=&subtitleStreamID=`
so the choice is remembered server side.

**Downloads:** `GET {Part.key}?download=1`.

## 7. Playback

Options are from the mpv 0.42 manual [V].

**Pipeline:** `vo=libmpv`, `hwdec=videotoolbox`. With the OpenGL render API on
macOS the VideoToolbox frames reach the renderer without a copy
(`CGLTexImageIOSurface2D`) [V]. The video view hosts a `CAOpenGLLayer`
subclass that draws only when mpv has a new frame. A display link times each
frame to the display refresh nearest the moment mpv wants it shown. The link
ticks only while frames are coming, and pauses about a second after the last
one, so a paused player wakes nothing.

**Quality:** always the original file, no server transcode. mpv scaler
defaults stay (`scale=lanczos`, `dscale=hermite`). If a file cannot be played
the user sees mpv's error, not a silent fallback.

**HDR:** HDR10 and HLG follow IINA's working method [V]: when the picture is
PQ/HLG and the screen reports EDR headroom, the layer sets
`wantsExtendedDynamicRangeContent` with a PQ colorspace and mpv gets
`target-trc=pq` and `target-prim`. `target-peak` stays at mpv's default.

**Dolby Vision:** the OpenGL renderer applies none of it. `init_video` in
mpv's video/out/gpu/video.c puts the picture's own colour tags back, and
`mp_get_csp_matrix` in video/csputils.c marks the Dolby Vision matrix "Not
supported" [V]. mpv applies it only in `gpu-next`, which the render API does
not offer, and FFmpeg's `libplacebo` filter needs Vulkan, which this build
leaves out [V]. So `vf=format:dolbyvision=no` takes the metadata off the
frames, which stay in the hardware decoder's buffers, and what mpv then
reports in `video-out-params` is the picture that is drawn. That picture
decides between HDR and SDR output.
- Profile 8.1 plays as the HDR10 underneath it. A frame of a public sample
  is within 0.002 on average, in PQ code values from 0 to 1, of the same
  frame decoded by AVFoundation [V]. The rendered frame is the same with and
  without the filter [V].
- Profile 8.4 plays as the HLG underneath it, converted to PQ by mpv [V].
- Profile 7 has HDR10 underneath and is expected to play as that [U]: no
  sample was at hand. mpv decodes its enhancement layer with a second
  decoder even though this renderer has no use for it
  (filters/f_enhancement_pair.c).
- Profile 5 has no picture underneath that stands on its own, and comes out
  in the wrong colours [V]. When
  `current-tracks/video/dolby-vision-profile` is 5 the player says so in a
  notice for 8 seconds after the file opens.
- `plex-rust play ... --unattended` reports `dolby_vision_profile`.

**Buffering:** `cache=yes`, `demuxer-max-bytes=2GiB`,
`demuxer-max-back-bytes=512MiB`, `demuxer-hysteresis-secs=10` (the manual
names this option for "significant power savings"). In RAM, not on disk. The
scrubber draws the buffered range from `demuxer-cache-state`. The three values
are starting points, tuned once in Milestone 0.

**Speed:** `speed` from 0.25 to 4.0 in fixed steps. mpv inserts `scaletempo2`
for pitch correction by default and mutes above 8x [V]. `video-sync=audio`
and `framedrop=vo` stay at defaults. No interpolation.

**Power:**
- Hardware decode, zero-copy, draw only on new frames.
- While the controls are hidden nothing polls. While visible, position is
  read 4 times per second. The only other timer is the 10 second timeline.
- The system Low Power Mode is never consulted, so picture quality is the
  same whether it is on or off.
- The plex-rust menu has one Low Power item, off by default and checked
  while on. On, it applies mpv's built in `fast` profile (bilinear scaling,
  no dither, no HDR peak computation [V]). It takes effect on the playing
  file immediately. A menu item and not a Settings window, because one
  switch does not fill a window.
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
the next frame without a wait for the one before, which halves the time it
takes to decode from a keyframe up to the target.
`plex-rust play item:<key> --seeks 3x10,-3x10` measures all of it.

**Subtitles and audio:** mpv renders embedded text and image subtitles
itself. Plex sidecar subtitles are added by URL with `sub-add`. Track menus
come from `track-list`.

**Skip intro / credits:** a button shown while the position is inside a
marker. **Next episode:** on end of file, play the episode that follows in
the show, into the next season when a season ends. Specials stay apart.

## 8. UI

Same layout as the Plex desktop app, with a current macOS look, dark
appearance.

**Design language.** Web component kits (shadcn, Radix and similar) cannot
run in AppKit, so "reuse" here means the platform's own modern kit plus open
icon and font assets:
- **Liquid Glass**, the macOS 26/27 design. Sidebar and toolbar get it from
  AppKit automatically [V]. The player controls and the skip button sit on
  `NSGlassEffectView`, which objc2-app-kit 0.3.2 exposes [V].
- **Icons:** SF Symbols through `NSImage(systemSymbolName:)`. They ship with
  macOS, scale with text and match the system weight. Where SF Symbols has no
  fitting glyph, the icon comes from Lucide (open source SVG set), embedded
  in the binary.
- **Type:** the system font (SF Pro), with fixed sizes and weights defined
  once in `ui/theme.rs` along with spacing, corner radii and colours. No
  literal sizes or colours anywhere else.
- **Colour from the artwork:** detail pages and the home hero use the
  `UltraBlurColors` four-corner gradient that Plex returns per item [V], the
  same source the current Plex apps use for their backgrounds.
- **Motion:** system animations only: poster hover lift, shelf scrolling,
  cross fade between pages, controls fade. Nothing custom runs while idle.
- Poster cells: rounded corners, progress bar for partly watched, a tick once
  watched, the number of episodes left on a show, title and year below.

- **Window:** `NSSplitViewController`. Sidebar: Home, one row per Movie/TV
  library, Downloads. Toolbar: back, search field. No `NSVisualEffectView` in
  the sidebar (Apple: it blocks the system glass) [V].
- **Home:** `NSCollectionView` with a compositional layout, one horizontally
  scrolling shelf per hub: Continue Watching, On Deck, Recently Added.
- **Library:** poster grid, paged as you scroll, sort menu.
- **Detail:** backdrop, poster, title, year, rating, summary, Play / Resume,
  watched toggle, download button, audio and subtitle pickers. Shows add a
  season picker and episode list.
- **Search:** results grouped by hub.
- **Player:** video fills the window, controls fade out: scrubber with
  buffered range and chapter ticks, time, speed, audio/subtitle menus, skip
  button, fullscreen. Space, arrows, `[` `]` for speed, `F` for fullscreen.
  Media keys through `MPRemoteCommandCenter`, and what is playing through
  `MPNowPlayingInfoCenter`, both held only while a player is open and
  someone is watching: a snapshot or other check never takes them.

Collection views use the classic `NSCollectionViewDataSource` protocol.
`NSCollectionViewDiffableDataSource` has no usable initializer in
objc2-app-kit 0.3.2 [V].

**Why it will feel faster:** every screen draws first from the SQLite
response cache, then refreshes from the server and updates in place. Posters
are requested at exact cell size, decoded off the main thread with ImageIO,
kept in a memory cache and a disk cache. Cells are reused.

## 9. Offline downloads

- Original file to `~/Movies/plex-rust/`, written as `.part`, resumed with an HTTP
  `Range` request, renamed when the byte count equals `Part.size`. A size
  mismatch is an error, not a retry loop.
- The item's metadata JSON and poster are stored next to it so Downloads
  works with no server.
- Offline playback hands mpv the local path. Position is stored locally and
  sent as one timeline call with `offline=1` when the server is next reached.

## 10. Packaging: one self-contained app

- A Mac GUI app has to be a `.app` bundle to carry its Info.plist and icon,
  so the deliverable is `plex-rust.app`, not a bare file. Inside it there is one
  executable and no bundled dylibs: libmpv and everything under it are linked
  statically. `otool -L` on the executable must list only `/usr/lib` and
  `/System/Library` paths. That is the acceptance check.
- Copy it to another Apple Silicon Mac and it runs. No Homebrew, no mpv.
- [U] mpv 0.42.0 builds as a static library on macOS with the minimal
  feature set. Fallback if it does not: build the same minimal set as dylibs,
  copy them into `Contents/Frameworks`, rewrite install names with
  `install_name_tool`, sign inside out. Still self-contained, just not one
  file inside the bundle.
- [U] An open upstream bug says mpv master fails to build with Vulkan
  disabled (#18817). Whether the 0.42.0 tag is affected is checked first.
  Fallback: apply the fix from upstream PR #18818 as a patch file in
  `scripts/patches/`.

## 11. Running the whole project unattended

Two actions need you, both before work starts. After them nothing does.

1. **`gh auth login`.** GitHub CLI is not logged in on this Mac [V], and your
   rule is that every change is pushed to a branch before merging.
2. **One Plex approval.** Kickoff runs the PIN login and prints one link. You
   approve it in the browser once. The token is stored outside the repo and
   is never printed or committed. No Plex server runs on this Mac [V], so the
   real server on your LAN or tailnet is the test target.

One thing may need a click later: the macOS Local Network prompt the first
time the bundled app reaches a LAN address. Runs started from a terminal are
exempt [V], so development and tests do not hit it.

How each step is checked without a person:

| Need | Mechanism |
| --- | --- |
| Plex API works | `plex` crate tests against recorded fixtures, plus a live run against the real server when signed in |
| Video plays right | `plex-rust play <url> --for 20s --report` prints JSON: `hwdec-current`, dropped frames, cache seconds, HDR state. Asserted by a script |
| UI looks right | `plex-rust snapshot <screen> <out.png>` puts the window where no display shows it and saves the window server's picture of it, glass included. An app may read back its own window, so no Screen Recording permission is needed. I read the PNGs and compare them with the design in section 8 |
| Power | `top -l 30 -o power -stats pid,command,cpu,power` works without sudo on this Mac [V]. Same file, Plex.app vs plex-rust. `sudo powermetrics` needs a password [V] and stays optional |
| Packaging | `otool -L`, then launch the bundled app and run the same play report |

Every [U] in this spec has a written fallback, so no check can stall the run
waiting for a decision. If a fallback also fails, work stops on that
milestone with the evidence and the rest continues where it does not depend
on it.

`play --report` and `snapshot` are the only test-only code paths in the app.

## 12. Milestones

Each milestone is a branch. Each bullet is roughly one commit.

**M0. Player core and proof.** Closes every [U] before UI work starts.
- `scripts/build-libmpv.sh` and the static link.
- `player` crate: mpv handle, options, typed properties, events.
- OpenGL render context in a bare AppKit window: `plex-rust <file-or-url>` plays.
- HDR layer setup.
- Checks, results written into the commit bodies:
  - [U] libmpv2 7.0.1 links against the static libmpv 0.42 (its headers are
    client API 2.5, mpv is 2.6). Fallback: our own bindings for the roughly
    20 C functions used, in `player/src/ffi.rs`.
  - [U] `CAOpenGLLayer` and CGL are reachable from the objc2 crates.
    Fallback: hand-written `extern_class!` bindings for that one class.
  - [U] `hwdec-current` reports `videotoolbox` for H.264, HEVC 10-bit, AV1.
    Fallback: none needed, mpv decodes in software; the report records it.
  - [U] `target-peak` source without private APIs. Fallback: leave
    `target-peak=auto`.
  - [U] The server answers `Range` on `Part.key` with 206 and plays without
    a decision call. Fallback: call
    `/video/:/transcode/universal/decision` with `directPlay=1` first.
  - [U] The server allows `download=1` for this account. Fallback: download
    through the plain `Part.key` URL.
  - Power: `top` power score for the same 4K HEVC file, 5 minutes each at 1x
    and 2x: Plex.app and plex-rust. The numbers go in the commit body. If plex-rust is
    not below Plex.app the result is reported and M1 still proceeds, since
    the Plex and UI work does not depend on the video path.

**M1. Plex core.** `plex` crate: models and client, PIN login, discovery and
connection choice, hubs / sections / items / search, timeline and scrobble.
Fixtures recorded from the real server.

**M2. Browse.** App shell and sidebar, login screen, response cache, image
cache, Home shelves, library grid, detail page, search.

**M3. Watch.** Player view and controls, resume and timeline reporting, track
menus and sidecar subtitles, markers and skip button, next episode, speed
controls, media keys, sleep assertion, Settings with the "Low power" switch.

**M4. Offline.** Download engine with resume, Downloads screen, offline
playback and progress sync.

**M5. Ship.** `scripts/bundle.sh` (Info.plist with `NSHighResolutionCapable`,
`LSMinimumSystemVersion`, `NSLocalNetworkUsageDescription`; ad hoc
`codesign -s -`), the `otool -L` check from section 10, app icon, README.

## 13. Verification

- Every commit: `cargo fmt --check`, `cargo clippy --all-targets -- -D
  warnings`, `cargo test`.
- Per milestone, the unattended checks in section 11.
- Final pass against the real server, scripted: log in, snapshot every
  screen, play a 4K HDR file with the report, seek, 2x and 3x speed, switch
  audio and subtitle tracks, read the resume point back from the server after
  quitting mid-episode, download an episode and play it from disk.
- Speed: time from launch to Home drawn, measured by the app's own log
  timestamps, cold and warm cache.
- Left for you, optional: watch something for real, and a
  `sudo powermetrics --samplers cpu_power,gpu_power,tasks --show-process-energy`
  run on battery for wattage. `powermetrics` calls its values estimates [V],
  so only a comparison on this one Mac means anything.
- Instruments is not available without full Xcode [V].
