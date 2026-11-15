//! What the Activity page shows: everything the server keeps of what was
//! watched, gathered in one go, and summed up for a stretch of time.
//!
//! Days and hours are the viewer's own: a row of the server's is placed
//! by its time in seconds since 1970 and the offset of the local time
//! zone at that moment, which the caller supplies.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

use plex::MediaServer;
use plex::activity::{Account, Device, Play, Span, Statistics, Streamed, Watched};
use plex::model::{Item, Kind, RatingKey, Section};

use crate::error::with_causes;

const DAY: i64 = 86_400;

/// How many titles, genres and recent plays the page lists.
const TOP_TITLES: usize = 10;
const TOP_GENRES: usize = 8;
const RECENT_PLAYS: usize = 12;

/// The longest stretch, in days, that the chart of "All Time" still
/// shows a day at a time. Longer, it shows months.
const DAYS_IN_A_DAILY_CHART: i64 = 62;

/// Everything the server said, before it is summed up.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Record {
    /// Newest first.
    pub plays: Vec<Play>,
    /// `None` when the server did not give its statistics.
    pub watched: Option<Rows<Watched>>,
    pub streamed: Option<Rows<Streamed>>,
    pub accounts: Vec<Account>,
    pub devices: Vec<Device>,
    /// What was played, and the shows of the episodes, by rating key.
    pub items: HashMap<RatingKey, Item>,
    pub libraries: Vec<Library>,
}

/// Rows of statistics: a month a row for the months before the server's
/// hour by hour rows start, then an hour a row.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows<T> {
    pub rows: Vec<T>,
    /// When the rows of an hour start. Rows before it are of a month.
    pub hourly_from: Option<u64>,
}

/// How much of a library has been watched.
#[derive(Debug, Clone, PartialEq)]
pub struct Library {
    pub title: String,
    /// Films, or the episodes of a library of shows.
    pub kind: Kind,
    pub total: u32,
    pub watched: u32,
}

/// Asks the server for everything the page shows. Only the history is
/// needed: without the rest the page shows less.
pub async fn gather(server: MediaServer, sections: Vec<Section>) -> plex::Result<Record> {
    let (plays, hourly, monthly, hourly_data, monthly_data, libraries) = tokio::join!(
        server.history(),
        server.watch_time(Span::Hour),
        server.watch_time(Span::Month),
        server.streamed(Span::Hour),
        server.streamed(Span::Month),
        libraries(&server, &sections),
    );
    let plays = plays?;
    let watched = both(hourly, monthly)?;
    let streamed = both(hourly_data, monthly_data)?;

    let mut keys: Vec<RatingKey> = Vec::new();
    let mut seen = HashSet::new();
    let played = plays.iter().flat_map(|play| [&play.rating_key, &play.grandparent_rating_key]);
    for key in played.flatten() {
        if seen.insert(key) {
            keys.push(key.clone());
        }
    }
    let items = optional(server.items(&keys).await)?.unwrap_or_default();

    // Each answer names the accounts and devices of its own rows.
    let (mut accounts, mut devices) = (Vec::new(), Vec::new());
    let watched_names = watched.iter().map(|(_, accounts, devices)| (accounts, devices));
    let streamed_names = streamed.iter().map(|(_, accounts, devices)| (accounts, devices));
    for (named_accounts, named_devices) in watched_names.chain(streamed_names) {
        add_new(&mut accounts, named_accounts, |account| account.id);
        add_new(&mut devices, named_devices, |device| device.id);
    }
    Ok(Record {
        plays,
        watched: watched.map(|(rows, ..)| rows),
        streamed: streamed.map(|(rows, ..)| rows),
        accounts,
        devices,
        items: items.into_iter().map(|item| (item.rating_key.clone(), item)).collect(),
        libraries: libraries?,
    })
}

fn add_new<T: Clone>(known: &mut Vec<T>, more: &[T], id: impl Fn(&T) -> u32) {
    for one in more {
        if !known.iter().any(|known| id(known) == id(one)) {
            known.push(one.clone());
        }
    }
}

type Merged<T> = (Rows<T>, Vec<Account>, Vec<Device>);

/// The hour by hour rows, after the monthly rows of the months they do
/// not reach back to.
fn both<T: At>(
    hourly: plex::Result<Statistics<T>>,
    monthly: plex::Result<Statistics<T>>,
) -> plex::Result<Option<Merged<T>>> {
    let (Some(hourly), Some(monthly)) = (optional(hourly)?, optional(monthly)?) else {
        return Ok(None);
    };
    let rows = merge(hourly.rows, monthly.rows);
    Ok(Some((rows, hourly.accounts, hourly.devices)))
}

/// A row of statistics, which starts at a time.
pub trait At {
    fn at(&self) -> u64;
}

impl At for Watched {
    fn at(&self) -> u64 {
        self.at
    }
}

impl At for Streamed {
    fn at(&self) -> u64 {
        self.at
    }
}

/// Keeps the monthly rows of months before the month the first hourly
/// row is in, then every hourly row. The server keeps both for the same
/// time, so taking both for a month would count it twice.
fn merge<T: At>(hourly: Vec<T>, monthly: Vec<T>) -> Rows<T> {
    let hourly_from = hourly.iter().map(At::at).min();
    // The start of the month the hourly rows start in: the latest monthly
    // row that starts no later than they do.
    let cut =
        hourly_from.and_then(|from| monthly.iter().map(At::at).filter(|at| *at <= from).max());
    let mut rows: Vec<T> = match (hourly_from, cut) {
        (None, _) => monthly,
        (Some(from), cut) => {
            let cut = cut.unwrap_or(from);
            monthly.into_iter().filter(|row| row.at() < cut).collect()
        }
    };
    rows.extend(hourly);
    Rows { rows, hourly_from }
}

/// A part of the answer the page can do without: missing from what was
/// saved, or refused. Only a sign-in that is over stops the page.
fn optional<T>(answer: plex::Result<T>) -> plex::Result<Option<T>> {
    match answer {
        Ok(answer) => Ok(Some(answer)),
        Err(error) if error.is_unauthorized() => Err(error),
        Err(plex::Error::NotRemembered) => Ok(None),
        Err(error) => {
            eprintln!("plex-rust: {}", with_causes(&error));
            Ok(None)
        }
    }
}

/// How much of each movie and TV library has been watched. A library
/// the server does not count is left out.
async fn libraries(server: &MediaServer, sections: &[Section]) -> plex::Result<Vec<Library>> {
    let mut libraries = Vec::new();
    for section in sections {
        let kind = match section.kind {
            Kind::Movie => Kind::Movie,
            Kind::Show => Kind::Episode,
            _ => continue,
        };
        let total = optional(server.count(section, kind, false).await)?;
        let unwatched = optional(server.count(section, kind, true).await)?;
        if let (Some(total), Some(unwatched)) = (total, unwatched) {
            let watched = total.saturating_sub(unwatched);
            libraries.push(Library { title: section.title.clone(), kind, total, watched });
        }
    }
    Ok(libraries)
}

/// The stretch of time the page sums up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Range {
    Week,
    #[default]
    Month,
    Year,
    All,
}

impl Range {
    pub const ALL: [Self; 4] = [Self::Week, Self::Month, Self::Year, Self::All];

    pub fn title(self) -> &'static str {
        match self {
            Self::Week => "7 Days",
            Self::Month => "30 Days",
            Self::Year => "12 Months",
            Self::All => "All Time",
        }
    }
}

/// What a bar of the chart covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Day,
    Month,
}

/// The time watched in a day or a month, by kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bar {
    /// The first day it covers, counted in the viewer's days since 1970.
    pub day: i64,
    pub episodes: u64,
    pub films: u64,
}

/// One thing among others, with what it adds up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub name: String,
    /// Words under the name, such as the platform of a device.
    pub detail: String,
    pub seconds: u64,
    pub plays: usize,
}

/// A film, or a show with all its episodes.
#[derive(Debug, Clone, PartialEq)]
pub struct Title {
    pub name: String,
    pub kind: Kind,
    pub thumb: Option<String>,
    /// The film or show, when it is still in the library.
    pub item: Option<Item>,
    pub plays: usize,
    /// Each play counted at the full length of what was played.
    pub seconds: u64,
    pub last: u64,
}

/// What was streamed, and how much of it in the server's own network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Data {
    pub bytes: u64,
    pub nearby: u64,
    /// By device, the most first, with bytes in `seconds`' place.
    pub devices: Vec<Share>,
}

/// The time watched in a cell of the week: a day of the week, Monday
/// first, and an hour of the day.
pub type Heat = [[u64; 24]; 7];

/// Everything the page shows for one range.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub range: Range,
    /// The days the range covers, inclusive.
    pub first_day: i64,
    pub last_day: i64,
    /// Whether the server gave the time watched.
    pub timed: bool,
    pub episode_seconds: u64,
    pub film_seconds: u64,
    pub episode_plays: usize,
    pub film_plays: usize,
    /// Films and shows, each counted once.
    pub titles: usize,
    pub days_watched: usize,
    pub longest_streak: usize,
    /// The days in a row up to today, or up to yesterday when nothing has
    /// been watched yet today.
    pub current_streak: usize,
    pub busiest: Option<(i64, u64)>,
    pub unit: Unit,
    pub bars: Vec<Bar>,
    pub heat: Heat,
    pub top: Vec<Title>,
    pub genres: Vec<Share>,
    /// By the decade a film came out or a show started, oldest first.
    pub decades: Vec<Share>,
    pub devices: Vec<Share>,
    /// Only when more than one account has watched.
    pub accounts: Vec<Share>,
    pub data: Option<Data>,
    pub recent: Vec<Play>,
    /// When hour by hour statistics start.
    pub hourly_from: Option<u64>,
}

impl Summary {
    pub fn seconds(&self) -> u64 {
        self.episode_seconds + self.film_seconds
    }

    pub fn plays(&self) -> usize {
        self.episode_plays + self.film_plays
    }
}

/// The viewer's clock: now, and the offset of local time from UTC, in
/// seconds, at any moment.
pub struct Clock<'a> {
    pub now: u64,
    pub offset: &'a dyn Fn(u64) -> i64,
}

impl Clock<'_> {
    /// The local day `at` falls on, counted from 1970-01-01.
    pub fn day(&self, at: u64) -> i64 {
        local(at, self).div_euclid(DAY)
    }

    /// The local hour of the day `at` falls in, from 0 to 23.
    fn hour(&self, at: u64) -> usize {
        usize::try_from(local(at, self).rem_euclid(DAY) / 3600).unwrap_or(0)
    }
}

fn local(at: u64, clock: &Clock) -> i64 {
    i64::try_from(at).unwrap_or(i64::MAX).saturating_add((clock.offset)(at))
}

/// The day of the week of `day`, Monday as 0. The first day of 1970 was a
/// Thursday.
pub fn weekday(day: i64) -> usize {
    usize::try_from((day + 3).rem_euclid(7)).unwrap_or(0)
}

/// The year, month from 1 and day of the month of `day`. Howard
/// Hinnant's `civil_from_days`, for the proleptic Gregorian calendar.
pub fn civil(day: i64) -> (i64, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let of_era = z.rem_euclid(146_097);
    let year_of_era = (of_era - of_era / 1460 + of_era / 36_524 - of_era / 146_096) / 365;
    let of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * of_year + 2) / 153;
    let day_of_month = of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    // Both are in range by construction: 1 to 31, and 1 to 12.
    (year, u32::try_from(month).unwrap_or(1), u32::try_from(day_of_month).unwrap_or(1))
}

/// The day of `year`-`month`-01. Hinnant's `days_from_civil`.
pub fn first_of_month(year: i64, month: u32) -> i64 {
    let month = i64::from(month);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The first day of the month `day` is in.
fn month_of(day: i64) -> i64 {
    let (year, month, _) = civil(day);
    first_of_month(year, month)
}

/// The first day of the month `months` months after the one `day` is in.
fn months_after(day: i64, months: i64) -> i64 {
    let (year, month, _) = civil(day);
    let index = year * 12 + i64::from(month) - 1 + months;
    first_of_month(index.div_euclid(12), u32::try_from(index.rem_euclid(12) + 1).unwrap_or(1))
}

impl Record {
    /// What `range` comes to, up to now.
    pub fn summary(&self, range: Range, clock: &Clock) -> Summary {
        let today = clock.day(clock.now);
        let first_day = match range {
            Range::Week => today - 6,
            Range::Month => today - 29,
            Range::Year => months_after(today, -11),
            Range::All => self.first_day(clock).unwrap_or(today),
        };
        let unit = match range {
            Range::Week | Range::Month => Unit::Day,
            Range::All if today - first_day < DAYS_IN_A_DAILY_CHART => Unit::Day,
            Range::Year | Range::All => Unit::Month,
        };
        let within = |at: u64| (first_day..=today).contains(&clock.day(at));

        let plays: Vec<&Play> = self.plays.iter().filter(|play| within(play.viewed_at)).collect();
        let watched: Vec<&Watched> = self
            .watched
            .iter()
            .flat_map(|rows| &rows.rows)
            .filter(|row| within(row.at) && row.seconds > 0)
            .collect();

        // The bars, every one there even when nothing was watched in it.
        let mut bars = Vec::new();
        let mut start = match unit {
            Unit::Day => first_day,
            Unit::Month => month_of(first_day),
        };
        while start <= today {
            bars.push(Bar { day: start, episodes: 0, films: 0 });
            start = match unit {
                Unit::Day => start + 1,
                Unit::Month => months_after(start, 1),
            };
        }
        let (mut episode_seconds, mut film_seconds) = (0, 0);
        let mut by_day: BTreeMap<i64, u64> = BTreeMap::new();
        let mut heat: Heat = [[0; 24]; 7];
        for row in &watched {
            let day = clock.day(row.at);
            *by_day.entry(day).or_default() += row.seconds;
            let bar_start = match unit {
                Unit::Day => day,
                Unit::Month => month_of(day),
            };
            let bar = bars.iter_mut().find(|bar| bar.day == bar_start);
            match row.kind {
                Kind::Movie => {
                    film_seconds += row.seconds;
                    if let Some(bar) = bar {
                        bar.films += row.seconds;
                    }
                }
                _ => {
                    episode_seconds += row.seconds;
                    if let Some(bar) = bar {
                        bar.episodes += row.seconds;
                    }
                }
            }
            let hourly = self.watched.as_ref().and_then(|rows| rows.hourly_from);
            if hourly.is_some_and(|from| row.at >= from) {
                heat[weekday(day)][clock.hour(row.at)] += row.seconds;
            }
        }

        let mut days: HashSet<i64> = by_day.keys().copied().collect();
        days.extend(plays.iter().map(|play| clock.day(play.viewed_at)));
        let busiest = by_day.iter().max_by_key(|(day, seconds)| (**seconds, -**day));
        let busiest = busiest.map(|(day, seconds)| (*day, *seconds));

        let mut accounts = self.shares_of_accounts(&plays, &watched);
        if accounts.len() < 2 {
            accounts.clear();
        }
        Summary {
            range,
            first_day,
            last_day: today,
            timed: self.watched.is_some(),
            episode_seconds,
            film_seconds,
            episode_plays: plays.iter().filter(|play| play.kind != Kind::Movie).count(),
            film_plays: plays.iter().filter(|play| play.kind == Kind::Movie).count(),
            titles: plays.iter().map(|play| title_key(play)).collect::<HashSet<_>>().len(),
            days_watched: days.len(),
            longest_streak: longest_streak(&days),
            current_streak: current_streak(&days, today),
            busiest,
            unit,
            bars,
            heat,
            top: self.top_titles(&plays),
            genres: self.genres(&plays),
            decades: self.decades(&plays),
            devices: self.shares_of_devices(&plays, &watched),
            accounts,
            data: self.data(&within),
            recent: plays.iter().take(RECENT_PLAYS).map(|play| (*play).clone()).collect(),
            hourly_from: self.watched.as_ref().and_then(|rows| rows.hourly_from),
        }
    }

    /// What device `id` is called, as the server's statistics name it.
    pub fn device_name(&self, id: u32) -> String {
        let device = self.devices.iter().find(|device| device.id == id);
        device.map_or_else(|| "Unknown device".to_owned(), device_name)
    }

    /// The day of the oldest play or statistic.
    fn first_day(&self, clock: &Clock) -> Option<i64> {
        let watched = self.watched.iter().flat_map(|rows| &rows.rows).map(|row| row.at);
        let played = self.plays.iter().map(|play| play.viewed_at);
        watched.chain(played).min().map(|at| clock.day(at))
    }

    /// The length of what `play` played, from the library.
    fn seconds_of(&self, play: &Play) -> u64 {
        let item = play.rating_key.as_ref().and_then(|key| self.items.get(key));
        item.and_then(|item| item.duration).map_or(0, |length| length.0 / 1000)
    }

    /// The film, or for an episode its show.
    fn title_item(&self, play: &Play) -> Option<&Item> {
        let key = match play.kind {
            Kind::Movie => play.rating_key.as_ref(),
            _ => play.grandparent_rating_key.as_ref(),
        };
        key.and_then(|key| self.items.get(key))
    }

    fn top_titles(&self, plays: &[&Play]) -> Vec<Title> {
        let mut titles: HashMap<String, Title> = HashMap::new();
        for play in plays {
            let item = self.title_item(play);
            let title = titles.entry(title_key(play)).or_insert_with(|| Title {
                name: play.grandparent_title.clone().unwrap_or_else(|| play.title.clone()),
                kind: if play.kind == Kind::Movie { Kind::Movie } else { Kind::Show },
                thumb: play.grandparent_thumb.clone().or_else(|| play.thumb.clone()),
                item: item.cloned(),
                plays: 0,
                seconds: 0,
                last: 0,
            });
            title.plays += 1;
            title.seconds += self.seconds_of(play);
            title.last = title.last.max(play.viewed_at);
        }
        let mut titles: Vec<Title> = titles.into_values().collect();
        titles.sort_by_key(|title| Reverse((title.plays, title.seconds, title.last)));
        titles.truncate(TOP_TITLES);
        titles
    }

    fn genres(&self, plays: &[&Play]) -> Vec<Share> {
        let mut genres: HashMap<&str, Share> = HashMap::new();
        for play in plays {
            let Some(item) = self.title_item(play) else { continue };
            for genre in &item.genres {
                let share = genres.entry(&genre.tag).or_insert_with(|| share(&genre.tag, ""));
                share.seconds += self.seconds_of(play);
                share.plays += 1;
            }
        }
        let mut genres: Vec<Share> = genres.into_values().collect();
        genres.sort_by(|a, b| (b.plays, b.seconds, &a.name).cmp(&(a.plays, a.seconds, &b.name)));
        genres.truncate(TOP_GENRES);
        genres
    }

    fn decades(&self, plays: &[&Play]) -> Vec<Share> {
        let mut decades: BTreeMap<u16, Share> = BTreeMap::new();
        for play in plays {
            let Some(year) = self.title_item(play).and_then(|item| item.year) else { continue };
            let decade = year / 10 * 10;
            let name = format!("{decade}s");
            let share = decades.entry(decade).or_insert_with(|| share(&name, ""));
            share.seconds += self.seconds_of(play);
            share.plays += 1;
        }
        decades.into_values().collect()
    }

    fn shares_of_devices(&self, plays: &[&Play], watched: &[&Watched]) -> Vec<Share> {
        let mut shares = Shares::default();
        for device in &self.devices {
            shares.name(device.id, device_name(device), platform(&device.platform));
        }
        for row in watched {
            shares.add(row.device, row.seconds, 0);
        }
        for play in plays {
            shares.add(play.device, 0, 1);
        }
        shares.ranked()
    }

    fn shares_of_accounts(&self, plays: &[&Play], watched: &[&Watched]) -> Vec<Share> {
        let mut shares = Shares::default();
        for account in &self.accounts {
            shares.name(account.id, account.name.clone(), String::new());
        }
        for row in watched {
            shares.add(row.account, row.seconds, 0);
        }
        for play in plays {
            shares.add(play.account, 0, 1);
        }
        shares.ranked()
    }

    fn data(&self, within: &dyn Fn(u64) -> bool) -> Option<Data> {
        let rows = self.streamed.as_ref()?;
        let mut shares = Shares::default();
        for device in &self.devices {
            shares.name(device.id, device_name(device), platform(&device.platform));
        }
        let (mut bytes, mut nearby) = (0, 0);
        for row in rows.rows.iter().filter(|row| within(row.at)) {
            bytes += row.bytes;
            if row.lan {
                nearby += row.bytes;
            }
            shares.add(row.device, row.bytes, 0);
        }
        Some(Data { bytes, nearby, devices: shares.ranked() })
    }
}

fn share(name: &str, detail: &str) -> Share {
    Share { name: name.to_owned(), detail: detail.to_owned(), seconds: 0, plays: 0 }
}

/// What one title's plays are counted under: the show of an episode, or
/// the film. By title where the server no longer has the item.
fn title_key(play: &Play) -> String {
    let key = match play.kind {
        Kind::Movie => play.rating_key.as_ref(),
        _ => play.grandparent_rating_key.as_ref(),
    };
    match key {
        Some(key) => key.to_string(),
        None => play.grandparent_title.clone().unwrap_or_else(|| play.title.clone()),
    }
}

/// Sums by the server's number for an account or a device. Numbers with
/// the same name and detail, as a device that reported itself twice, are
/// summed as one.
#[derive(Default)]
struct Shares {
    names: HashMap<u32, (String, String)>,
    sums: Vec<Share>,
}

impl Shares {
    fn name(&mut self, id: u32, name: String, detail: String) {
        self.names.insert(id, (name, detail));
    }

    fn add(&mut self, id: u32, seconds: u64, plays: usize) {
        let (name, detail) =
            self.names.get(&id).cloned().unwrap_or_else(|| ("Unknown".to_owned(), String::new()));
        let same = |share: &&mut Share| {
            share.name.eq_ignore_ascii_case(&name) && share.detail.eq_ignore_ascii_case(&detail)
        };
        match self.sums.iter_mut().find(same) {
            Some(share) => {
                share.seconds += seconds;
                share.plays += plays;
            }
            None => self.sums.push(Share { name, detail, seconds, plays }),
        }
    }

    fn ranked(mut self) -> Vec<Share> {
        self.sums.retain(|share| share.seconds > 0 || share.plays > 0);
        self.sums.sort_by_key(|share| Reverse((share.seconds, share.plays)));
        self.sums
    }
}

fn device_name(device: &Device) -> String {
    match (device.name.as_str(), device.platform.as_str()) {
        ("", "") => "Unnamed device".to_owned(),
        ("", platform) => self::platform(platform),
        (name, _) => name.to_owned(),
    }
}

/// The platform as its maker writes it. The server passes on what each
/// app calls itself, in any case.
fn platform(platform: &str) -> String {
    match platform.to_ascii_lowercase().as_str() {
        "macos" => "macOS".to_owned(),
        "ios" => "iOS".to_owned(),
        "tvos" => "tvOS".to_owned(),
        "ipados" => "iPadOS".to_owned(),
        _ => platform.to_owned(),
    }
}

/// The longest run of days in a row in `days`.
fn longest_streak(days: &HashSet<i64>) -> usize {
    let starts = days.iter().filter(|day| !days.contains(&(*day - 1)));
    starts.map(|start| (0..).take_while(|n| days.contains(&(start + n))).count()).max().unwrap_or(0)
}

/// The run of days in a row that ends today, or yesterday.
fn current_streak(days: &HashSet<i64>, today: i64) -> usize {
    let end = if days.contains(&today) { today } else { today - 1 };
    (0..).take_while(|n| days.contains(&(end - n))).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3600;
    /// 2026-11-14, a Saturday, at midnight UTC.
    const SATURDAY: u64 = 1_794_614_400;

    fn utc() -> impl Fn(u64) -> i64 {
        |_| 0
    }

    fn play(key: &str, show: Option<&str>, at: u64) -> Play {
        Play {
            rating_key: Some(RatingKey::new(key)),
            kind: if show.is_some() { Kind::Episode } else { Kind::Movie },
            title: key.to_owned(),
            grandparent_title: show.map(str::to_owned),
            grandparent_rating_key: show.map(RatingKey::new),
            thumb: None,
            grandparent_thumb: None,
            parent_index: None,
            index: None,
            viewed_at: at,
            account: 1,
            device: 1,
        }
    }

    fn watched(at: u64, kind: Kind, seconds: u64) -> Watched {
        Watched { at, account: 1, device: 1, kind, plays: 0, seconds }
    }

    fn item(key: &str, minutes: u64) -> (RatingKey, Item) {
        let json = serde_json::json!({
            "ratingKey": key, "type": "episode", "title": key,
            "duration": minutes * 60_000,
        });
        let item: Item = serde_json::from_value(json).unwrap();
        (RatingKey::new(key), item)
    }

    #[test]
    fn monthly_rows_stand_in_only_for_months_the_hourly_rows_miss() {
        let month = |at| watched(at, Kind::Episode, 100);
        let hourly = vec![watched(SATURDAY, Kind::Episode, 7)];
        // October, then November, which the hourly row is in.
        let monthly = vec![month(SATURDAY - 40 * 86_400), month(SATURDAY - 13 * 86_400)];
        let rows = merge(hourly, monthly);
        let seconds: Vec<u64> = rows.rows.iter().map(|row| row.seconds).collect();
        assert_eq!(seconds, [100, 7]);
        assert_eq!(rows.hourly_from, Some(SATURDAY));
    }

    #[test]
    fn days_and_hours_are_the_viewers_own() {
        // Eight hours behind UTC, 01:00 UTC on Saturday is 17:00 on Friday.
        let behind = |_| -8 * 3600;
        let clock = Clock { now: SATURDAY, offset: &behind };
        let at = SATURDAY + HOUR;
        assert_eq!(weekday(clock.day(at)), 4);
        assert_eq!(clock.hour(at), 17);
        assert_eq!(civil(clock.day(SATURDAY)), (2026, 11, 13));
    }

    #[test]
    fn the_calendar_turns_over_at_month_and_year_ends() {
        let new_year = first_of_month(2027, 1);
        assert_eq!(civil(new_year), (2027, 1, 1));
        assert_eq!(civil(new_year - 1), (2026, 12, 31));
        assert_eq!(months_after(new_year - 1, -11), first_of_month(2026, 1));
        assert_eq!(civil(first_of_month(2028, 3) - 1), (2028, 2, 29));
    }

    #[test]
    fn a_range_counts_what_falls_in_it_and_no_more() {
        let offset = utc();
        let clock = Clock { now: SATURDAY + 20 * HOUR, offset: &offset };
        let record = Record {
            plays: vec![
                play("e2", Some("show"), SATURDAY + 19 * HOUR),
                play("e1", Some("show"), SATURDAY - 86_400),
                play("film", None, SATURDAY - 10 * 86_400),
            ],
            watched: Some(Rows {
                rows: vec![
                    watched(SATURDAY - 10 * 86_400, Kind::Movie, 6000),
                    watched(SATURDAY - 86_400 + 21 * HOUR, Kind::Episode, 1200),
                    watched(SATURDAY + 19 * HOUR, Kind::Episode, 1500),
                ],
                hourly_from: Some(0),
            }),
            items: [item("e1", 20), item("e2", 25), item("film", 100)].into_iter().collect(),
            ..Record::default()
        };

        let week = record.summary(Range::Week, &clock);
        assert_eq!((week.episode_seconds, week.film_seconds), (2700, 0));
        assert_eq!((week.episode_plays, week.film_plays, week.titles), (2, 0, 1));
        assert_eq!(week.bars.len(), 7);
        assert_eq!(week.bars[6].episodes, 1500);
        assert_eq!((week.days_watched, week.longest_streak, week.current_streak), (2, 2, 2));
        assert_eq!(week.heat[5][19], 1500);
        assert_eq!(week.top[0].seconds, 45 * 60);

        let month = record.summary(Range::Month, &clock);
        assert_eq!((month.film_seconds, month.titles, month.longest_streak), (6000, 2, 2));
        assert_eq!(month.busiest, Some((clock.day(SATURDAY - 10 * 86_400), 6000)));
        // Ranked by plays: the show's two before the film's one.
        let ranked: Vec<&str> = month.top.iter().map(|title| title.name.as_str()).collect();
        assert_eq!(ranked, ["show", "film"]);
    }

    #[test]
    fn a_device_that_reported_itself_twice_is_one_device() {
        let record = Record {
            plays: vec![play("film", None, SATURDAY)],
            devices: vec![
                Device { id: 1, name: "Mac".to_owned(), platform: "macos".to_owned() },
                Device { id: 2, name: "Mac".to_owned(), platform: "macOS".to_owned() },
            ],
            watched: Some(Rows {
                rows: vec![
                    Watched { device: 2, ..watched(SATURDAY, Kind::Movie, 50) },
                    watched(SATURDAY, Kind::Movie, 10),
                ],
                hourly_from: Some(0),
            }),
            ..Record::default()
        };
        let offset = utc();
        let clock = Clock { now: SATURDAY, offset: &offset };
        let devices = record.summary(Range::All, &clock).devices;
        assert_eq!(
            devices,
            [Share { name: "Mac".to_owned(), detail: "macOS".to_owned(), seconds: 60, plays: 1 }]
        );
    }
}
