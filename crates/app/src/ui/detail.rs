//! The page for one film, show or episode: what it is, the buttons to
//! watch it, and for a show, its episodes season by season.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSButton, NSCellImagePosition, NSColor, NSControlSize, NSFontWeightBold, NSFontWeightMedium,
    NSFontWeightRegular, NSFontWeightSemibold, NSLayoutAttribute, NSLayoutConstraint,
    NSPopUpButton, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_core_foundation::CGSize;
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol, NSRect, NSString, ns_string};
use plex::library::next_up;
use plex::model::{Item, Kind, Media, RatingKey, StreamKind};

use super::artwork::{self, ArtView};
use super::backdrop::Backdrop;
use super::hover::HoverView;
use super::layout;
use super::posters::{Action, Header, OnAction, Outline, Posters, Shelf, Style};
use super::text;
use super::theme;
use crate::images::Images;

/// How many genres and cast members a page names.
const GENRES_SHOWN: usize = 3;
const CAST_SHOWN: usize = 4;

pub struct Ivars {
    backdrop: Retained<Backdrop>,
    /// The header and, under it, the episodes of the chosen season.
    list: Retained<Posters>,
    hero: Hero,
    images: Rc<Images>,
    /// Shared with the links in the header, which act on it.
    item: Rc<RefCell<Option<Item>>>,
    /// Every episode of a show, in order.
    episodes: RefCell<Vec<Item>>,
    /// The season whose episodes are listed.
    season: RefCell<Option<RatingKey>>,
    on_action: Rc<OnAction>,
}

/// The views at the top of the page.
struct Hero {
    view: Retained<NSView>,
    poster: Rc<ArtView>,
    /// Around the poster. On an episode's page it leads to the show.
    poster_link: Retained<HoverView>,
    /// The line above an episode's title: its show and its season, both
    /// links, and its number.
    eyebrow: Retained<NSStackView>,
    show_name: Retained<NSTextField>,
    season_name: Retained<NSTextField>,
    episode_number: Retained<NSTextField>,
    title: Retained<NSTextField>,
    facts: Retained<NSTextField>,
    badges: Retained<NSStackView>,
    genres: Retained<NSTextField>,
    play: Retained<NSButton>,
    watched: Retained<NSButton>,
    remaining: Retained<NSTextField>,
    summary: Retained<NSTextField>,
    credits: Retained<NSTextField>,
    seasons: Retained<NSPopUpButton>,
    season_size: Retained<NSTextField>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Detail` does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "PlexRustDetail"]
    #[ivars = Ivars]
    pub struct Detail;

    unsafe impl NSObjectProtocol for Detail {}

    impl Detail {
        #[unsafe(method(play:))]
        fn play(&self, _sender: Option<&AnyObject>) {
            self.press_play();
        }

        #[unsafe(method(toggleWatched:))]
        fn toggle_watched(&self, _sender: Option<&AnyObject>) {
            self.press_watched();
        }

        #[unsafe(method(seasonChosen:))]
        fn season_chosen(&self, _sender: Option<&AnyObject>) {
            let chosen = usize::try_from(self.ivars().hero.seasons.indexOfSelectedItem()).ok();
            let seasons = seasons(&self.ivars().episodes.borrow());
            if let Some((key, _)) = chosen.and_then(|chosen| seasons.into_iter().nth(chosen)) {
                *self.ivars().season.borrow_mut() = Some(key);
                self.list_season();
            }
        }
    }
);

impl Detail {
    /// An empty page. `season` is the season a show's page opens on, when
    /// the viewer came by way of one. `on_action` is called when they ask
    /// to open, play or mark the item or one of its episodes.
    pub fn new(
        main_thread: MainThreadMarker,
        images: &Rc<Images>,
        season: Option<RatingKey>,
        on_action: impl Fn(&Item, Action) + 'static,
    ) -> Retained<Self> {
        let on_action: Rc<OnAction> = Rc::new(on_action);
        let item: Rc<RefCell<Option<Item>>> = Rc::new(RefCell::new(None));
        let (linked, follow) = (Rc::clone(&item), Rc::clone(&on_action));
        let hero = Hero::new(main_thread, move || {
            // Cloned so the handler is free to replace the page.
            let episode = linked.borrow().clone();
            if let Some(episode) = episode {
                follow(&episode, Action::OpenShow);
            }
        });
        let header = Header { view: hero.view.clone(), height: theme::HERO_HEIGHT };
        let from_list = Rc::clone(&on_action);
        let list = Posters::new(
            main_thread,
            Rc::clone(images),
            Some(header),
            // The header belongs to the first shelf's place, so there is
            // always a shelf, if only an empty one.
            vec![Outline { title: "", style: Style::List, count: 0 }],
            "",
            move |item, action| from_list(item, action),
            || {},
        );
        let backdrop = Backdrop::new(main_thread);
        layout::fill(&backdrop, list.view());

        let this = Self::alloc(main_thread).set_ivars(Ivars {
            backdrop,
            list,
            hero,
            images: Rc::clone(images),
            item,
            episodes: RefCell::new(Vec::new()),
            season: RefCell::new(season),
            on_action,
        });
        // SAFETY: `init` is NSObject's designated initializer.
        let detail: Retained<Self> = unsafe { msg_send![super(this), init] };
        let hero = &detail.ivars().hero;
        // SAFETY: the controls do not retain their target. `Detail` owns
        // the page they are in, so it outlives every click.
        unsafe {
            for (button, action) in
                [(&hero.play, sel!(play:)), (&hero.watched, sel!(toggleWatched:))]
            {
                button.setTarget(Some(&detail));
                button.setAction(Some(action));
            }
            hero.seasons.setTarget(Some(&detail));
            hero.seasons.setAction(Some(sel!(seasonChosen:)));
        }
        detail
    }

    pub fn view(&self) -> &NSView {
        &self.ivars().backdrop
    }

    pub fn focus(&self) {
        self.ivars().list.focus();
    }

    /// Scrolls the page so that the content `y` points down is at the top.
    pub fn scroll_to(&self, y: f64) {
        self.ivars().list.scroll_to(y);
    }

    /// Plays the item, or for a show, the episode to watch next.
    pub fn press_play(&self) {
        if let Some(item) = self.playable() {
            self.act(&item, Action::Play);
        }
    }

    /// Does what a click on an episode's poster, show name or season name
    /// does: goes to the show, opened on the episode's season.
    pub fn follow_show_link(&self) {
        let item = self.ivars().item.borrow().clone();
        if let Some(item) = item {
            self.act(&item, Action::OpenShow);
        }
    }

    /// Marks the item as watched if it is not, and the other way round.
    pub fn press_watched(&self) {
        let item = self.ivars().item.borrow().clone();
        if let Some(item) = item {
            self.act(&item, Action::SetWatched(!item.is_watched()));
        }
    }

    /// Fills the page with `item`. Called again when the server has more
    /// to say about it, such as how far it was watched.
    pub fn show(&self, item: &Item) {
        let ivars = self.ivars();
        ivars.hero.show(item, &ivars.images);
        if let Some(colors) = item.colors {
            ivars.backdrop.set_colors(colors);
        }
        let art = match item.kind {
            Kind::Episode => item.grandparent_art.as_deref().or(item.art.as_deref()),
            _ => item.art.as_deref(),
        };
        if let Some(path) = art {
            ivars.backdrop.show_art(path, &ivars.images);
        }
        *ivars.item.borrow_mut() = Some(item.clone());
        self.show_play_button();
    }

    /// Fills in every episode of the show on the page, in order.
    pub fn show_episodes(&self, episodes: Vec<Item>) {
        let ivars = self.ivars();
        let seasons = seasons(&episodes);
        let titles: Vec<Retained<NSString>> =
            seasons.iter().map(|(_, title)| NSString::from_str(title)).collect();
        // Without a season to open on, open on the one to watch next.
        let wanted = ivars.season.borrow().clone();
        let wanted = wanted.or_else(|| next_up(&episodes)?.parent_rating_key.clone());
        let chosen = seasons.iter().position(|(key, _)| Some(key) == wanted.as_ref()).unwrap_or(0);
        *ivars.season.borrow_mut() = seasons.get(chosen).map(|(key, _)| key.clone());
        *ivars.episodes.borrow_mut() = episodes;

        let picker = &ivars.hero.seasons;
        picker.removeAllItems();
        picker.addItemsWithTitles(&NSArray::from_retained_slice(&titles));
        picker.selectItemAtIndex(chosen.cast_signed());
        picker.setHidden(seasons.is_empty());
        self.show_play_button();
        self.list_season();
    }

    /// Lists the episodes of the chosen season under the header.
    fn list_season(&self) {
        let ivars = self.ivars();
        let season = ivars.season.borrow().clone();
        let episodes: Vec<Item> = ivars
            .episodes
            .borrow()
            .iter()
            .filter(|episode| episode.parent_rating_key == season)
            .cloned()
            .collect();
        let size = u32::try_from(episodes.len()).unwrap_or(u32::MAX);
        text::set(&ivars.hero.season_size, &text::count(size, "episode"));
        ivars.hero.season_size.setHidden(episodes.is_empty());
        ivars.list.show(vec![Shelf { title: String::new(), style: Style::List, items: episodes }]);
    }

    /// What the play button plays: the item itself, or for a show, the
    /// episode to watch next.
    fn playable(&self) -> Option<Item> {
        let item = self.ivars().item.borrow().clone()?;
        match item.kind {
            Kind::Show => next_up(&self.ivars().episodes.borrow()).cloned(),
            Kind::Movie | Kind::Episode => Some(item),
            Kind::Season | Kind::Other => None,
        }
    }

    /// "Play", "Resume", or for a show "Play S2 · E15".
    fn show_play_button(&self) {
        let hero = &self.ivars().hero;
        let playable = self.playable();
        hero.play.setHidden(playable.is_none());
        let Some(playable) = playable else {
            return;
        };
        let verb = if playable.view_offset.is_some() { "Resume" } else { "Play" };
        let is_show =
            self.ivars().item.borrow().as_ref().is_some_and(|item| item.kind == Kind::Show);
        let title = match text::episode_code(&playable).filter(|_| is_show) {
            Some(code) => format!("{verb} {code}"),
            None => verb.to_owned(),
        };
        hero.play.setTitle(&NSString::from_str(&title));
        let remaining = text::remaining(&playable);
        text::set(&hero.remaining, remaining.as_deref().unwrap_or_default());
        hero.remaining.setHidden(remaining.is_none());
    }

    fn act(&self, item: &Item, action: Action) {
        // What the handler shows next may replace this page, which would
        // free it while this method is still running.
        let _alive = self.retain();
        (self.ivars().on_action)(item, action);
    }
}

/// The seasons that `episodes` belong to, in order, as key and title.
fn seasons(episodes: &[Item]) -> Vec<(RatingKey, String)> {
    let mut seasons: Vec<(RatingKey, String)> = Vec::new();
    for episode in episodes {
        let Some(key) = &episode.parent_rating_key else {
            continue;
        };
        if seasons.last().is_none_or(|(last, _)| last != key) {
            let title = episode.parent_title.clone().unwrap_or_else(|| "Season".to_owned());
            seasons.push((key.clone(), title));
        }
    }
    seasons
}

impl Hero {
    /// `open_show` is called when a link from an episode to its show is
    /// clicked.
    fn new(main_thread: MainThreadMarker, open_show: impl Fn() + 'static) -> Self {
        let open_show: Rc<dyn Fn()> = Rc::new(open_show);
        // SAFETY: the weights are constants that AppKit provides.
        let (bold, semibold, medium, regular) = unsafe {
            (NSFontWeightBold, NSFontWeightSemibold, NSFontWeightMedium, NSFontWeightRegular)
        };
        let (primary, secondary) = (NSColor::labelColor(), NSColor::secondaryLabelColor());
        let line = |size, weight, color| text::label(size, weight, color, main_thread);

        let poster = Rc::new(ArtView::new(main_thread, theme::RADIUS_L));
        let (lifted, follow) = (Rc::clone(&poster), Rc::clone(&open_show));
        let poster_link =
            HoverView::link(main_thread, move |over| lifted.set_hovered(over), move || follow());
        poster_link.setTranslatesAutoresizingMaskIntoConstraints(false);
        layout::cover(&poster_link, poster.view());
        // The shadow sits on the link, since the poster's own layer swaps
        // its shadow for the lift under the pointer.
        poster_link.setWantsLayer(true);
        if let Some(layer) = poster_link.layer() {
            layer.setShadowOpacity(theme::HERO_SHADOW_OPACITY);
            layer.setShadowRadius(theme::HERO_SHADOW_RADIUS);
            layer.setShadowOffset(CGSize { width: 0.0, height: -theme::HERO_SHADOW_DROP });
        }
        let (show_link, show_name) = text_link(Rc::clone(&open_show), main_thread);
        let (season_link, season_name) = text_link(open_show, main_thread);
        let episode_number = line(theme::FONT_HEADLINE, semibold, &secondary);
        let dot = || {
            let dot = line(theme::FONT_HEADLINE, semibold, &secondary);
            text::set(&dot, "·");
            dot
        };
        let (first_dot, second_dot) = (dot(), dot());
        let eyebrow: [&NSView; 5] =
            [&show_link, &first_dot, &season_link, &second_dot, &episode_number];
        let eyebrow = NSStackView::stackViewWithViews(&NSArray::from_slice(&eyebrow), main_thread);
        eyebrow.setSpacing(theme::SPACE_S);
        let title = text::paragraph(theme::FONT_LARGE_TITLE, &primary, 2, main_thread);
        title.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize_weight(
            theme::FONT_LARGE_TITLE,
            bold,
        )));
        let badges = NSStackView::new(main_thread);
        badges.setSpacing(theme::SPACE_S);

        let play = button("Play", Some("play.fill"), main_thread);
        play.setControlSize(NSControlSize::Large);
        // The default button: drawn in the accent colour, pressed by Return.
        play.setKeyEquivalent(ns_string!("\r"));
        play.setBezelColor(Some(&NSColor::controlAccentColor()));
        let watched = button("Mark as Watched", Some("checkmark"), main_thread);
        watched.setControlSize(NSControlSize::Large);
        let remaining = line(theme::FONT_BODY, regular, &secondary);
        let buttons: [&NSView; 3] = [&play, &watched, &remaining];
        let buttons = NSStackView::stackViewWithViews(&NSArray::from_slice(&buttons), main_thread);
        buttons.setSpacing(theme::SPACE_M);

        let hero = Self {
            view: NSView::new(main_thread),
            poster,
            poster_link,
            eyebrow,
            show_name,
            season_name,
            episode_number,
            title,
            facts: line(theme::FONT_HEADLINE, medium, &secondary),
            badges,
            genres: line(theme::FONT_BODY, regular, &secondary),
            play,
            watched,
            remaining,
            summary: text::paragraph(
                theme::FONT_HEADLINE,
                &primary,
                theme::HERO_SUMMARY_LINES,
                main_thread,
            ),
            credits: text::paragraph(theme::FONT_BODY, &secondary, 2, main_thread),
            seasons: NSPopUpButton::initWithFrame_pullsDown(
                NSPopUpButton::alloc(main_thread),
                NSRect::ZERO,
                false,
            ),
            season_size: line(theme::FONT_BODY, regular, &secondary),
        };
        hero.seasons.setControlSize(NSControlSize::Large);
        hero.seasons.setHidden(true);
        hero.season_size.setHidden(true);
        hero.place(&buttons, main_thread);
        hero
    }

    /// The poster on the left, the text in a column beside it, and the
    /// season picker along the bottom, where the episode list begins.
    fn place(&self, buttons: &NSStackView, main_thread: MainThreadMarker) {
        let column: [&NSView; 8] = [
            &self.eyebrow,
            &self.title,
            &self.facts,
            &self.badges,
            buttons,
            &self.summary,
            &self.genres,
            &self.credits,
        ];
        let column = NSStackView::stackViewWithViews(&NSArray::from_slice(&column), main_thread);
        column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        column.setAlignment(NSLayoutAttribute::Leading);
        column.setSpacing(theme::SPACE_M);
        column.setCustomSpacing_afterView(theme::SPACE_XS, &self.eyebrow);
        column.setCustomSpacing_afterView(theme::SPACE_S, &self.title);
        column.setCustomSpacing_afterView(theme::SPACE_L, &self.badges);
        column.setCustomSpacing_afterView(theme::SPACE_L, buttons);
        column.setTranslatesAutoresizingMaskIntoConstraints(false);

        let season: [&NSView; 2] = [&self.seasons, &self.season_size];
        let season = NSStackView::stackViewWithViews(&NSArray::from_slice(&season), main_thread);
        season.setSpacing(theme::SPACE_M);
        season.setTranslatesAutoresizingMaskIntoConstraints(false);

        let (view, poster) = (&self.view, &self.poster_link);
        view.addSubview(poster);
        view.addSubview(&column);
        view.addSubview(&season);
        let margin = theme::PAGE_MARGIN;
        let poster_height = (theme::HERO_POSTER_WIDTH * theme::POSTER_ASPECT).round();
        for text in [&self.title, &self.summary, &self.credits] {
            text.setPreferredMaxLayoutWidth(theme::HERO_TEXT_WIDTH);
        }
        NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
            poster.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            poster.topAnchor().constraintEqualToAnchor_constant(&view.topAnchor(), theme::SPACE_L),
            poster.widthAnchor().constraintEqualToConstant(theme::HERO_POSTER_WIDTH),
            poster.heightAnchor().constraintEqualToConstant(poster_height),
            column
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&poster.trailingAnchor(), theme::SPACE_XXL),
            column.topAnchor().constraintEqualToAnchor(&poster.topAnchor()),
            column
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&view.trailingAnchor(), -margin),
            column.widthAnchor().constraintLessThanOrEqualToConstant(theme::HERO_TEXT_WIDTH),
            season.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), margin),
            season
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&view.bottomAnchor(), -theme::SPACE_S),
        ]));
    }

    fn show(&self, item: &Item, images: &Rc<Images>) {
        let set = |label: &NSTextField, words: &str| {
            text::set(label, words);
            label.setHidden(words.is_empty());
        };
        let is_episode = item.kind == Kind::Episode;
        self.eyebrow.setHidden(!is_episode);
        self.poster_link.set_live(is_episode);
        set(&self.show_name, item.grandparent_title.as_deref().unwrap_or_default());
        set(&self.season_name, item.parent_title.as_deref().unwrap_or_default());
        let number = item.index.map(|number| format!("Episode {number}"));
        set(&self.episode_number, number.as_deref().unwrap_or_default());
        set(&self.title, &item.title);
        set(&self.facts, &facts(item));
        set(&self.summary, item.summary.as_deref().unwrap_or_default());
        set(&self.genres, &names(&item.genres, GENRES_SHOWN).join(", "));
        set(&self.credits, &credits(item));

        let badges: Vec<Retained<NSView>> = item
            .media
            .first()
            .map(badges)
            .unwrap_or_default()
            .iter()
            .map(|words| badge(words, self.view.mtm()))
            .collect();
        self.badges.setViews_inGravity(
            &NSArray::from_retained_slice(&badges),
            objc2_app_kit::NSStackViewGravity::Leading,
        );
        self.badges.setHidden(badges.is_empty());

        let mark = if item.is_watched() { "Mark as Unwatched" } else { "Mark as Watched" };
        self.watched.setTitle(&NSString::from_str(mark));

        // An episode's own artwork is a wide frame, so it borrows its show's.
        let poster = match item.kind {
            Kind::Episode => item.grandparent_thumb.as_deref().or(item.thumb.as_deref()),
            _ => item.thumb.as_deref(),
        };
        self.poster.show(poster, theme::HERO_POSTER_REQUEST, images);
        self.poster.set_progress(item.progress());
    }
}

/// A few words that lead somewhere: brighter under the pointer, which
/// turns into a hand over them.
fn text_link(
    on_click: Rc<dyn Fn()>,
    main_thread: MainThreadMarker,
) -> (Retained<HoverView>, Retained<NSTextField>) {
    // SAFETY: the weight is a constant that AppKit provides.
    let weight = unsafe { NSFontWeightSemibold };
    let label =
        text::label(theme::FONT_HEADLINE, weight, &NSColor::secondaryLabelColor(), main_thread);
    let lit = label.clone();
    let link = HoverView::link(
        main_thread,
        move |over| {
            let color = if over { NSColor::labelColor() } else { NSColor::secondaryLabelColor() };
            lit.setTextColor(Some(&color));
        },
        move || on_click(),
    );
    layout::cover(&link, &label);
    (link, label)
}

/// The line under the title, such as "2015 · 2 h 18 min · PG-13 · ★ 8.4".
fn facts(item: &Item) -> String {
    let mut facts: Vec<String> = Vec::new();
    facts.extend(item.year.map(|year| year.to_string()));
    match item.kind {
        // The server gives a show the length of one episode, which reads
        // as the length of the show. Its size says more.
        Kind::Show => {
            facts.extend(item.child_count.map(|seasons| text::count(seasons, "season")));
            facts.extend(item.leaf_count.map(|episodes| text::count(episodes, "episode")));
        }
        _ => facts.extend(item.duration.map(|duration| text::length(duration.into()))),
    }
    facts.extend(item.content_rating.clone());
    facts.extend(item.audience_rating.or(item.rating).map(|rating| format!("★ {rating:.1}")));
    facts.join(" · ")
}

/// Who made it and who is in it, one line each.
fn credits(item: &Item) -> String {
    let mut lines: Vec<String> = Vec::new();
    if !item.directors.is_empty() {
        lines.push(format!("Directed by {}", names(&item.directors, CAST_SHOWN).join(", ")));
    }
    if !item.roles.is_empty() {
        lines.push(format!("Starring {}", names(&item.roles, CAST_SHOWN).join(", ")));
    }
    lines.join("\n")
}

fn names(tags: &[plex::model::Tag], limit: usize) -> Vec<&str> {
    tags.iter().take(limit).map(|tag| tag.tag.as_str()).collect()
}

/// What the file is, in the words printed on a disc case: "4K", "HDR10",
/// "5.1".
fn badges(media: &Media) -> Vec<String> {
    let mut badges: Vec<String> = Vec::new();
    badges.extend(media.video_resolution.as_deref().map(|resolution| match resolution {
        "4k" => "4K".to_owned(),
        "sd" => "SD".to_owned(),
        lines => format!("{lines}p"),
    }));
    // The server titles a video track by its resolution and then, if it has
    // one, its dynamic range: "4K HDR10", "1080p".
    let video = media.parts.first().and_then(|part| part.embedded(StreamKind::Video).next());
    let title = video.and_then(|video| video.display_title.as_deref()).unwrap_or_default();
    let range: Vec<&str> = title.split_whitespace().skip(1).collect();
    if !range.is_empty() {
        badges.push(range.join(" "));
    }
    badges.extend(media.audio_channels.map(|channels| match channels {
        1 => "Mono".to_owned(),
        2 => "Stereo".to_owned(),
        6 => "5.1".to_owned(),
        8 => "7.1".to_owned(),
        other => format!("{other} ch"),
    }));
    badges
}

/// A few words in an outlined box.
fn badge(words: &str, main_thread: MainThreadMarker) -> Retained<NSView> {
    let frame = artwork::layered(main_thread);
    if let Some(layer) = frame.layer() {
        layer.setCornerRadius(theme::RADIUS_S);
        layer.setBorderWidth(theme::ARTWORK_OUTLINE_WIDTH);
        layer.setBorderColor(Some(&theme::white(theme::FACT_BADGE_BORDER_OPACITY).CGColor()));
    }
    // SAFETY: the weight is a constant that AppKit provides.
    let weight = unsafe { NSFontWeightSemibold };
    let label =
        text::label(theme::FONT_CAPTION, weight, &NSColor::secondaryLabelColor(), main_thread);
    text::set(&label, words);
    frame.addSubview(&label);
    let (across, down) = (theme::SPACE_S - theme::SPACE_XXS, theme::SPACE_XXS);
    NSLayoutConstraint::activateConstraints(&NSArray::from_retained_slice(&[
        label.leadingAnchor().constraintEqualToAnchor_constant(&frame.leadingAnchor(), across),
        label.trailingAnchor().constraintEqualToAnchor_constant(&frame.trailingAnchor(), -across),
        label.topAnchor().constraintEqualToAnchor_constant(&frame.topAnchor(), down),
        label.bottomAnchor().constraintEqualToAnchor_constant(&frame.bottomAnchor(), -down),
    ]));
    frame
}

fn button(title: &str, symbol: Option<&str>, main_thread: MainThreadMarker) -> Retained<NSButton> {
    // SAFETY: the target and action are set once the page exists.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str(title), None, None, main_thread)
    };
    if let Some(name) = symbol {
        button.setImage(artwork::symbol(name, theme::FONT_BODY).as_deref());
        button.setImagePosition(NSCellImagePosition::ImageLeading);
    }
    button
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: serde_json::Value) -> Item {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_show_states_its_size_and_not_the_length_of_an_episode() {
        let show = item(serde_json::json!({
            "ratingKey": "1", "type": "show", "title": "Title", "year": 2014,
            "duration": 2_520_000, "childCount": 7, "leafCount": 100,
            "contentRating": "TV-14", "audienceRating": 7.9,
        }));
        assert_eq!(facts(&show), "2014 · 7 seasons · 100 episodes · TV-14 · ★ 7.9");
    }

    #[test]
    fn a_film_states_its_length() {
        let film = item(serde_json::json!({
            "ratingKey": "1", "type": "movie", "title": "Title", "year": 2015,
            "duration": 8_286_624, "contentRating": "PG-13",
        }));
        assert_eq!(facts(&film), "2015 · 2 h 18 min · PG-13");
    }

    #[test]
    fn badges_name_the_picture_and_the_sound() {
        let film = item(serde_json::json!({
            "ratingKey": "1", "type": "movie", "title": "Title",
            "Media": [{
                "id": 1, "videoResolution": "4k", "audioChannels": 6,
                "Part": [{ "id": 1, "key": "/library/parts/1/0/file.mkv", "size": 1, "Stream": [
                    { "id": 1, "streamType": 1, "displayTitle": "4K HDR10" },
                ]}],
            }],
        }));
        assert_eq!(badges(&film.media[0]), ["4K", "HDR10", "5.1"]);
    }

    #[test]
    fn seasons_come_from_the_episodes_in_order() {
        let episode = |season: &str| {
            item(serde_json::json!({
                "ratingKey": "9", "type": "episode", "title": "Title",
                "parentRatingKey": season, "parentTitle": format!("Season {season}"),
            }))
        };
        let titles: Vec<String> = seasons(&[episode("1"), episode("1"), episode("2")])
            .into_iter()
            .map(|(_, title)| title)
            .collect();
        assert_eq!(titles, ["Season 1", "Season 2"]);
    }
}
