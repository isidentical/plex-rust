#!/bin/sh
# Builds a static libmpv and everything under it into vendor/libmpv.
#
# plex-rust links this statically so the app depends only on macOS system
# libraries. Homebrew's mpv is not usable for that: it pulls in vapoursynth,
# yt-dlp, luajit and a Vulkan stack, none of which the player needs.
#
# Each library is built once; delete vendor/libmpv to start over.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
src=$root/vendor/src
prefix=$root/vendor/libmpv
jobs=$(sysctl -n hw.ncpu)

export MACOSX_DEPLOYMENT_TARGET=26.0
# Only our own prefix is searched, so nothing from Homebrew can leak in.
export PKG_CONFIG_LIBDIR=$prefix/lib/pkgconfig
export PATH=$prefix/bin:$PATH

mkdir -p "$src" "$prefix"

# fetch <name> <url> <sha256>: download, verify, unpack into $src/<name>.
fetch() {
    name=$1 url=$2 sha=$3
    [ -d "$src/$name" ] && return
    archive=$src/$(basename "$url")
    curl --fail --location --silent --show-error --output "$archive" "$url"
    echo "$sha  $archive" | shasum -a 256 --check --quiet
    mkdir "$src/$name"
    tar -xf "$archive" -C "$src/$name" --strip-components 1
}

# built <name>: true once <name> has been installed into the prefix.
built() { [ -e "$prefix/.built-$1" ]; }
done_building() { touch "$prefix/.built-$1"; }

# apply <name> <patch>: applies scripts/patches/<patch> to $src/<name> and
# has <name> built again, unless the patch is already in. A patch that is
# added later so reaches a tree that was unpacked and built before it.
apply() {
    patch=$root/scripts/patches/$2
    # It is in if it would come out cleanly. --force keeps patch from
    # guessing that a patch which does not come out was meant forwards.
    if patch -d "$src/$1" -p1 --reverse --force --dry-run < "$patch" > /dev/null 2>&1; then
        return
    fi
    patch -d "$src/$1" -p1 --forward < "$patch"
    rm -f "$prefix/.built-$1"
}

meson_build() {
    name=$1
    shift
    # A failed run leaves a configured directory that would ignore new options.
    rm -rf "$src/$name/build"
    meson setup "$src/$name/build" "$src/$name" \
        --prefix "$prefix" --libdir lib --buildtype release \
        --default-library static "$@"
    meson install -C "$src/$name/build"
}

if ! built dav1d; then
    fetch dav1d \
        https://code.videolan.org/videolan/dav1d/-/archive/1.5.4/dav1d-1.5.4.tar.bz2 \
        2abfb0c89212e6e4733a54e0ae509ec00a5b845a6360946f918806e14aedb011
    meson_build dav1d -Denable_tools=false -Denable_tests=false
    done_building dav1d
fi

if ! built freetype; then
    fetch freetype \
        https://downloads.sourceforge.net/project/freetype/freetype2/2.15.0/freetype-2.15.0.tar.xz \
        93cd47eb719c325107f2d3cf087d06299e944c3b48a40b55537302304dea1dd5
    meson_build freetype -Dharfbuzz=disabled -Dpng=disabled -Dbzip2=disabled \
        -Dbrotli=disabled -Dzlib=system
    done_building freetype
fi

if ! built fribidi; then
    fetch fribidi \
        https://github.com/fribidi/fribidi/releases/download/v1.0.18/fribidi-1.0.18.tar.xz \
        1de5e8073d035388b71d0b2d452b1da1f8caeea8a7702997b401fe1b6250727f
    meson_build fribidi -Ddocs=false -Dbin=false -Dtests=false
    done_building fribidi
fi

if ! built harfbuzz; then
    fetch harfbuzz \
        https://github.com/harfbuzz/harfbuzz/releases/download/15.1.0/harfbuzz-15.1.0.tar.xz \
        0fc7c565443a5f4be7da65ac5ee5bd737d9b158dc908708b7689a0a5796b0c8d
    meson_build harfbuzz -Dfreetype=enabled -Dcoretext=enabled \
        -Dglib=disabled -Dgobject=disabled -Dcairo=disabled -Dicu=disabled \
        -Dtests=disabled -Ddocs=disabled -Dutilities=disabled
    done_building harfbuzz
fi

if ! built libass; then
    fetch libass \
        https://github.com/libass/libass/releases/download/0.17.5/libass-0.17.5.tar.xz \
        2dca25c0e0c837ddf00b52011b3f82cac1e4ddd3ad018227806b0c2288864acc
    (
        cd "$src/libass"
        # CoreText finds the fonts on macOS, so fontconfig is not needed.
        ./configure --prefix="$prefix" --enable-static --disable-shared \
            --disable-fontconfig
        make -j"$jobs" install
    )
    done_building libass
fi

if ! built ffmpeg; then
    fetch ffmpeg \
        https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz \
        8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
    (
        cd "$src/ffmpeg"
        # Playback only: no encoders, muxers, devices or command line tools.
        # mpv does its own HTTP through libcurl, so no TLS library either.
        ./configure --prefix="$prefix" --enable-static --disable-shared \
            --enable-gpl --disable-programs --disable-doc --disable-debug \
            --disable-encoders --disable-muxers --disable-devices \
            --disable-autodetect --enable-zlib \
            --enable-videotoolbox --enable-audiotoolbox --enable-libdav1d
        make -j"$jobs" install
    )
    done_building ffmpeg
fi

if ! built libplacebo; then
    # Cloned, not downloaded: the release archive lacks the git submodules
    # (glad, jinja) that the build needs.
    [ -d "$src/libplacebo" ] || git clone --quiet --depth 1 --branch v7.387.0 \
        --recurse-submodules --shallow-submodules \
        https://code.videolan.org/videolan/libplacebo.git "$src/libplacebo"
    meson_build libplacebo -Dopengl=enabled -Dvulkan=disabled \
        -Dshaderc=disabled -Dglslang=disabled -Dlcms=disabled \
        -Dxxhash=disabled -Ddemos=false -Dtests=false
    done_building libplacebo
fi

fetch mpv \
    https://github.com/mpv-player/mpv/archive/refs/tags/v0.42.0.tar.gz \
    bcfebbfa2b65a6d88dc97ea18117831072373e38b5785a2afb874ed4df5070dd
# 0.42.0 does not compile with Vulkan disabled. This is the upstream fix,
# https://github.com/mpv-player/mpv/pull/18818, not yet released.
apply mpv mpv-build-without-vulkan.patch
# Ours: the picture does not wait for the sound after a seek.
apply mpv mpv-restart-video-before-audio.patch

if ! built mpv; then
    # macOS ships libcurl but no pkg-config file for it, and mpv's HTTP
    # backend is only found through pkg-config.
    cat > "$prefix/lib/pkgconfig/libcurl.pc" <<EOF
Name: libcurl
Description: The libcurl that ships with macOS
Version: $(curl-config --version | cut -d' ' -f2)
Libs: -lcurl
EOF
    meson_build mpv -Dlibmpv=true -Dcplayer=false -Dgpl=true \
        -Dlibcurl=enabled -Dvideotoolbox-gl=enabled \
        -Dlua=disabled -Djavascript=disabled -Dvapoursynth=disabled \
        -Drubberband=disabled -Dvulkan=disabled -Dlibarchive=disabled \
        -Dlibbluray=disabled -Dlcms2=disabled -Dzimg=disabled \
        -Duchardet=disabled -Djpeg=disabled -Dcdda=disabled \
        -Ddvdnav=disabled -Dmanpage-build=disabled
    # mpv compiles its Swift sources into a static archive that it names
    # swift.o, and ar cannot nest that inside libmpv.a: the installed archive
    # comes out empty. libtool merges archives and objects correctly.
    libtool -static -o "$prefix/lib/libmpv.a" \
        "$src/mpv/build/osdep/mac/swift.o" "$src/mpv/build"/libmpv.a.p/*.o
    done_building mpv
fi

echo "static libmpv installed in $prefix"
