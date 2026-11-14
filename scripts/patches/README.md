# Patches

`scripts/build-libmpv.sh` downloads the upstream sources of mpv 0.42.0 and
FFmpeg 9.0.2, checks each archive against a SHA-256, and applies the patches
in this folder before building. The repository itself contains no mpv or
FFmpeg source, only these differences from it.

Each patch is distributed under the license of the files it modifies, as
the header of each file states it. The built app links mpv with
`-Dgpl=true` and FFmpeg with `--enable-gpl`, so the binary as a whole is
under the GPL, version 2 or later (mpv's `Copyright` file, FFmpeg's
`LICENSE.md`).

## mpv-build-without-vulkan.patch

- **Changes:** `video/out/mac/view.swift` casts to `CAMetalLayer` where it
  cast to mpv's own `MetalLayer`, a class that is not compiled when Vulkan
  is disabled. Without it mpv 0.42.0 does not build with `-Dvulkan=disabled`.
- **Applies to:** mpv 0.42.0.
- **Origin:** upstream. It is the change of mpv pull request
  [#18818](https://github.com/mpv-player/mpv/pull/18818), "mac/view: fix a
  libmpv build failure under macOS with Vulkan disabled", by low-batt, which
  fixes issue [#18817](https://github.com/mpv-player/mpv/issues/18817). The
  diff here is the pull request's, unchanged. It is not in the 0.42.0
  release.
- **License:** LGPL, version 2.1 or later, as the header of `view.swift`
  states.

## mpv-restart-video-before-audio.patch

- **Changes:** after a seek the picture restarts at once and the sound
  joins it in sync, where mpv held the picture for the audio output's
  latency. Touches `audio/out/ao.h`, `audio/out/buffer.c`, `player/audio.c`
  and `player/core.h`. The patch's own header explains the mechanism and
  what is left unchanged.
- **Applies to:** mpv 0.42.0.
- **Origin:** written for this project. Not from upstream.
- **License:** LGPL, version 2.1 or later, as the headers of all four
  files state.

## mpv-decode-on-for-short-seeks-forward.patch

- **Changes:** an exact seek a little way forward goes on decoding from
  the current frame when no keyframe lies between it and the target, where
  mpv restarted the decoder at the keyframe before the target. Touches
  `demux/demux.c`, `demux/demux.h`, `filters/filter.c`, `filters/filter.h`
  and `player/playloop.c`. The patch's own header lists the seeks it leaves
  alone.
- **Applies to:** mpv 0.42.0.
- **Origin:** written for this project. Not from upstream.
- **License:** LGPL, version 2.1 or later. The headers of `demux/demux.c`,
  `demux/demux.h` and `player/playloop.c` state it. `filters/filter.c` and
  `filters/filter.h` carry no license header, and for those mpv's
  `Copyright` file applies: "C source files without Copyright notice are
  usually licensed as LGPLv2.1+".

## ffmpeg-decode-videotoolbox-asynchronously.patch

- **Changes:** the VideoToolbox hardware decoder is handed the next frame
  without a wait for the one before, and a frame is waited for only when
  it is returned to its user. Touches `libavcodec/videotoolbox.c` and
  `libavcodec/vt_internal.h`. The patch's own header describes each part
  and the one change in how a failed frame is reported.
- **Applies to:** FFmpeg 9.0.2.
- **Origin:** written for this project. Not from upstream.
- **License:** LGPL, version 2.1 or later, as the headers of both files
  state.

## Changing a patch

The build applies a patch unless it is already in the unpacked tree, and
rebuilds that library when it applied one. A patch that changes after it
was applied is not noticed: delete `vendor/src/mpv` or `vendor/src/ffmpeg`
and the matching `vendor/libmpv/.built-*` file, then run the script again.
