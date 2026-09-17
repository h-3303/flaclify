//! The search panel of the Get view (Tier 2): search MusicBrainz for songs, albums and artists,
//! tick what is wanted, and hand the names to `flacli get`; or name the music directly, as
//! `flacli get` takes it. MusicBrainz supplies names only; flacli finds the files on Soulseek
//! and files them.
//!
//! This replaced the blind "Search Soulseek" fetch, which handed a bare term to flacli. flacli
//! reads a bare term as a song title, so "black box recorder" fetched whichever recording bore
//! that title rather than the band's music. Here nothing is fetched until it has been named.

use adw::prelude::*;
use gtk::{gio, glib, glib::clone};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
    time::Duration,
};

use super::{actions::fetch, controller::flacli};
use crate::{
    meta_providers::musicbrainz::{
        FoundAlbum, FoundArtist, FoundSong, browse_release_groups, format_length, musicbrainz_enabled,
        release_group_tracks, search_albums, search_artists, search_songs,
    },
    window::EuphonicaWindow,
};

/// MusicBrainz asks for one request a second.
const PAUSE: Duration = Duration::from_millis(1100);

const SONGS: &str = "songs";
const ALBUMS: &str = "albums";
const ARTISTS: &str = "artists";
const ASK: &str = "ask";

const PLACEHOLDER: &str = "Search MusicBrainz for a song, an album or an artist, or type “Artist - Title”. Tick what you want; flacli fetches it.";

/// One thing the user has ticked, as `flacli get` takes it.
#[derive(Debug, Clone)]
enum Pick {
    Song { artist: String, title: String, album: String },
    Album { artist: String, title: String },
}

impl Pick {
    /// A JSON item for `flacli get`, so titles with a dash in them survive.
    fn item(&self) -> String {
        match self {
            Pick::Song { artist, title, album } => {
                serde_json::json!({"kind": "track", "artist": artist, "title": title, "album": album}).to_string()
            }
            Pick::Album { artist, title } => {
                serde_json::json!({"kind": "album", "artist": artist, "album": title}).to_string()
            }
        }
    }

    /// One line of the confirmation: "Artist – Title" or "Artist – Album (whole album)".
    fn describe(&self) -> String {
        match self {
            Pick::Song { artist, title, .. } if artist.is_empty() => title.clone(),
            Pick::Song { artist, title, .. } => format!("{artist} – {title}"),
            Pick::Album { artist, title } if artist.is_empty() => format!("{title} (whole album)"),
            Pick::Album { artist, title } => format!("{artist} – {title} (whole album)"),
        }
    }
}

type Basket = Rc<RefCell<BTreeMap<String, Pick>>>;
type Changed = Rc<dyn Fn()>;
/// Every tick box made, so a Get can clear them all.
type Checks = Rc<RefCell<Vec<glib::WeakRef<gtk::CheckButton>>>>;

/// "2 songs and 1 album".
fn describe_picks(songs: usize, albums: usize) -> String {
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    match (songs, albums) {
        (0, 0) => "Nothing ticked yet".to_owned(),
        (s, 0) => plural(s, "song"),
        (0, a) => plural(a, "album"),
        (s, a) => format!("{} and {}", plural(s, "song"), plural(a, "album")),
    }
}

/// One page of results: a heading and a list, or a line of text while there is nothing to list.
#[derive(Clone)]
struct Page {
    stack: gtk::Stack,
    heading: gtk::Label,
    list: gtk::ListBox,
    status: adw::StatusPage,
}

impl Page {
    fn new() -> Self {
        let heading = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .css_classes(["heading"])
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&heading);
        content.append(&list);
        let clamp = adw::Clamp::builder().maximum_size(900).child(&content).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let status = adw::StatusPage::builder().description(PLACEHOLDER).build();
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(&status, Some("status"));
        stack.add_named(&scroller, Some("list"));
        stack.set_visible_child_name("status");
        Self { stack, heading, list, status }
    }

    fn set_status(&self, text: &str) {
        self.status.set_description(Some(text));
        self.stack.set_visible_child_name("status");
    }

    fn clear(&self) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        self.heading.set_visible(false);
    }

    fn set_heading(&self, text: Option<&str>) {
        self.heading.set_label(text.unwrap_or(""));
        self.heading.set_visible(text.is_some());
    }

    fn show_list(&self) {
        self.stack.set_visible_child_name("list");
    }
}

/// What every row builder needs: the basket, the change callback and the tick registry.
#[derive(Clone)]
struct Ctx {
    basket: Basket,
    changed: Changed,
    checks: Checks,
}

/// A row with a tick box that puts `pick` in the basket under `key`.
fn check_row(title: &str, subtitle: &str, suffix: &str, key: String, pick: Pick, ctx: &Ctx) -> (adw::ActionRow, gtk::CheckButton) {
    let check = gtk::CheckButton::builder()
        .valign(gtk::Align::Center)
        .active(ctx.basket.borrow().contains_key(&key))
        .build();
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .use_markup(false)
        .activatable_widget(&check)
        .build();
    row.add_prefix(&check);
    if !suffix.is_empty() {
        row.add_suffix(
            &gtk::Label::builder()
                .label(suffix)
                .valign(gtk::Align::Center)
                .css_classes(["dim-label", "numeric"])
                .build(),
        );
    }
    let (basket, changed) = (ctx.basket.clone(), ctx.changed.clone());
    check.connect_toggled(move |check| {
        if check.is_active() {
            basket.borrow_mut().insert(key.clone(), pick.clone());
        } else {
            basket.borrow_mut().remove(&key);
        }
        changed();
    });
    ctx.checks.borrow_mut().push(check.downgrade());
    (row, check)
}

fn song_row(song: &FoundSong, ctx: &Ctx) -> adw::ActionRow {
    let mut subtitle = song.artist.clone();
    if !song.album.is_empty() {
        subtitle.push_str(" · ");
        subtitle.push_str(&song.album);
    }
    if let Some(year) = song.year.as_deref() {
        subtitle.push_str(" · ");
        subtitle.push_str(year);
    }
    let pick = Pick::Song {
        artist: song.artist.clone(),
        title: song.title.clone(),
        album: song.album.clone(),
    };
    check_row(&song.title, &subtitle, &format_length(song.length_ms), format!("song:{}", song.mbid), pick, ctx).0
}

/// An album row: open it to tick tracks, or tick its first inner row for the whole album. The
/// tracklist is fetched on the first opening. While the whole album is ticked its tracks are
/// greyed out, since flacli's album mode fetches the whole folder.
fn album_row(album: &FoundAlbum, ctx: &Ctx) -> adw::ExpanderRow {
    let key = format!("album:{}", album.mbid);
    let row = adw::ExpanderRow::builder()
        .title(&album.title)
        .subtitle(album.describe())
        .use_markup(false)
        .show_enable_switch(false)
        .build();

    let track_checks: Rc<RefCell<Vec<gtk::CheckButton>>> = Rc::new(RefCell::new(Vec::new()));
    let (whole_row, album_check) = check_row(
        "Whole album",
        "Every track, as one folder from one share",
        "",
        key,
        Pick::Album {
            artist: album.artist.clone(),
            title: album.title.clone(),
        },
        ctx,
    );
    whole_row.add_css_class("property");
    row.add_row(&whole_row);
    album_check.connect_toggled(clone!(
        #[strong]
        track_checks,
        move |check| {
            let whole = check.is_active();
            for track in track_checks.borrow().iter() {
                if whole {
                    track.set_active(false);
                }
                track.set_sensitive(!whole);
            }
        }
    ));

    let loaded = Rc::new(Cell::new(false));
    let album = album.clone();
    let ctx = ctx.clone();
    row.connect_expanded_notify(clone!(
        #[strong]
        track_checks,
        #[weak]
        album_check,
        move |row| {
            if !row.is_expanded() || loaded.get() {
                return;
            }
            loaded.set(true);
            let placeholder = adw::ActionRow::builder()
                .title("Reading the tracklist from MusicBrainz…")
                .sensitive(false)
                .build();
            row.add_row(&placeholder);
            let (mbid, artist, album_title) = (album.mbid.clone(), album.artist.clone(), album.title.clone());
            glib::spawn_future_local(clone!(
                #[weak]
                row,
                #[strong]
                ctx,
                #[strong]
                track_checks,
                #[weak]
                album_check,
                async move {
                    let (id, credited) = (mbid.clone(), artist.clone());
                    let tracks = gio::spawn_blocking(move || release_group_tracks(&id, &credited)).await;
                    row.remove(&placeholder);
                    let tracks = match tracks {
                        Ok(Ok(tracks)) => tracks,
                        Ok(Err(e)) => {
                            row.add_row(&adw::ActionRow::builder().title(format!("MusicBrainz did not answer: {e}")).sensitive(false).build());
                            return;
                        }
                        Err(_) => return,
                    };
                    if tracks.is_empty() {
                        row.add_row(&adw::ActionRow::builder().title("MusicBrainz lists no tracks for this release").sensitive(false).build());
                        return;
                    }
                    let whole = album_check.is_active();
                    for track in tracks {
                        let pick = Pick::Song {
                            artist: track.artist.clone(),
                            title: track.title.clone(),
                            album: album_title.clone(),
                        };
                        let subtitle = if track.artist == artist { String::new() } else { track.artist.clone() };
                        let (track_row, check) = check_row(
                            &format!("{}. {}", track.position, track.title),
                            &subtitle,
                            &format_length(track.length_ms),
                            format!("track:{mbid}:{}", track.position),
                            pick,
                            &ctx,
                        );
                        check.set_sensitive(!whole);
                        row.add_row(&track_row);
                        track_checks.borrow_mut().push(check);
                    }
                }
            ));
        }
    ));
    row
}

fn artist_row(artist: &FoundArtist, open: Rc<dyn Fn(FoundArtist)>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(&artist.name)
        .subtitle(&artist.note)
        .use_markup(false)
        .activatable(true)
        .tooltip_text("Show the albums MusicBrainz lists for this artist")
        .build();
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    let artist = artist.clone();
    row.connect_activated(move |_| open(artist.clone()));
    row
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Songs,
    Albums,
    Artists,
}

/// The "Ask flacli" page: name the music as `flacli get` takes it, one item a line.
fn ask_page(window: &EuphonicaWindow) -> gtk::Box {
    let intro = gtk::Label::builder()
        .label("Name the music as flacli takes it, one item a line. flacli resolves each on MusicBrainz, skips what the library holds and fetches the rest from Soulseek, confident matches at once; doubtful ones wait in Incoming for review.")
        .wrap(true)
        .xalign(0.0)
        .css_classes(["dim-label"])
        .build();
    let forms = gtk::Label::builder()
        .label("Artist - Title\nArtist - Album (album)\nalbum: Artist - Album")
        .xalign(0.0)
        .selectable(true)
        .css_classes(["monospace", "dim-label"])
        .build();
    let text = gtk::TextView::builder()
        .monospace(true)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .accepts_tab(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    let frame = gtk::ScrolledWindow::builder()
        .child(&text)
        .min_content_height(160)
        .vexpand(true)
        .css_classes(["card"])
        .build();
    let get_btn = gtk::Button::builder()
        .label("Get these")
        .halign(gtk::Align::End)
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    text.buffer().connect_changed(clone!(
        #[weak]
        get_btn,
        move |buffer| {
            let (start, end) = buffer.bounds();
            let has_line = buffer.text(&start, &end, false).lines().any(|l| !l.trim().is_empty());
            get_btn.set_sensitive(has_line);
        }
    ));
    get_btn.connect_clicked(clone!(
        #[weak]
        window,
        #[weak]
        text,
        move |_| {
            let buffer = text.buffer();
            let (start, end) = buffer.bounds();
            let items: Vec<String> = buffer
                .text(&start, &end, false)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            if items.is_empty() {
                return;
            }
            let label = format!("{} item{}", items.len(), if items.len() == 1 { "" } else { "s" });
            buffer.set_text("");
            fetch(&window, items, label);
        }
    ));
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    content.append(&intro);
    content.append(&forms);
    content.append(&frame);
    content.append(&get_btn);
    let clamp = adw::Clamp::builder().maximum_size(900).child(&content).build();
    let outer = gtk::Box::builder().orientation(gtk::Orientation::Vertical).vexpand(true).build();
    outer.append(&clamp);
    outer
}

/// The search panel: entry, the four pages, and the basket bar. Lives in the Get view.
pub struct Finder {
    /// The whole panel, to place in a view.
    pub root: gtk::Box,
    /// The page switcher, for a header bar's title slot.
    pub switcher: adw::ViewSwitcher,
    entry: gtk::SearchEntry,
    view: adw::ViewStack,
    search: Rc<dyn Fn(String)>,
}

impl Finder {
    pub fn new(window: &EuphonicaWindow) -> Rc<Self> {
        let basket: Basket = Rc::new(RefCell::new(BTreeMap::new()));
        let checks: Checks = Rc::new(RefCell::new(Vec::new()));
        let counter = gtk::Label::builder()
            .label(describe_picks(0, 0))
            .xalign(0.0)
            .hexpand(true)
            .css_classes(["dim-label"])
            .build();
        let get_btn = gtk::Button::builder()
            .label("Get")
            .css_classes(["suggested-action"])
            .sensitive(false)
            .tooltip_text("Hand the ticked songs and albums to flacli")
            .build();
        let changed: Changed = Rc::new(clone!(
            #[strong]
            basket,
            #[weak]
            counter,
            #[weak]
            get_btn,
            move || {
                let picks = basket.borrow();
                let songs = picks.values().filter(|p| matches!(p, Pick::Song { .. })).count();
                counter.set_label(&describe_picks(songs, picks.len() - songs));
                get_btn.set_sensitive(!picks.is_empty());
            }
        ));
        let ctx = Ctx {
            basket: basket.clone(),
            changed,
            checks: checks.clone(),
        };

        let songs = Page::new();
        let albums = Page::new();
        let artists = Page::new();
        let view = adw::ViewStack::new();
        view.add_titled(&songs.stack, Some(SONGS), "Songs");
        view.add_titled(&albums.stack, Some(ALBUMS), "Albums");
        view.add_titled(&artists.stack, Some(ARTISTS), "Artists");
        view.add_titled(&ask_page(window), Some(ASK), "By name");
        let switcher = adw::ViewSwitcher::builder()
            .stack(&view)
            .policy(adw::ViewSwitcherPolicy::Wide)
            .build();

        let entry = gtk::SearchEntry::builder()
            .placeholder_text("Song, album or artist · “Artist - Title” for one song")
            .hexpand(true)
            .build();
        let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
        let search_btn = gtk::Button::builder().label("Search").build();
        let search_line = gtk::Box::builder()
            .spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        search_line.append(&entry);
        search_line.append(&spinner);
        search_line.append(&search_btn);
        // The search line has no place on the Ask page.
        view.connect_visible_child_name_notify(clone!(
            #[weak]
            search_line,
            move |view| search_line.set_visible(view.visible_child_name().as_deref() != Some(ASK))
        ));

        // Opening an artist lists their studio albums and EPs on the Albums page.
        let open_artist: Rc<dyn Fn(FoundArtist)> = Rc::new(clone!(
            #[strong]
            albums,
            #[weak]
            view,
            #[weak]
            spinner,
            #[strong]
            ctx,
            move |artist: FoundArtist| {
                albums.clear();
                albums.set_status(&format!("Asking MusicBrainz for the albums of {}…", artist.name));
                view.set_visible_child_name(ALBUMS);
                spinner.set_visible(true);
                glib::spawn_future_local(clone!(
                    #[strong]
                    albums,
                    #[weak]
                    spinner,
                    #[strong]
                    ctx,
                    async move {
                        let mbid = artist.mbid.clone();
                        let groups = gio::spawn_blocking(move || browse_release_groups(&mbid)).await;
                        spinner.set_visible(false);
                        let groups = match groups {
                            Ok(Ok(groups)) => groups,
                            Ok(Err(e)) => {
                                albums.set_status(&format!("MusicBrainz did not answer: {e}"));
                                return;
                            }
                            Err(_) => return,
                        };
                        if groups.is_empty() {
                            albums.set_status(&format!("MusicBrainz lists no studio album or EP for {}.", artist.name));
                            return;
                        }
                        albums.clear();
                        albums.set_heading(Some(&format!("Albums and EPs by {}, newest first", artist.name)));
                        for group in groups {
                            let album = FoundAlbum {
                                mbid: group.mbid.clone(),
                                title: group.title.clone(),
                                artist: artist.name.clone(),
                                kind: group.kind.to_owned(),
                                year: group.year.clone(),
                            };
                            albums.list.append(&album_row(&album, &ctx));
                        }
                        albums.show_list();
                    }
                ));
            }
        ));

        // One search fills the three pages in turn, the visible one first, a second apart as
        // MusicBrainz asks. A newer search makes an older one's late results fall on the floor.
        let generation = Rc::new(Cell::new(0u32));
        let search: Rc<dyn Fn(String)> = Rc::new(clone!(
            #[strong]
            songs,
            #[strong]
            albums,
            #[strong]
            artists,
            #[weak]
            view,
            #[weak]
            spinner,
            #[strong]
            ctx,
            #[strong]
            open_artist,
            #[strong]
            generation,
            move |term: String| {
                let term = term.trim().to_owned();
                if term.is_empty() {
                    return;
                }
                generation.set(generation.get().wrapping_add(1));
                let this_search = generation.get();
                for page in [&songs, &albums, &artists] {
                    page.clear();
                    page.set_status(&format!("Searching MusicBrainz for “{term}”…"));
                }
                spinner.set_visible(true);
                let order = match view.visible_child_name().as_deref() {
                    Some(ALBUMS) => [Kind::Albums, Kind::Songs, Kind::Artists],
                    Some(ARTISTS) => [Kind::Artists, Kind::Songs, Kind::Albums],
                    Some(ASK) => {
                        view.set_visible_child_name(SONGS);
                        [Kind::Songs, Kind::Albums, Kind::Artists]
                    }
                    _ => [Kind::Songs, Kind::Albums, Kind::Artists],
                };
                glib::spawn_future_local(clone!(
                    #[strong]
                    songs,
                    #[strong]
                    albums,
                    #[strong]
                    artists,
                    #[weak]
                    spinner,
                    #[strong]
                    ctx,
                    #[strong]
                    open_artist,
                    #[strong]
                    generation,
                    async move {
                        for (step, kind) in order.into_iter().enumerate() {
                            if step > 0 {
                                glib::timeout_future(PAUSE).await;
                            }
                            if generation.get() != this_search {
                                return;
                            }
                            let t = term.clone();
                            match kind {
                                Kind::Songs => {
                                    let found = gio::spawn_blocking(move || search_songs(&t)).await;
                                    if generation.get() != this_search {
                                        return;
                                    }
                                    match found {
                                        Ok(Ok(list)) if list.is_empty() => songs.set_status(&format!("No song on MusicBrainz matches “{term}”.")),
                                        Ok(Ok(list)) => {
                                            songs.clear();
                                            for song in &list {
                                                songs.list.append(&song_row(song, &ctx));
                                            }
                                            songs.show_list();
                                        }
                                        Ok(Err(e)) => songs.set_status(&format!("MusicBrainz did not answer: {e}")),
                                        Err(_) => return,
                                    }
                                }
                                Kind::Albums => {
                                    let found = gio::spawn_blocking(move || search_albums(&t)).await;
                                    if generation.get() != this_search {
                                        return;
                                    }
                                    match found {
                                        Ok(Ok(list)) if list.is_empty() => albums.set_status(&format!("No album on MusicBrainz matches “{term}”.")),
                                        Ok(Ok(list)) => {
                                            albums.clear();
                                            for album in &list {
                                                albums.list.append(&album_row(album, &ctx));
                                            }
                                            albums.show_list();
                                        }
                                        Ok(Err(e)) => albums.set_status(&format!("MusicBrainz did not answer: {e}")),
                                        Err(_) => return,
                                    }
                                }
                                Kind::Artists => {
                                    let found = gio::spawn_blocking(move || search_artists(&t)).await;
                                    if generation.get() != this_search {
                                        return;
                                    }
                                    match found {
                                        Ok(Ok(list)) if list.is_empty() => artists.set_status(&format!("No artist on MusicBrainz matches “{term}”.")),
                                        Ok(Ok(list)) => {
                                            artists.clear();
                                            for artist in &list {
                                                artists.list.append(&artist_row(artist, open_artist.clone()));
                                            }
                                            artists.show_list();
                                        }
                                        Ok(Err(e)) => artists.set_status(&format!("MusicBrainz did not answer: {e}")),
                                        Err(_) => return,
                                    }
                                }
                            }
                        }
                        if generation.get() == this_search {
                            spinner.set_visible(false);
                        }
                    }
                ));
            }
        ));
        entry.connect_activate(clone!(
            #[strong]
            search,
            move |entry| search(entry.text().to_string())
        ));
        search_btn.connect_clicked(clone!(
            #[strong]
            search,
            #[weak]
            entry,
            move |_| search(entry.text().to_string())
        ));

        let bottom = gtk::Box::builder()
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();
        bottom.append(&counter);
        bottom.append(&get_btn);
        // The basket bar belongs to the search pages; the Ask page has its own button.
        view.connect_visible_child_name_notify(clone!(
            #[weak]
            bottom,
            move |view| bottom.set_visible(view.visible_child_name().as_deref() != Some(ASK))
        ));

        // Show exactly what goes to flacli before it goes: a whole album is a whole folder.
        get_btn.connect_clicked(clone!(
            #[weak]
            window,
            #[strong]
            basket,
            #[strong]
            checks,
            move |_| {
                let (label, lines, items) = {
                    let picks = basket.borrow();
                    if picks.is_empty() {
                        return;
                    }
                    let songs = picks.values().filter(|p| matches!(p, Pick::Song { .. })).count();
                    (
                        describe_picks(songs, picks.len() - songs),
                        picks.values().map(Pick::describe).collect::<Vec<String>>(),
                        picks.values().map(Pick::item).collect::<Vec<String>>(),
                    )
                };
                let confirm = adw::AlertDialog::builder()
                    .heading(format!("Get {label}?"))
                    .body(format!(
                        "{}\n\nflacli searches Soulseek for each line, fetches the confident matches and files them as Artist/Album/NN - Title. Doubtful ones wait in Incoming for review.",
                        lines.join("\n")
                    ))
                    .build();
                confirm.add_response("back", "_Back");
                confirm.add_response("get", "_Get");
                confirm.set_response_appearance("get", adw::ResponseAppearance::Suggested);
                confirm.set_default_response(Some("get"));
                confirm.set_close_response("back");
                glib::spawn_future_local(clone!(
                    #[weak]
                    window,
                    #[strong]
                    checks,
                    async move {
                        if confirm.choose_future(Some(&window)).await != "get" {
                            return;
                        }
                        // Untick everything; each tick's own handler empties the basket.
                        let live: Vec<gtk::CheckButton> = checks.borrow().iter().filter_map(|c| c.upgrade()).collect();
                        for check in live {
                            check.set_active(false);
                        }
                        checks.borrow_mut().retain(|c| c.upgrade().is_some());
                        fetch(&window, items, label);
                    }
                ));
            }
        ));

        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        root.append(&search_line);
        root.append(&view);
        root.append(&bottom);

        Rc::new(Self {
            root,
            switcher,
            entry,
            view,
            search,
        })
    }

    /// Put the term in the entry and run it.
    pub fn search_for(&self, term: &str) {
        let term = term.trim();
        if term.is_empty() {
            return;
        }
        if self.view.visible_child_name().as_deref() == Some(ASK) {
            self.view.set_visible_child_name(SONGS);
        }
        self.entry.set_text(term);
        (self.search)(term.to_owned());
    }

    pub fn focus_entry(&self) {
        self.entry.grab_focus();
    }
}

/// Open the Get view on the term, or on what it last showed. Called from the app action, the
/// album view's empty-search line, and anywhere else that used to open a dialog.
pub fn get_music(window: &EuphonicaWindow, term: Option<&str>) {
    let ctl = flacli();
    if !ctl.state().available() {
        window.show_dialog(
            "flacli is not set up",
            "Get hands what you tick to flacli, which is not usable from here. Preferences → Integrations → flacli says what is missing.",
        );
        return;
    }
    if !musicbrainz_enabled() {
        window.show_dialog(
            "MusicBrainz is switched off",
            "Get asks MusicBrainz for the names of songs, albums and artists. Switch it on under Preferences → Metadata.",
        );
        return;
    }
    window.show_get_view(term);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_are_described_in_words() {
        assert_eq!(describe_picks(0, 0), "Nothing ticked yet");
        assert_eq!(describe_picks(1, 0), "1 song");
        assert_eq!(describe_picks(0, 2), "2 albums");
        assert_eq!(describe_picks(2, 1), "2 songs and 1 album");
    }

    #[test]
    fn picks_describe_albums_as_whole() {
        let album = Pick::Album { artist: "Black Box Recorder".into(), title: "England Made Me".into() };
        assert_eq!(album.describe(), "Black Box Recorder – England Made Me (whole album)");
        let song = Pick::Song { artist: String::new(), title: "Royals".into(), album: String::new() };
        assert_eq!(song.describe(), "Royals");
    }

    #[test]
    fn items_are_json_for_flacli() {
        let song = Pick::Song {
            artist: "Black Box Recorder".into(),
            title: "Child Psychology".into(),
            album: "England Made Me".into(),
        };
        let parsed: serde_json::Value = serde_json::from_str(&song.item()).unwrap();
        assert_eq!(parsed["kind"], "track");
        assert_eq!(parsed["title"], "Child Psychology");
        let album = Pick::Album {
            artist: "Black Box Recorder".into(),
            title: "The Facts of Life".into(),
        };
        let parsed: serde_json::Value = serde_json::from_str(&album.item()).unwrap();
        assert_eq!(parsed["kind"], "album");
        assert_eq!(parsed["album"], "The Facts of Life");
    }
}
