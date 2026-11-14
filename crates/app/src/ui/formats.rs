//! What a file is, in the words printed on a disc case: "4K", "Dolby
//! Vision", "DTS-HD MA 5.1".

use plex::model::{Media, Stream, StreamKind};

use super::text;

/// The badges for one version of an item: its resolution, its dynamic
/// range and its sound.
pub fn badges(media: &Media) -> Vec<String> {
    let mut badges: Vec<String> = Vec::new();
    badges.extend(media.video_resolution.as_deref().map(resolution));
    let part = media.parts.first();
    if let Some(video) = part.and_then(|part| part.embedded(StreamKind::Video).next()) {
        badges.extend(ranges(video).into_iter().map(str::to_owned));
    }
    // The track that will play says more than the file as a whole, which
    // only knows its first one.
    let heard = part.and_then(|part| {
        let chosen = part.embedded(StreamKind::Audio).find(|track| track.selected);
        chosen.or_else(|| part.embedded(StreamKind::Audio).next())
    });
    let (codec, profile, channels) = match heard {
        Some(track) => (track.codec.as_deref(), track.profile.as_deref(), track.channels),
        None => {
            (media.audio_codec.as_deref(), media.audio_profile.as_deref(), media.audio_channels)
        }
    };
    badges.extend(sound(codec, profile, channels));
    badges
}

/// What a badge has no word for: how the picture is encoded and how big
/// the file is, as in "HEVC · 6.8 GB".
pub fn file_facts(media: &Media) -> String {
    let mut facts: Vec<String> = Vec::new();
    facts.extend(media.video_codec.as_deref().map(|codec| match codec {
        "h264" => "H.264".to_owned(),
        "mpeg2video" => "MPEG-2".to_owned(),
        "mpeg4" | "msmpeg4v3" => "MPEG-4".to_owned(),
        "vc1" => "VC-1".to_owned(),
        other => other.to_uppercase(),
    }));
    // A film split across files is as big as all of them.
    let sizes: Vec<u64> = media.parts.iter().filter_map(|part| part.size).collect();
    if !sizes.is_empty() {
        facts.push(text::size(sizes.iter().sum()));
    }
    facts.join(" · ")
}

fn resolution(lines: &str) -> String {
    match lines {
        "4k" => "4K".to_owned(),
        "sd" => "SD".to_owned(),
        lines => format!("{lines}p"),
    }
}

/// The dynamic ranges a picture can be shown in. A Dolby Vision picture
/// of any profile but 5 also holds a plain HDR10 or HLG one.
fn ranges(video: &Stream) -> Vec<&'static str> {
    let mut ranges: Vec<&'static str> = Vec::new();
    if video.dolby_vision {
        ranges.push("Dolby Vision");
    }
    let has_base_layer = video.dolby_vision_profile != Some(5);
    match video.color_trc.as_deref() {
        Some("smpte2084") if has_base_layer => ranges.push("HDR10"),
        Some("arib-std-b67") if has_base_layer => ranges.push("HLG"),
        _ => {}
    }
    ranges
}

/// The sound of a track by the name on the box, then its speakers:
/// "Dolby Digital Plus 5.1". Sound placed as objects goes by that alone,
/// since it has no fixed number of speakers: "Dolby Atmos", "DTS:X".
fn sound(codec: Option<&str>, profile: Option<&str>, channels: Option<u32>) -> Option<String> {
    let profile = profile.unwrap_or_default();
    let format = codec.map(|codec| match (codec, profile) {
        ("eac3" | "truehd", profile) if profile.contains("atmos") => "Dolby Atmos".to_owned(),
        ("ac3", _) => "Dolby Digital".to_owned(),
        ("eac3", _) => "Dolby Digital Plus".to_owned(),
        ("truehd", _) => "Dolby TrueHD".to_owned(),
        ("dca" | "dca-ma", profile) if profile.contains("dts:x") => "DTS:X".to_owned(),
        ("dca-ma", _) | ("dca", "ma") => "DTS-HD MA".to_owned(),
        ("dca", "hra") => "DTS-HD HRA".to_owned(),
        ("dca", "es") => "DTS-ES".to_owned(),
        ("dca", _) => "DTS".to_owned(),
        ("aac", "he-aac") => "HE-AAC".to_owned(),
        ("opus", _) => "Opus".to_owned(),
        ("vorbis", _) => "Vorbis".to_owned(),
        (codec, _) if codec.starts_with("pcm") => "PCM".to_owned(),
        (other, _) => other.to_uppercase(),
    });
    let has_objects = matches!(format.as_deref(), Some("Dolby Atmos" | "DTS:X"));
    let layout = channels.filter(|_| !has_objects).map(|channels| match channels {
        1 => "Mono".to_owned(),
        2 => "Stereo".to_owned(),
        6 => "5.1".to_owned(),
        7 => "6.1".to_owned(),
        8 => "7.1".to_owned(),
        other => format!("{other} ch"),
    });
    let words: Vec<String> = format.into_iter().chain(layout).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media(json: serde_json::Value) -> Media {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn the_file_is_named_by_picture_sound_encoding_and_size() {
        let film = media(serde_json::json!({
            "id": 1, "videoResolution": "4k", "audioChannels": 6,
            "videoCodec": "hevc", "audioCodec": "eac3",
            "Part": [{
                "id": 1, "key": "/library/parts/1/0/file.mkv", "size": 6_828_818_931_u64,
                "Stream": [{ "id": 1, "streamType": 1, "colorTrc": "smpte2084" }],
            }],
        }));
        assert_eq!(badges(&film), ["4K", "HDR10", "Dolby Digital Plus 5.1"]);
        assert_eq!(file_facts(&film), "HEVC · 6.8 GB");
    }

    #[test]
    fn the_sound_is_that_of_the_track_chosen() {
        let film = media(serde_json::json!({
            "id": 1, "videoResolution": "1080", "audioCodec": "ac3", "audioChannels": 6,
            "Part": [{ "id": 1, "key": "/library/parts/1/0/file.mkv", "Stream": [
                { "id": 1, "streamType": 2, "codec": "ac3", "channels": 6 },
                { "id": 2, "streamType": 2, "codec": "dca", "profile": "ma", "channels": 8,
                  "selected": true },
            ]}],
        }));
        assert_eq!(badges(&film), ["1080p", "DTS-HD MA 7.1"]);
    }

    #[test]
    fn dolby_vision_names_the_plain_picture_it_falls_back_to() {
        let video = |json: serde_json::Value| -> Stream { serde_json::from_value(json).unwrap() };
        let with_fallback = video(serde_json::json!({
            "id": 1, "streamType": 1, "colorTrc": "smpte2084",
            "DOVIPresent": true, "DOVIProfile": 8,
        }));
        assert_eq!(ranges(&with_fallback), ["Dolby Vision", "HDR10"]);
        let alone = video(serde_json::json!({
            "id": 1, "streamType": 1, "colorTrc": "smpte2084",
            "DOVIPresent": true, "DOVIProfile": 5,
        }));
        assert_eq!(ranges(&alone), ["Dolby Vision"]);
        let hlg = video(serde_json::json!({
            "id": 1, "streamType": 1, "colorTrc": "arib-std-b67",
        }));
        assert_eq!(ranges(&hlg), ["HLG"]);
        let standard = video(serde_json::json!({ "id": 1, "streamType": 1, "colorTrc": "bt709" }));
        assert!(ranges(&standard).is_empty());
    }

    #[test]
    fn sound_goes_by_the_name_on_the_box() {
        let named = |codec, profile, channels| sound(Some(codec), profile, Some(channels));
        assert_eq!(named("dca-ma", Some("ma"), 6).as_deref(), Some("DTS-HD MA 5.1"));
        assert_eq!(named("dca", Some("dts"), 6).as_deref(), Some("DTS 5.1"));
        assert_eq!(named("truehd", None, 8).as_deref(), Some("Dolby TrueHD 7.1"));
        let atmos = Some("dolby digital plus + dolby atmos");
        assert_eq!(named("eac3", atmos, 6).as_deref(), Some("Dolby Atmos"));
        assert_eq!(named("aac", Some("lc"), 2).as_deref(), Some("AAC Stereo"));
        assert_eq!(named("flac", None, 1).as_deref(), Some("FLAC Mono"));
        assert_eq!(sound(None, None, None), None);
    }
}
