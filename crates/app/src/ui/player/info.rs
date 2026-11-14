//! The panel that says how the file is being played: what it is made of,
//! how it is decoded, and how playback is keeping up.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSTextAlignment, NSTextField, NSView};
use objc2_foundation::NSString;
use player::Player;

use super::controls::{label, medium, regular};
use super::layout::{add, pin, row};
use super::surface::{Material, Surface};
use super::theme;

pub struct Info {
    surface: Surface,
    /// What each line is about, one under the other.
    names: Retained<NSTextField>,
    values: Retained<NSTextField>,
}

impl Info {
    /// The panel, hidden until it is asked for.
    pub fn new(main_thread: MainThreadMarker, material: Material) -> Self {
        let this = Self {
            surface: Surface::new(main_thread, material, theme::CARD_RADIUS),
            names: label(main_thread, theme::FONT_SIZE, regular(), &theme::secondary_text()),
            values: label(main_thread, theme::FONT_SIZE, medium(), &theme::text()),
        };
        for column in [&this.names, &this.values] {
            column.setMaximumNumberOfLines(0);
        }
        this.names.setAlignment(NSTextAlignment::Right);
        this.view().setAccessibilityLabel(Some(&NSString::from_str("Playback information")));
        this.view().setHidden(true);

        let content = this.surface.content();
        let columns: [&NSView; 2] = [&this.names, &this.values];
        let columns = row(&columns, theme::INFO_COLUMN_SPACING, main_thread);
        let padding = theme::CARD_PADDING;
        add(content, &columns);
        pin(&[
            columns.topAnchor().constraintEqualToAnchor_constant(&content.topAnchor(), padding),
            columns
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&content.bottomAnchor(), -padding),
            columns
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&content.leadingAnchor(), padding),
            columns
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&content.trailingAnchor(), -padding),
        ]);
        this
    }

    pub fn view(&self) -> &NSView {
        self.surface.view()
    }

    pub fn is_shown(&self) -> bool {
        !self.view().isHidden()
    }

    pub fn set_shown(&self, shown: bool) {
        self.view().setHidden(!shown);
    }

    /// Brings the panel up to date with `player`.
    pub fn refresh(&self, player: &Player) -> player::Result<()> {
        let lines = lines(player)?;
        let names: Vec<&str> = lines.iter().map(|(name, _)| *name).collect();
        let values: Vec<&str> = lines.iter().map(|(_, value)| value.as_str()).collect();
        self.names.setStringValue(&NSString::from_str(&names.join("\n")));
        self.values.setStringValue(&NSString::from_str(&values.join("\n")));
        Ok(())
    }
}

/// What the panel says, as a name and a value for each line. A line the
/// player has nothing to say about is left out.
fn lines(player: &Player) -> player::Result<Vec<(&'static str, String)>> {
    let text = |name: &str| player.diagnostic(name);
    let number = |name: &str| -> player::Result<Option<f64>> {
        Ok(text(name)?.and_then(|value| value.parse().ok()))
    };
    let mut lines = Vec::new();

    if let (Some(width), Some(height)) = (number("video-params/w")?, number("video-params/h")?) {
        let codec = text("video-format")?.unwrap_or_default().to_uppercase();
        let rate = number("container-fps")?.map(|rate| format!(" \u{b7} {rate:.3} fps"));
        let rate = rate.unwrap_or_default();
        lines.push(("Picture", format!("{width:.0} \u{d7} {height:.0} \u{b7} {codec}{rate}")));
    }
    if let Some(transfer) = text("video-params/gamma")? {
        let range = if matches!(transfer.as_str(), "pq" | "hlg") { "HDR" } else { "SDR" };
        let primaries = text("video-params/primaries")?.unwrap_or_default();
        lines.push(("Colour", format!("{range} \u{b7} {transfer} \u{b7} {primaries}")));
    }
    let decoder = match player.hardware_decoder()? {
        Some(name) => format!("Hardware \u{b7} {name}"),
        None => "Software".to_owned(),
    };
    lines.push(("Decoding", decoder));
    if let Some(codec) = text("audio-codec-name")? {
        let channels = text("audio-params/hr-channels")?.unwrap_or_default();
        let rate = number("audio-params/samplerate")?.unwrap_or_default() / 1000.0;
        lines.push((
            "Sound",
            format!("{} \u{b7} {channels} \u{b7} {rate} kHz", codec.to_uppercase()),
        ));
    }
    // mpv works the picture's rate out a little after the sound's.
    // Until it has, the sum would be the sound alone under a name that
    // says more.
    if let Some(picture) = number("video-bitrate")?.filter(|bits| *bits > 0.0) {
        let bits = picture + number("audio-bitrate")?.unwrap_or_default();
        lines.push(("Bit rate", format!("{:.1} Mbit/s", bits / 1e6)));
    }
    lines.push(("Dropped frames", player.dropped_frames()?.to_string()));
    if let Some(ahead) = player.buffered()? {
        lines.push(("Buffered", format!("{} s ahead", ahead.as_secs())));
    }
    Ok(lines)
}
