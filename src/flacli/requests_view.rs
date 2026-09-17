//! The Requests page: what has been asked of flacli and not yet landed. One row per track of
//! flacli's Requests playlist that is not on disk, with where it stands; a track leaves the list
//! the moment it is filed, and then it is simply in the library. Requests is never stored as a
//! playlist in MPD.

use adw::prelude::*;
use gtk::{glib, glib::clone};
use std::{cell::Cell, rc::Rc};

use super::{
    actions::skip_rest,
    controller::{MissingTrack, PlaylistStatus, flacli, status_label},
    get_view::show_sidebar_button,
};
use crate::window::EuphonicaWindow;

const EMPTY: &str = "Nothing requested. Get adds songs and albums here; each one leaves the list as it lands in the library.";

impl std::fmt::Debug for RequestsView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RequestsView")
    }
}

pub struct RequestsView {
    pub widget: adw::ToolbarView,
    window: glib::WeakRef<EuphonicaWindow>,
    title: adw::WindowTitle,
    stack: gtk::Stack,
    status: adw::StatusPage,
    list: gtk::ListBox,
    skip_all: gtk::Button,
    on_screen: Cell<bool>,
}

/// Rows worth showing: everything not on disk, except what the user gave up on.
fn shown(track: &MissingTrack) -> bool {
    track.status != "skipped"
}

fn describe(track: &MissingTrack) -> String {
    let mut line = track.artist.clone().filter(|a| !a.is_empty()).unwrap_or_default();
    if let Some(album) = track.album.as_deref().filter(|a| !a.is_empty()) {
        if !line.is_empty() {
            line.push_str(" · ");
        }
        line.push_str(album);
    }
    line
}

impl RequestsView {
    pub fn new(window: &EuphonicaWindow) -> Rc<Self> {
        let header = adw::HeaderBar::new();
        header.pack_start(&show_sidebar_button(window));
        let title = adw::WindowTitle::new("Requests", "");
        header.set_title_widget(Some(&title));
        let spinner = gtk::Spinner::builder().spinning(true).visible(false).build();
        flacli()
            .state()
            .bind_property("active", &spinner, "visible")
            .sync_create()
            .build();
        header.pack_end(&spinner);
        let refresh = gtk::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Ask flacli again")
            .build();
        refresh.connect_clicked(|_| flacli().refresh());
        header.pack_end(&refresh);
        let skip_all = gtk::Button::builder()
            .label("Skip the rest")
            .tooltip_text("Stop, cancel queued downloads, and give up on everything still missing")
            .visible(false)
            .build();
        header.pack_end(&skip_all);

        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();
        let clamp = adw::Clamp::builder().maximum_size(900).child(&list).build();
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        let status = adw::StatusPage::builder().description(EMPTY).build();
        let stack = gtk::Stack::builder().vexpand(true).build();
        stack.add_named(&status, Some("status"));
        stack.add_named(&scroller, Some("list"));
        stack.set_visible_child_name("status");

        let widget = adw::ToolbarView::new();
        widget.add_top_bar(&header);
        widget.set_content(Some(&stack));

        let this = Rc::new(Self {
            widget,
            window: window.downgrade(),
            title,
            stack,
            status,
            list,
            skip_all,
            on_screen: Cell::new(false),
        });

        this.skip_all.connect_clicked(clone!(
            #[weak(rename_to = view)]
            this,
            move |_| {
                let (Some(window), Some(playlist)) = (view.window.upgrade(), flacli().requests_playlist()) else {
                    return;
                };
                skip_rest(&window, playlist);
            }
        ));

        // Rebuilt from the controller's snapshot on every refresh.
        flacli().state().connect_local(
            "refreshed",
            false,
            clone!(
                #[weak(rename_to = view)]
                this,
                #[upgrade_or]
                None,
                move |_| {
                    view.rebuild();
                    None
                }
            ),
        );
        this.rebuild();
        this
    }

    /// The page came on screen: keep the Requests playlist's detail fresh while it is.
    pub fn shown(&self) {
        self.on_screen.set(true);
        let ctl = flacli();
        match ctl.requests_playlist() {
            Some(playlist) => ctl.watch(Some(playlist.playlist_id)),
            None => ctl.refresh(),
        }
    }

    pub fn hidden(&self) {
        if self.on_screen.replace(false) {
            flacli().watch(None);
        }
    }

    fn rebuild(&self) {
        let ctl = flacli();
        let playlist = ctl.requests_playlist();
        // First sight of the playlist while on screen: its detail is what the rows need.
        if self.on_screen.get() {
            if let Some(p) = playlist.as_ref().filter(|p| !p.detailed) {
                ctl.watch(Some(p.playlist_id));
            }
        }
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let Some(playlist) = playlist else {
            self.title.set_subtitle("");
            self.skip_all.set_visible(false);
            self.status.set_description(Some(EMPTY));
            self.stack.set_visible_child_name("status");
            return;
        };
        self.title.set_subtitle(&playlist.summary());
        let rows: Vec<&MissingTrack> = playlist.missing.iter().filter(|t| shown(t)).collect();
        let remaining: u32 = playlist
            .counts
            .iter()
            .filter(|(status, _)| !matches!(status.as_str(), "done" | "in_library" | "skipped"))
            .map(|(_, n)| *n)
            .sum();
        self.skip_all.set_visible(remaining > 0);
        if rows.is_empty() {
            self.status.set_description(Some(if remaining > 0 && !playlist.detailed {
                "Asking flacli what is still missing…"
            } else {
                EMPTY
            }));
            self.stack.set_visible_child_name("status");
            return;
        }
        for track in rows {
            self.list.append(&self.row(&playlist, track));
        }
        self.stack.set_visible_child_name("list");
    }

    fn row(&self, playlist: &PlaylistStatus, track: &MissingTrack) -> adw::ActionRow {
        let row = adw::ActionRow::builder()
            .title(&track.title)
            .subtitle(describe(track))
            .use_markup(false)
            .build();
        let state = gtk::Label::builder()
            .label(status_label(&track.status))
            .valign(gtk::Align::Center)
            .css_classes(["dim-label", "caption"])
            .build();
        row.add_suffix(&state);
        match track.status.as_str() {
            "searching" | "downloading" => {
                row.add_suffix(&gtk::Spinner::builder().spinning(true).valign(gtk::Align::Center).build());
            }
            "candidates" => {
                let review = gtk::Button::builder()
                    .icon_name("document-edit-symbolic")
                    .tooltip_text("Review the doubtful matches")
                    .valign(gtk::Align::Center)
                    .css_classes(["flat", "circular"])
                    .build();
                let playlist = playlist.clone();
                let window = self.window.clone();
                review.connect_clicked(clone!(
                    #[strong]
                    window,
                    move |_| {
                        if let Some(window) = window.upgrade() {
                            super::actions::review(&window, playlist.clone());
                        }
                    }
                ));
                row.add_suffix(&review);
            }
            "not_found" | "failed" => row.add_css_class("dim-label"),
            _ => {}
        }
        let skip = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Skip this one; a queued download is cancelled")
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let (playlist_id, track_id, title) = (playlist.playlist_id, track.track_id, track.title.clone());
        let window = self.window.clone();
        skip.connect_clicked(clone!(
            #[strong]
            window,
            #[weak]
            row,
            move |button| {
                button.set_sensitive(false);
                row.set_sensitive(false);
                let title = title.clone();
                glib::spawn_future_local(clone!(
                    #[strong]
                    window,
                    #[weak]
                    row,
                    async move {
                        let outcome = flacli().skip(playlist_id, &[track_id]).await;
                        if let (Err(e), Some(window)) = (&outcome, window.upgrade()) {
                            row.set_sensitive(true);
                            window.send_simple_toast(&format!("flacli could not skip “{title}”: {e}"), 6);
                        }
                    }
                ));
            }
        ));
        row.add_suffix(&skip);
        row
    }
}
