//! The Tidy view: the library as flacli keeps it. One page runs `flacli tidy` (a dry run first,
//! then Apply on the yes), files new arrivals, and fills what the player shows but the library
//! lacks: artist bios and album wikis, artist pictures, album covers. Everything here is a
//! flacli command; the view shows the counts and the results.

use adw::prelude::*;
use gtk::glib;
use serde_json::Value;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use super::{
    controller::{args, run},
    get_view::show_sidebar_button,
};
use crate::window::EuphonicaWindow;

/// Entries one Fill looks up. Wikis take about four requests each at MusicBrainz's one a
/// second, so a run stays a few minutes at most; Fill again for the rest.
const FILL_LIMIT: u32 = 25;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Content {
    Wiki,
    Avatar,
    Cover,
}

impl Content {
    fn command(self) -> &'static str {
        match self {
            Content::Wiki => "wiki",
            Content::Avatar => "avatar",
            Content::Cover => "cover",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Content::Wiki => "bios and wikis",
            Content::Avatar => "artist pictures",
            Content::Cover => "album covers",
        }
    }
}

/// A list's length, or a number, or nothing.
fn count(value: &Value, key: &str) -> u64 {
    match value.get(key) {
        Some(Value::Array(items)) => items.len() as u64,
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

fn plural(n: u64, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// What a dry run found, in one line.
fn plan_summary(plan: &Value) -> String {
    let tag_files = plan.get("tag_changes").map(|t| count(t, "files")).unwrap_or(0);
    let questions = plan.get("open_questions").cloned().unwrap_or(Value::Null);
    let mut parts = vec![
        format!("{} in {}", plural(tag_files, "tag change"), plural(count(plan, "files"), "file")),
        plural(count(plan, "moves"), "move"),
        plural(count(plan, "deletions"), "deletion"),
    ];
    let extras = count(plan, "extras_to_file");
    if extras > 0 {
        parts.push(format!("{extras} extras to file"));
    }
    let reports = count(plan, "download_reports");
    if reports > 0 {
        parts.push(format!("{} to shelve", plural(reports, "download report")));
    }
    let mut open: Vec<String> = Vec::new();
    for (key, word) in [
        ("album_title_variants", "album title variant"),
        ("strays", "stray file"),
        ("duplicate_edit_groups", "possible duplicate group"),
        ("path_clashes", "path clash"),
        ("artist_spelling_variants", "artist spelling variant"),
        ("missing_album", "file without an album"),
        ("various_artists_groups", "various-artists group"),
    ] {
        let n = count(&questions, key);
        if n > 0 {
            open.push(plural(n, word));
        }
    }
    let mut line = parts.join(", ");
    if !open.is_empty() {
        line.push_str(". Open questions: ");
        line.push_str(&open.join(", "));
        line.push_str(" (resolved in approved.py)");
    }
    let unreadable = count(plan, "unreadable");
    if unreadable > 0 {
        line.push_str(&format!(". {} unreadable", plural(unreadable, "file")));
    }
    let recent = count(plan, "recent_files");
    if recent > 0 {
        line.push_str(&format!(". {} still being written", plural(recent, "file")));
    }
    line.push('.');
    line
}

fn has_work(plan: &Value) -> bool {
    plan.get("tag_changes").map(|t| count(t, "files")).unwrap_or(0) > 0
        || count(plan, "moves") > 0
        || count(plan, "deletions") > 0
        || count(plan, "extras_to_file") > 0
        || count(plan, "download_reports") > 0
}

/// The album-title merges a plan suggests, one line each.
fn variant_lines(plan: &Value) -> Vec<String> {
    plan.get("open_questions")
        .and_then(|q| q.get("album_title_variants"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|v| {
                    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("?");
                    let files = count(v, "files");
                    format!("{} → {}  ({}, {})", s("variant"), s("canonical"), s("artist"), plural(files, "file"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The deletions of a plan by name, for the confirmation.
fn deletion_lines(plan: &Value) -> Vec<String> {
    plan.get("deletions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|d| {
                    let path = d.get("path").and_then(Value::as_str).unwrap_or("?");
                    match d.get("why").and_then(Value::as_str) {
                        Some(why) => format!("{path} — {why}"),
                        None => path.to_owned(),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The scalar fields of a result, for the log.
fn scalars(value: &Value) -> String {
    let Some(map) = value.as_object() else {
        return value.to_string();
    };
    map.iter()
        .filter_map(|(k, v)| match v {
            Value::Null | Value::Object(_) => None,
            Value::Array(items) => Some(format!("{k}: {}", items.len())),
            Value::String(s) if s.len() > 120 => None,
            other => Some(format!("{k}: {other}")),
        })
        .collect::<Vec<String>>()
        .join(" · ")
}

fn monospace_pane(height: i32) -> (gtk::ScrolledWindow, gtk::TextView) {
    let text = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .monospace(true)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    let pane = gtk::ScrolledWindow::builder()
        .child(&text)
        .min_content_height(height)
        .max_content_height(height)
        .propagate_natural_height(true)
        .css_classes(["card"])
        .build();
    (pane, text)
}

fn suffix_button(label: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .label(label)
        .valign(gtk::Align::Center)
        .tooltip_text(tooltip)
        .build()
}

impl std::fmt::Debug for TidyView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TidyView").field("busy", &self.busy.get()).finish()
    }
}

pub struct TidyView {
    pub widget: adw::ToolbarView,
    window: glib::WeakRef<EuphonicaWindow>,
    busy: Cell<bool>,
    spinner: gtk::Spinner,
    buttons: Vec<gtk::Button>,
    plan: RefCell<Option<Value>>,
    plan_row: adw::ActionRow,
    apply_btn: gtk::Button,
    report_row: adw::ExpanderRow,
    report: gtk::TextView,
    variants_row: adw::ActionRow,
    wiki_row: adw::ActionRow,
    avatar_row: adw::ActionRow,
    cover_row: adw::ActionRow,
    log: gtk::TextView,
    counted: Cell<bool>,
}

impl TidyView {
    pub fn new(window: &EuphonicaWindow) -> Rc<Self> {
        let header = adw::HeaderBar::new();
        header.pack_start(&show_sidebar_button(window));
        header.set_title_widget(Some(&adw::WindowTitle::new("Tidy", "")));
        let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
        header.pack_end(&spinner);
        let refresh_btn = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Count again what is missing")
            .build();
        header.pack_end(&refresh_btn);

        // Library files
        let files_group = adw::PreferencesGroup::builder()
            .title("Library files")
            .description("flacli tidy: tags normalised, lossy duplicates of FLACs dropped, every track filed as Artist/Album/NN - Title. Analyse is a dry run; nothing changes until Apply, and every deletion is shown by name first.")
            .build();
        let analyse_btn = suffix_button("Analyse", "Dry run: write the plan and the report, change nothing");
        let apply_btn = suffix_button("Apply", "Do what the plan says");
        apply_btn.add_css_class("destructive-action");
        apply_btn.set_sensitive(false);
        let plan_row = adw::ActionRow::builder()
            .title("Plan")
            .subtitle("Not analysed yet.")
            .use_markup(false)
            .build();
        plan_row.add_suffix(&analyse_btn);
        plan_row.add_suffix(&apply_btn);
        files_group.add(&plan_row);
        let new_btn = suffix_button("File new", "Only file tracks that arrived by other routes; never deletes");
        let new_row = adw::ActionRow::builder()
            .title("New arrivals")
            .subtitle("Tracks that arrived by other routes than flacli, tagged and filed. Never deletes.")
            .use_markup(false)
            .build();
        new_row.add_suffix(&new_btn);
        files_group.add(&new_row);
        let accept_btn = suffix_button("Accept", "Write these merges into approved.py and plan again; nothing moves until Apply");
        let variants_row = adw::ActionRow::builder()
            .title("Album titles that are variants of another album")
            .subtitle("")
            .use_markup(false)
            .visible(false)
            .build();
        variants_row.add_suffix(&accept_btn);
        files_group.add(&variants_row);
        let (report_pane, report) = monospace_pane(320);
        let report_row = adw::ExpanderRow::builder()
            .title("Report")
            .subtitle("Written by Analyse")
            .use_markup(false)
            .show_enable_switch(false)
            .sensitive(false)
            .build();
        let report_holder = gtk::ListBoxRow::builder().activatable(false).selectable(false).child(&report_pane).build();
        report_row.add_row(&report_holder);
        files_group.add(&report_row);

        // Missing content
        let fill_all_btn = gtk::Button::builder()
            .label("Fill everything")
            .valign(gtk::Align::Center)
            .css_classes(["suggested-action"])
            .tooltip_text("Bios and wikis, then artist pictures, then album covers")
            .build();
        let content_group = adw::PreferencesGroup::builder()
            .title("Missing content")
            .description(format!(
                "What the player shows but the library lacks, kept beside the music and pushed into the player's cache. Each Fill looks up {FILL_LIMIT} entries; Fill again for the rest. Wikis come from Wikipedia where an article exists; the others are left as briefs for an agent to write from the facts flacli gathers."
            ))
            .header_suffix(&fill_all_btn)
            .build();
        let make_row = |title: &str, tooltip: &str| {
            let button = suffix_button("Fill", tooltip);
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle("Not counted yet.")
                .use_markup(false)
                .build();
            row.add_suffix(&button);
            content_group.add(&row);
            (row, button)
        };
        let (wiki_row, wiki_btn) = make_row("Artist bios and album wikis", "Wikipedia's lead paragraph where an article exists");
        let (avatar_row, avatar_btn) = make_row("Artist pictures", "Artist folder, Wikidata portrait, MusicBrainz, Deezer");
        let (cover_row, cover_btn) = make_row("Album covers", "Folder or embedded picture, Cover Art Archive, Deezer, iTunes");

        // Log
        let log_group = adw::PreferencesGroup::builder().title("Log").build();
        let (log_pane, log) = monospace_pane(200);
        log_group.add(&log_pane);

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(24)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        content.append(&files_group);
        content.append(&content_group);
        content.append(&log_group);
        let clamp = adw::Clamp::builder().maximum_size(860).child(&content).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let widget = adw::ToolbarView::new();
        widget.add_top_bar(&header);
        widget.set_content(Some(&scroller));

        let this = Rc::new(Self {
            widget,
            window: window.downgrade(),
            busy: Cell::new(false),
            spinner,
            buttons: vec![
                analyse_btn.clone(),
                apply_btn.clone(),
                new_btn.clone(),
                accept_btn.clone(),
                fill_all_btn.clone(),
                wiki_btn.clone(),
                avatar_btn.clone(),
                cover_btn.clone(),
                refresh_btn.clone(),
            ],
            plan: RefCell::new(None),
            plan_row,
            apply_btn: apply_btn.clone(),
            report_row,
            report,
            variants_row,
            wiki_row,
            avatar_row,
            cover_row,
            log,
            counted: Cell::new(false),
        });

        let hook = |button: &gtk::Button, view: &Rc<Self>, action: fn(Rc<Self>)| {
            let view = Rc::downgrade(view);
            button.connect_clicked(move |_| {
                if let Some(view) = view.upgrade() {
                    action(view);
                }
            });
        };
        hook(&analyse_btn, &this, |v| v.analyse());
        hook(&apply_btn, &this, |v| v.apply());
        hook(&new_btn, &this, |v| v.file_new());
        hook(&accept_btn, &this, |v| v.accept_variants());
        hook(&refresh_btn, &this, |v| v.refresh_counts());
        hook(&fill_all_btn, &this, |v| v.fill_all());
        hook(&wiki_btn, &this, |v| v.fill(Content::Wiki));
        hook(&avatar_btn, &this, |v| v.fill(Content::Avatar));
        hook(&cover_btn, &this, |v| v.fill(Content::Cover));
        this
    }

    /// The view came on screen: count what is missing, the first time and whenever idle.
    pub fn shown(self: &Rc<Self>) {
        if !self.busy.get() {
            self.clone().refresh_counts();
        }
    }

    fn log_line(&self, text: &str) {
        let stamp = glib::DateTime::now_local()
            .ok()
            .and_then(|t| t.format("%H:%M").ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        let buffer = self.log.buffer();
        let mut end = buffer.end_iter();
        buffer.insert(&mut end, &format!("{stamp}  {text}\n"));
        let mark = buffer.create_mark(None, &buffer.end_iter(), false);
        self.log.scroll_mark_onscreen(&mark);
    }

    fn start(&self) -> bool {
        if self.busy.get() {
            return false;
        }
        self.busy.set(true);
        self.spinner.set_visible(true);
        for button in &self.buttons {
            button.set_sensitive(false);
        }
        true
    }

    fn finish(&self) {
        self.busy.set(false);
        self.spinner.set_visible(false);
        for button in &self.buttons {
            button.set_sensitive(true);
        }
        self.apply_btn
            .set_sensitive(self.plan.borrow().as_ref().is_some_and(has_work));
    }

    fn failed(&self, what: &str, error: impl std::fmt::Display) {
        self.log_line(&format!("{what} failed: {error}"));
        if let Some(window) = self.window.upgrade() {
            window.send_simple_toast(&format!("{what} failed: {error}"), 6);
        }
    }

    // Counting

    async fn count_one(&self, kind: Content) {
        let row = match kind {
            Content::Wiki => &self.wiki_row,
            Content::Avatar => &self.avatar_row,
            Content::Cover => &self.cover_row,
        };
        row.set_subtitle("Counting…");
        match run::<Value>(args(&[kind.command(), "missing"])).await {
            Ok(found) => {
                let missing = count(&found, "missing");
                let line = match kind {
                    Content::Wiki => {
                        let artists = count(&found, "artists");
                        let albums = count(&found, "albums");
                        format!("{} without text, of {} and {}.", plural(missing, "entry").replace("entrys", "entries"), plural(artists, "artist"), plural(albums, "album"))
                    }
                    Content::Avatar => format!("{} without a picture, of {}.", plural(missing, "artist"), count(&found, "artists")),
                    Content::Cover => {
                        let bare = count(&found, "tracks_without_picture");
                        let mut line = format!("{} without a cover file, of {}.", plural(missing, "album"), count(&found, "albums"));
                        if bare > 0 {
                            line.push_str(&format!(" {} without an embedded picture.", plural(bare, "track")));
                        }
                        line
                    }
                };
                row.set_subtitle(&line);
            }
            Err(e) => row.set_subtitle(&format!("Could not count: {e}")),
        }
    }

    pub fn refresh_counts(self: Rc<Self>) {
        if !self.start() {
            return;
        }
        glib::spawn_future_local(async move {
            for kind in [Content::Wiki, Content::Avatar, Content::Cover] {
                self.count_one(kind).await;
            }
            self.counted.set(true);
            self.finish();
        });
    }

    // Tidy

    /// A dry run's result onto the page: the summary line, the merges offered, the report.
    fn show_plan(&self, plan: Value) {
        let summary = plan_summary(&plan);
        self.plan_row.set_subtitle(&summary);
        self.log_line(&format!("Analysed: {summary}"));
        let variants = variant_lines(&plan);
        self.variants_row.set_visible(!variants.is_empty());
        self.variants_row.set_subtitle(&variants.join("\n"));
        let report_path = plan.get("report_path").and_then(Value::as_str).unwrap_or("").to_owned();
        match std::fs::read_to_string(&report_path) {
            Ok(text) if !report_path.is_empty() => {
                self.report.buffer().set_text(&text);
                self.report_row.set_subtitle(&report_path);
                self.report_row.set_sensitive(true);
            }
            _ => {
                self.report_row.set_subtitle("No report written");
                self.report_row.set_sensitive(false);
            }
        }
        *self.plan.borrow_mut() = Some(plan);
    }

    async fn analyse_inner(&self) -> bool {
        self.plan_row.set_subtitle("Reading every file…");
        match run::<Value>(args(&["tidy"])).await {
            Ok(plan) => {
                self.show_plan(plan);
                true
            }
            Err(e) => {
                self.plan_row.set_subtitle(&format!("Could not analyse: {e}"));
                self.failed("Analyse", e);
                *self.plan.borrow_mut() = None;
                false
            }
        }
    }

    pub fn analyse(self: Rc<Self>) {
        if !self.start() {
            return;
        }
        glib::spawn_future_local(async move {
            self.analyse_inner().await;
            self.finish();
        });
    }

    pub fn apply(self: Rc<Self>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let (summary, deletions) = match self.plan.borrow().as_ref() {
            Some(plan) => (plan_summary(plan), deletion_lines(plan)),
            None => return,
        };
        let mut body = summary;
        if !deletions.is_empty() {
            body.push_str("\n\nDeleted, by name:\n");
            body.push_str(&deletions.join("\n"));
        }
        body.push_str("\n\nOpen questions are not touched; resolve them in approved.py and Analyse again.");
        let confirm = adw::AlertDialog::builder()
            .heading("Apply the tidy plan?")
            .body(body)
            .build();
        confirm.add_response("back", "_Back");
        confirm.add_response("apply", "_Apply");
        confirm.set_response_appearance("apply", adw::ResponseAppearance::Destructive);
        confirm.set_default_response(Some("back"));
        confirm.set_close_response("back");
        glib::spawn_future_local(async move {
            if confirm.choose_future(Some(&window)).await != "apply" {
                return;
            }
            if !self.start() {
                return;
            }
            self.plan_row.set_subtitle("Applying…");
            match run::<Value>(args(&["tidy", "--apply"])).await {
                Ok(result) => {
                    self.log_line(&format!("Applied: {}", scalars(&result)));
                    window.send_simple_toast("Tidy applied. Analysing again…", 4);
                    self.analyse_inner().await;
                }
                Err(e) => {
                    self.failed("Apply", e);
                    self.plan_row.set_subtitle("Apply failed; see the log.");
                }
            }
            self.finish();
        });
    }

    /// Write the suggested album-title merges into approved.py and plan again.
    fn accept_variants(self: Rc<Self>) {
        if !self.start() {
            return;
        }
        glib::spawn_future_local(async move {
            self.log_line("Accepting the album-title merges into approved.py…");
            match run::<Value>(args(&["tidy", "--accept-album-variants"])).await {
                Ok(plan) => {
                    let accepted = count(&plan, "accepted_album_variants");
                    self.log_line(&format!("Accepted {}; the plan below now carries the moves. Apply does them.", plural(accepted, "merge")));
                    self.show_plan(plan);
                }
                Err(e) => self.failed("Accept", e),
            }
            self.finish();
        });
    }

    pub fn file_new(self: Rc<Self>) {
        if !self.start() {
            return;
        }
        glib::spawn_future_local(async move {
            self.log_line("Filing new arrivals…");
            match run::<Value>(args(&["tidy", "--new"])).await {
                Ok(result) => self.log_line(&format!("Filed: {}", scalars(&result))),
                Err(e) => self.failed("File new", e),
            }
            self.finish();
        });
    }

    // Filling

    async fn fill_inner(&self, kind: Content) {
        self.log_line(&format!("Filling {}…", kind.noun()));
        let limit = FILL_LIMIT.to_string();
        match run::<Value>(args(&[kind.command(), "fill", "--limit", &limit])).await {
            Ok(result) => {
                let filled = count(&result, "filled");
                let errors = count(&result, "errors");
                let remaining = count(&result, "remaining");
                let mut line = format!("Filled {} {}", filled, kind.noun());
                match kind {
                    Content::Wiki => {
                        let to_write = count(&result, "to_write");
                        if to_write > 0 {
                            line.push_str(&format!(", {to_write} without a Wikipedia article left as briefs for an agent"));
                        }
                    }
                    _ => {
                        let not_found = count(&result, "not_found");
                        if not_found > 0 {
                            line.push_str(&format!(", {not_found} not found at any source"));
                        }
                    }
                }
                if errors > 0 {
                    line.push_str(&format!(", {} ", plural(errors, "error")));
                }
                if remaining > 0 {
                    line.push_str(&format!("; {remaining} more to look up, Fill again"));
                }
                line.push('.');
                self.log_line(&line);
            }
            Err(e) => self.failed(&format!("Fill {}", kind.noun()), e),
        }
        self.count_one(kind).await;
    }

    fn fill(self: Rc<Self>, kind: Content) {
        if !self.start() {
            return;
        }
        glib::spawn_future_local(async move {
            self.fill_inner(kind).await;
            self.finish();
        });
    }

    pub fn fill_all(self: Rc<Self>) {
        if !self.start() {
            return;
        }
        glib::spawn_future_local(async move {
            for kind in [Content::Wiki, Content::Avatar, Content::Cover] {
                self.fill_inner(kind).await;
            }
            if let Some(window) = self.window.upgrade() {
                window.send_simple_toast("Fill done. The player shows the new text and pictures when it next opens the artist or album.", 8);
            }
            self.finish();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_summary_reads_the_dry_run() {
        let plan: Value = serde_json::json!({
            "files": 497, "unreadable": [], "recent_files": [],
            "open_questions": {"strays": ["a.md"], "duplicate_edit_groups": 12, "path_clashes": 0,
                               "artist_spelling_variants": 0, "missing_album": 0, "various_artists_groups": []},
            "tag_changes": {"files": 2}, "deletions": [], "moves": 0, "extras_to_file": 0, "download_reports": 0
        });
        assert_eq!(
            plan_summary(&plan),
            "2 tag changes in 497 files, 0 moves, 0 deletions. Open questions: 1 stray file, 12 possible duplicate groups (resolved in approved.py)."
        );
        assert!(has_work(&plan));
        assert!(!has_work(&serde_json::json!({"tag_changes": {"files": 0}, "moves": 0, "deletions": []})));
    }

    #[test]
    fn deletions_are_named() {
        let plan = serde_json::json!({"deletions": [{"path": "x/video rip.mp3", "rule": "R10", "why": "flac beats lossy"}]});
        assert_eq!(deletion_lines(&plan), vec!["x/video rip.mp3 — flac beats lossy"]);
    }
}
