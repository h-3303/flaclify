//! The write actions of Tier 2, as flows the views call: fetch a named release or search term,
//! import a playlist from a service, queue after the totals, review doubtful matches, cancel.
//!
//! The shell's rules hold here. Naming the music is the yes, so `fetch` asks nothing further.
//! A playlist is never queued until the totals have been seen, so `queue` shows them first.
//! Every flow checks the Nicotine+ bridge before offering a fetch (issue #8).

use adw::prelude::*;
use gtk::{glib, glib::clone};
use std::{cell::Cell, rc::Rc};

use super::controller::{PlaylistStatus, ReviewTrack, SAFE_CONFIDENCE, flacli};
use crate::window::EuphonicaWindow;

fn bridge_missing(window: &EuphonicaWindow) {
    window.show_dialog(
        "Nicotine+ is not reachable",
        "flacli fetches through Nicotine+ with its MCP Bridge plugin, and it does not answer. Start Nicotine+ with the plugin enabled; `flacli doctor` says what is missing.",
    );
}

fn failed(window: &EuphonicaWindow, heading: &str, error: impl std::fmt::Display) {
    window.show_dialog(heading, &error.to_string());
}

/// `flacli get` for one or more items ("Artist - Album (album)", "Artist - Title", or a bare
/// search term). `label` is what the toast names.
pub fn fetch(window: &EuphonicaWindow, items: Vec<String>, label: String) {
    glib::spawn_future_local(clone!(
        #[weak]
        window,
        async move {
            let flacli = flacli();
            if !flacli.ensure_bridge().await {
                bridge_missing(&window);
                return;
            }
            window.send_simple_toast(&format!("Asking flacli for {label}…"), 3);
            match flacli.get(&items).await {
                Ok(result) => window.send_simple_toast(&result.summary(), 8),
                Err(e) => failed(&window, "flacli could not fetch", e),
            }
        }
    ));
}

/// Paste a TIDAL, Deezer or YouTube Music link; flacli imports and matches it in the background.
/// The Incoming section then shows it; queueing waits for the totals.
pub fn import_from_service(window: &EuphonicaWindow) {
    let entry = gtk::Entry::builder()
        .placeholder_text("https://tidal.com/playlist/… · deezer.com/… · music.youtube.com/…")
        .activates_default(true)
        .build();
    let dialog = adw::AlertDialog::builder()
        .heading("Import a playlist")
        .body("Paste a share link from TIDAL, Deezer or YouTube Music. flacli imports it, resolves it against MusicBrainz, notes what the library already has and matches the rest on Soulseek in the background. Nothing is queued until the totals have been shown.")
        .extra_child(&entry)
        .build();
    dialog.add_response("cancel", "_Cancel");
    dialog.add_response("import", "_Import");
    dialog.set_response_appearance("import", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("import"));
    dialog.set_close_response("cancel");
    dialog.choose(
        Some(window),
        gtk::gio::Cancellable::NONE,
        clone!(
            #[weak]
            window,
            #[strong]
            entry,
            move |response| {
                if response != "import" {
                    return;
                }
                let target = entry.text().trim().to_owned();
                if !(target.starts_with("http://") || target.starts_with("https://")) {
                    window.show_dialog("Not a link", "Paste the playlist's share link, starting with https://.");
                    return;
                }
                glib::spawn_future_local(clone!(
                    #[weak]
                    window,
                    async move {
                        let flacli = flacli();
                        if !flacli.ensure_bridge().await {
                            bridge_missing(&window);
                            return;
                        }
                        window.send_simple_toast("Importing… flacli is reading the playlist", 4);
                        match flacli.sync(&target).await {
                            Ok(result) => {
                                let name = result
                                    .imported
                                    .first()
                                    .map(|p| p.name.clone())
                                    .unwrap_or_else(|| "the playlist".to_owned());
                                let text = if result.to_fetch > 0 {
                                    format!("Imported {name}: matching {} tracks in the background. Incoming shows the totals when it is done.", result.to_fetch)
                                } else if result.note.is_empty() {
                                    format!("Imported {name}.")
                                } else {
                                    format!("Imported {name}: {}", result.note)
                                };
                                window.send_simple_toast(&text, 8);
                            }
                            Err(e) => failed(&window, "flacli could not import the playlist", e),
                        }
                    }
                ));
            }
        ),
    );
}

/// Show what `flacli queue` would download, and queue it on the yes. Candidates at or above the
/// safe confidence are approved first, as `flacli queue --min-confidence` does; doubtful ones
/// stay for `review`.
pub fn queue(window: &EuphonicaWindow, playlist: PlaylistStatus) {
    glib::spawn_future_local(clone!(
        #[weak]
        window,
        async move {
            let flacli = flacli();
            if !flacli.ensure_bridge().await {
                bridge_missing(&window);
                return;
            }
            let min_confidence = if playlist.count("candidates") > 0 {
                Some(SAFE_CONFIDENCE)
            } else {
                None
            };
            let totals = match flacli.queue_totals(playlist.playlist_id, min_confidence).await {
                Ok(totals) => totals,
                Err(e) => {
                    failed(&window, "flacli could not prepare the queue", e);
                    return;
                }
            };
            if totals.tracks == 0 {
                let doubtful = playlist.count("candidates");
                window.show_dialog(
                    "Nothing to queue",
                    &if doubtful > 0 {
                        format!("No candidate reached {SAFE_CONFIDENCE:.2}; {doubtful} need a decision. Use Review.")
                    } else {
                        format!("No approved track is waiting. {}", playlist.next)
                    },
                );
                flacli.refresh();
                return;
            }
            let dialog = adw::AlertDialog::builder()
                .heading(format!("Queue {} track{} for “{}”?", totals.tracks, if totals.tracks == 1 { "" } else { "s" }, playlist.name))
                .body(format!(
                    "{} flacli queues them in Nicotine+ and files each finished track as Artist/Album/NN - Title.",
                    totals.describe()
                ))
                .build();
            dialog.add_response("cancel", "_Cancel");
            dialog.add_response("queue", "_Queue");
            dialog.set_response_appearance("queue", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("queue"));
            dialog.set_close_response("cancel");
            let response = dialog.choose_future(Some(&window)).await;
            if response != "queue" {
                flacli.refresh();
                return;
            }
            match flacli.queue_confirmed(playlist.playlist_id).await {
                Ok(done) => {
                    let errors = done.error_count();
                    let text = if errors > 0 {
                        format!("Queued {} from “{}”, {errors} could not be queued.", done.queued_count(), playlist.name)
                    } else {
                        format!("Queued {} track{} from “{}”. Incoming follows the downloads.", totals.tracks, if totals.tracks == 1 { "" } else { "s" }, playlist.name)
                    };
                    window.send_simple_toast(&text, 8);
                }
                Err(e) => failed(&window, "flacli could not queue", e),
            }
        }
    ));
}

/// Stop the running job of a playlist.
pub fn cancel_job(window: &EuphonicaWindow, playlist: PlaylistStatus) {
    glib::spawn_future_local(clone!(
        #[weak]
        window,
        async move {
            match flacli().cancel(playlist.playlist_id).await {
                Ok(()) => window.send_simple_toast(&format!("Stopped the job for “{}”", playlist.name), 4),
                Err(e) => failed(&window, "flacli could not stop the job", e),
            }
        }
    ));
}

/// One row of the review dialog: the track, its candidates to pick from, and the why.
fn review_row(
    window: &EuphonicaWindow,
    playlist_id: u32,
    track: &ReviewTrack,
    remaining: Rc<Cell<u32>>,
    counter: gtk::Label,
) -> adw::ActionRow {
    let best = track.candidates.first();
    let row = adw::ActionRow::builder()
        .title(track.name())
        .subtitle(match best {
            Some(candidate) => format!("{}\nwhy: {}", candidate.describe(), candidate.why()),
            None => "no candidate".to_owned(),
        })
        .use_markup(false)
        .activatable(false)
        .build();
    let tooltip = |candidate: Option<&super::controller::Candidate>| {
        let mut lines: Vec<String> = Vec::new();
        if let Some(album) = track.album.as_deref().filter(|a| !a.is_empty()) {
            lines.push(format!("Album: {album}"));
        }
        if let Some(candidate) = candidate {
            let location = candidate.location();
            if !location.is_empty() {
                lines.push(format!("On {}'s share: {location}", candidate.user));
            }
        }
        lines.join("\n")
    };
    let initial_tip = tooltip(best);
    row.set_tooltip_text(Some(initial_tip.as_str()).filter(|t| !t.is_empty()));

    // Which candidate to take: the best by default, the others by their own line.
    let choices = gtk::StringList::new(
        &track
            .candidates
            .iter()
            .map(|c| c.describe())
            .collect::<Vec<String>>()
            .iter()
            .map(String::as_str)
            .collect::<Vec<&str>>(),
    );
    let picker = gtk::DropDown::builder()
        .model(&choices)
        .valign(gtk::Align::Center)
        .visible(track.candidates.len() > 1)
        .tooltip_text("Which candidate to take")
        .build();
    picker.connect_selected_notify(clone!(
        #[weak]
        row,
        #[strong(rename_to = candidates)]
        track.candidates,
        move |picker| {
            if let Some(candidate) = candidates.get(picker.selected() as usize) {
                row.set_subtitle(&format!("{}\nwhy: {}", candidate.describe(), candidate.why()));
                let location = candidate.location();
                if !location.is_empty() {
                    row.set_tooltip_text(Some(&format!("On {}'s share: {location}", candidate.user)));
                }
            }
        }
    ));

    let approve = gtk::Button::builder()
        .label("Approve")
        .valign(gtk::Align::Center)
        .css_classes(["suggested-action"])
        .sensitive(best.is_some())
        .build();
    let skip = gtk::Button::builder()
        .label("Skip")
        .valign(gtk::Align::Center)
        .build();
    let buttons = gtk::Box::builder()
        .spacing(6)
        .valign(gtk::Align::Center)
        .build();
    buttons.append(&picker);
    buttons.append(&approve);
    buttons.append(&skip);
    row.add_suffix(&buttons);

    let track_id = track.track_id;
    let settle = move |row: &adw::ActionRow, remaining: &Rc<Cell<u32>>, counter: &gtk::Label, verdict: &str| {
        row.set_sensitive(false);
        row.set_subtitle(verdict);
        remaining.set(remaining.get().saturating_sub(1));
        counter.set_label(&format!("{} to decide", remaining.get()));
    };
    approve.connect_clicked(clone!(
        #[weak]
        window,
        #[weak]
        row,
        #[weak]
        picker,
        #[weak]
        buttons,
        #[strong]
        remaining,
        #[strong]
        counter,
        move |_| {
            buttons.set_sensitive(false);
            let candidate = picker.selected() as usize;
            glib::spawn_future_local(clone!(
                #[weak]
                window,
                #[weak]
                row,
                #[weak]
                buttons,
                #[strong]
                remaining,
                #[strong]
                counter,
                async move {
                    match flacli().approve(playlist_id, &[track_id], candidate).await {
                        Ok(()) => settle(&row, &remaining, &counter, "approved · queue it from Incoming"),
                        Err(e) => {
                            buttons.set_sensitive(true);
                            failed(&window, "flacli could not approve", e);
                        }
                    }
                }
            ));
        }
    ));
    skip.connect_clicked(clone!(
        #[weak]
        window,
        #[weak]
        row,
        #[weak]
        buttons,
        #[strong]
        remaining,
        #[strong]
        counter,
        move |_| {
            buttons.set_sensitive(false);
            glib::spawn_future_local(clone!(
                #[weak]
                window,
                #[weak]
                row,
                #[weak]
                buttons,
                #[strong]
                remaining,
                #[strong]
                counter,
                async move {
                    match flacli().skip(playlist_id, &[track_id]).await {
                        Ok(()) => settle(&row, &remaining, &counter, "skipped"),
                        Err(e) => {
                            buttons.set_sensitive(true);
                            failed(&window, "flacli could not skip", e);
                        }
                    }
                }
            ));
        }
    ));
    row
}

/// The doubtful matches of a playlist as a dialog: approve or skip per track, with the why
/// flacli computed, then queue the approved ones from the dialog's own button.
pub fn review(window: &EuphonicaWindow, playlist: PlaylistStatus) {
    glib::spawn_future_local(clone!(
        #[weak]
        window,
        async move {
            let flacli = flacli();
            let review = match flacli.review(playlist.playlist_id).await {
                Ok(review) => review,
                Err(e) => {
                    failed(&window, "flacli could not list the candidates", e);
                    return;
                }
            };
            if review.tracks.is_empty() {
                window.show_dialog("Nothing to review", &format!("No track of “{}” is waiting on a decision.", playlist.name));
                flacli.refresh();
                return;
            }

            let remaining = Rc::new(Cell::new(review.total.max(review.tracks.len() as u32)));
            let counter = gtk::Label::builder()
                .label(format!("{} to decide", remaining.get()))
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["dim-label"])
                .build();
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["boxed-list"])
                .build();
            for track in review.tracks.iter() {
                list.append(&review_row(&window, playlist.playlist_id, track, remaining.clone(), counter.clone()));
            }
            let intro = gtk::Label::builder()
                .label(format!(
                    "Each line is flacli's best Soulseek candidate for a track of “{}”, with the agreement it found on title, artist, album and duration. Confidence at or above {SAFE_CONFIDENCE:.2} is normally safe; read the why below that.",
                    playlist.name
                ))
                .wrap(true)
                .xalign(0.0)
                .css_classes(["caption", "dim-label"])
                .build();
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(12)
                .margin_top(6)
                .margin_bottom(12)
                .margin_start(12)
                .margin_end(12)
                .build();
            content.append(&intro);
            content.append(&list);
            let clamp = adw::Clamp::builder().maximum_size(900).child(&content).build();
            let scroller = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&clamp)
                .build();

            let queue_btn = gtk::Button::builder()
                .label("Queue approved…")
                .css_classes(["suggested-action"])
                .build();
            let bottom = gtk::Box::builder()
                .spacing(12)
                .margin_top(6)
                .margin_bottom(6)
                .margin_start(12)
                .margin_end(12)
                .build();
            bottom.append(&counter);
            bottom.append(&queue_btn);

            let toolbar = adw::ToolbarView::new();
            toolbar.add_top_bar(&adw::HeaderBar::new());
            toolbar.add_bottom_bar(&bottom);
            toolbar.set_content(Some(&scroller));
            let dialog = adw::Dialog::builder()
                .title(format!("Review · {}", playlist.name))
                .content_width(820)
                .content_height(600)
                .child(&toolbar)
                .build();
            queue_btn.connect_clicked(clone!(
                #[weak]
                window,
                #[weak]
                dialog,
                #[strong]
                playlist,
                move |_| {
                    dialog.close();
                    queue(&window, playlist.clone());
                }
            ));
            dialog.connect_closed(move |_| flacli.refresh());
            dialog.present(Some(&window));
        }
    ));
}
