use adw::subclass::prelude::*;
use glib::{Properties, clone, closure_local};
use gtk::{CompositeTemplate, glib, prelude::*};
use std::cell::Cell;

use crate::{
    application::EuphonicaApplication,
    cache::Cache,
    client::state::StickersSupportLevel,
    common::{INode, ImageStack, View},
    flacli::{FlacliState, PlaylistStatus, cancel_job, flacli, queue, review, skip_rest},
    utils,
    window::EuphonicaWindow,
};
use std::rc::Rc;

use super::SidebarButton;

/// Build the 16px rounded playlist cover ImageStack used as the prefix of
/// recent playlist buttons in the sidebar
fn playlist_cover_prefix() -> (ImageStack, gtk::Box) {
    let cover = ImageStack::new();
    cover.set_size(16);
    cover.set_is_thumbnail(true);
    let rounded_box = gtk::Box::builder()
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["border-radius-6"])
        .build();
    rounded_box.append(&cover);
    (cover, rounded_box)
}

fn fetch_playlist_cover(cache: &Rc<Cache>, cover: ImageStack, name: &str, is_dynamic: bool) {
    let name = name.to_string();
    cover.show_spinner();
    glib::spawn_future_local(clone!(
        #[strong]
        cache,
        #[strong]
        cover,
        async move {
            match cache.get_playlist_cover(name, is_dynamic, true).await {
                Ok(Some(tex)) => cover.show(&tex),
                Ok(None) => cover.clear(),
                Err(e) => {
                    dbg!(e);
                    cover.clear();
                }
            }
        }
    ));
}

/// One line per flacli playlist in the Incoming section: its name over what is happening to it,
/// and the actions it is waiting on (Tier 2): stop a running job, review doubtful matches, queue.
fn incoming_row(window: &EuphonicaWindow, playlist: &PlaylistStatus) -> (gtk::Box, gtk::Button) {
    let name = gtk::Label::builder()
        .label(&playlist.name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    let summary = gtk::Label::builder()
        .label(playlist.summary())
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["caption", "dim-label"])
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    content.append(&name);
    content.append(&summary);
    let open = gtk::Button::builder()
        .child(&content)
        .hexpand(true)
        .tooltip_text(&playlist.next)
        .css_classes(["flat"])
        .build();

    let row = gtk::Box::builder().spacing(0).build();
    row.append(&open);
    let action = |icon: &str, tip: &str| {
        gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tip)
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build()
    };
    if playlist.is_running() {
        let stop = action("stop-sign-outline-symbolic", "Stop the job");
        stop.connect_clicked(clone!(
            #[weak]
            window,
            #[strong]
            playlist,
            move |_| cancel_job(&window, playlist.clone())
        ));
        row.append(&stop);
    } else {
        if playlist.count("candidates") > 0 {
            let review_btn = action(
                "document-edit-symbolic",
                &format!("Review {} doubtful match(es)", playlist.count("candidates")),
            );
            review_btn.connect_clicked(clone!(
                #[weak]
                window,
                #[strong]
                playlist,
                move |_| review(&window, playlist.clone())
            ));
            row.append(&review_btn);
        }
        if playlist.count("approved") > 0 || playlist.count("candidates") > 0 {
            let queue_btn = action(
                "arrow-pointing-at-line-down-symbolic",
                "Queue the confident matches (shows the totals first)",
            );
            queue_btn.connect_clicked(clone!(
                #[weak]
                window,
                #[strong]
                playlist,
                move |_| queue(&window, playlist.clone())
            ));
            row.append(&queue_btn);
        }
    }
    // Always there: give up on what is still missing.
    let skip_btn = action("window-close-symbolic", "Skip the rest: stop, cancel queued downloads, forget what is not on disk");
    skip_btn.connect_clicked(clone!(
        #[weak]
        window,
        #[strong]
        playlist,
        move |_| skip_rest(&window, playlist.clone())
    ));
    row.append(&skip_btn);
    (row, open)
}

mod imp {
    use super::*;

    #[derive(Debug, Properties, Default, CompositeTemplate)]
    #[properties(wrapper_type = super::Sidebar)]
    #[template(resource = "/io/github/h3303/Flaclify/gtk/sidebar.ui")]
    pub struct Sidebar {
        #[template_child]
        pub recent_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub albums_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub artists_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub folders_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub playlists_section: TemplateChild<gtk::Box>,
        #[template_child]
        pub playlists_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub recent_playlists: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub dyn_playlists_section: TemplateChild<gtk::Box>,
        #[template_child]
        pub dyn_playlists_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub recent_dyn_playlists: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub incoming_section: TemplateChild<gtk::Box>,
        #[template_child]
        pub incoming_spinner: TemplateChild<gtk::Spinner>,
        #[template_child]
        pub incoming_refresh: TemplateChild<gtk::Button>,
        #[template_child]
        pub incoming_list: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub flacli_section: TemplateChild<gtk::Box>,
        #[template_child]
        pub get_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub requests_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub tidy_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub ask_btn: TemplateChild<SidebarButton>,
        #[template_child]
        pub queue_btn: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub queue_len: TemplateChild<gtk::Label>,
        #[property(get, set)]
        pub showing_queue_view: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Sidebar {
        const NAME: &'static str = "EuphonicaSidebar";
        type Type = super::Sidebar;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for Sidebar {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Sidebar {}

    // Trait shared by all boxes
    impl BoxImpl for Sidebar {}
}

glib::wrapper! {
    pub struct Sidebar(ObjectSubclass<imp::Sidebar>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl Default for Sidebar {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl Sidebar {
    pub fn new() -> Self {
        Self::default()
    }

    // Dirty hack to remove the highlight effect on hover
    // (as the items themselves are toggle buttons already, there is no need
    // for the ListBoxRows to do this)
    pub fn hide_highlights(&self) {
        let settings = utils::settings_manager().child("ui");
        let recent_playlists_widget = self.imp().recent_playlists.get();
        let recent_dyn_playlists_widget = self.imp().recent_dyn_playlists.get();
        for idx in 0..settings.uint("recent-playlists-count") {
            if let Some(row) = recent_playlists_widget.row_at_index(idx as i32) {
                row.set_activatable(false);
            }
            if let Some(row) = recent_dyn_playlists_widget.row_at_index(idx as i32) {
                row.set_activatable(false);
            }
        }
    }

    pub fn setup(&self, win: &EuphonicaWindow, app: &EuphonicaApplication) {
        let settings = utils::settings_manager().child("ui");
        
        let stack = win.get_stack();
        let split_view = win.get_split_view();
        let player = app.get_player();
        let library = app.get_library();
        let client_state = app.get_client().get_client_state();
        stack
            .bind_property("visible-child-name", self, "showing-queue-view")
            .transform_to(|_, name: String| Some(name == "queue"))
            .sync_create()
            .build();

        let recent_btn = self.imp().recent_btn.get();
        recent_btn.set_active(true);
        
        // Hook each button to their respective views
        recent_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("recent");
                }
            }
        ));

        self.imp().albums_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("albums");
                }
            }
        ));

        self.imp().artists_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("artists");
                }
            }
        ));

        self.imp().folders_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("folders");
                }
            }
        ));

        // flacli's pages
        self.imp().get_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("get");
                }
            }
        ));
        self.imp().requests_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("requests");
                }
            }
        ));
        self.imp().tidy_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("tidy");
                }
            }
        ));
        self.imp().ask_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("ask");
                }
            }
        ));

        let playlist_view = win.get_playlist_view();
        let playlists = library.playlists();
        let cache = app.get_cache();
        let recent_playlists_model = gtk::SliceListModel::new(
            Some(gtk::SortListModel::new(
                Some(playlists.clone()),
                Some(
                    gtk::StringSorter::builder()
                        .expression(gtk::PropertyExpression::new(
                            INode::static_type(),
                            Option::<gtk::PropertyExpression>::None,
                            "last-modified",
                        ))
                        .build(),
                ),
            )),
            0,
            5, // placeholder, will be bound to a GSettings key later
        );
        settings
            .bind("recent-playlists-count", &recent_playlists_model, "size")
            .build();

        self.imp().playlists_btn.connect_toggled(clone!(
            #[weak]
            stack,
            #[weak]
            playlist_view,
            move |btn| {
                if btn.is_active() {
                    playlist_view.pop();
                    if stack
                        .visible_child_name()
                        .is_none_or(|name| name.as_str() != "playlists")
                    {
                        stack.set_visible_child_name("playlists");
                    }
                }
            }
        ));

        let recent_playlists_widget = self.imp().recent_playlists.get();
        recent_playlists_widget.bind_model(
            Some(&recent_playlists_model),
            clone!(
                #[strong]
                cache,
                #[weak]
                stack,
                #[weak]
                playlist_view,
                #[weak]
                split_view,
                #[weak]
                recent_btn,
                #[upgrade_or]
                SidebarButton::new("ERROR").upcast::<gtk::Widget>(),
                move |obj| {
                    let playlist = obj.downcast_ref::<INode>().unwrap();
                    let btn = SidebarButton::new(playlist.get_uri());
                    let (cover, cover_box) = playlist_cover_prefix();
                    fetch_playlist_cover(&cache, cover, playlist.get_uri(), false);
                    btn.set_prefix_child(cover_box);
                    btn.set_group(Some(&recent_btn));
                    btn.connect_toggled(clone!(
                        #[weak]
                        stack,
                        #[weak]
                        playlist_view,
                        #[weak]
                        split_view,
                        #[weak]
                        playlist,
                        move |btn| {
                            if btn.is_active() {
                                playlist_view.on_playlist_clicked(&playlist);
                                if stack
                                    .visible_child_name()
                                    .is_none_or(|name| name.as_str() != "playlists")
                                {
                                    stack.set_visible_child_name("playlists");
                                }
                                split_view.set_show_sidebar(!split_view.is_collapsed());
                            }
                        }
                    ));
                    btn.into()
                }
            ),
        );

        let dyn_playlist_view = win.get_dyn_playlist_view();
        let dyn_playlists = library.dyn_playlists();
        let recent_dyn_playlists_model = gtk::SliceListModel::new(
            Some(gtk::SortListModel::new(
                Some(dyn_playlists.clone()),
                Some(
                    gtk::StringSorter::builder()
                        .expression(gtk::PropertyExpression::new(
                            INode::static_type(),
                            Option::<gtk::PropertyExpression>::None,
                            "last-modified",
                        ))
                        .build(),
                ),
            )),
            0,
            5, // placeholder, will be bound to a GSettings key later
        );
        settings
            .bind(
                "recent-playlists-count",
                &recent_dyn_playlists_model,
                "size",
            )
            .build();

        self.imp().dyn_playlists_btn.connect_toggled(clone!(
            #[weak]
            stack,
            #[weak]
            dyn_playlist_view,
            move |btn| {
                if btn.is_active() {
                    dyn_playlist_view.pop();
                    stack.set_visible_child_name("dyn-playlists");
                }
            }
        ));

        let recent_dyn_playlists_widget = self.imp().recent_dyn_playlists.get();
        recent_dyn_playlists_widget.bind_model(
            Some(&recent_dyn_playlists_model),
            clone!(
                #[strong]
                cache,
                #[weak]
                stack,
                #[weak]
                dyn_playlist_view,
                #[weak]
                split_view,
                #[weak]
                recent_btn,
                #[upgrade_or]
                SidebarButton::new("ERROR").upcast::<gtk::Widget>(),
                move |obj| {
                    let playlist = obj.downcast_ref::<INode>().unwrap();
                    let btn = SidebarButton::new(playlist.get_uri());
                    let (cover, cover_box) = playlist_cover_prefix();
                    fetch_playlist_cover(&cache, cover, playlist.get_uri(), true);
                    btn.set_prefix_child(cover_box);
                    btn.set_group(Some(&recent_btn));
                    btn.connect_toggled(clone!(
                        #[weak]
                        stack,
                        #[weak]
                        dyn_playlist_view,
                        #[weak]
                        split_view,
                        #[weak]
                        playlist,
                        move |btn| {
                            if btn.is_active() {
                                dyn_playlist_view.on_playlist_clicked(&playlist);
                                if stack
                                    .visible_child_name()
                                    .is_none_or(|name| name.as_str() != "dyn-playlists")
                                {
                                    stack.set_visible_child_name("dyn-playlists");
                                }
                                split_view.set_show_sidebar(!split_view.is_collapsed());
                            }
                        }
                    ));
                    btn.into()
                }
            ),
        );

        self.hide_highlights();
        playlists.connect_items_changed(clone!(
            #[weak(rename_to = this)]
            self,
            move |_, _, _, _| {
                this.hide_highlights();
            }
        ));
        dyn_playlists.connect_items_changed(clone!(
            #[weak(rename_to = this)]
            self,
            move |_, _, _, _| {
                this.hide_highlights();
            }
        ));

        // Hide the list widget when there is no playlist at all to avoid
        // an unnecessary ~6px space after the Saved Playlists button
        recent_playlists_model
            .bind_property("n-items", &recent_playlists_widget, "visible")
            .transform_to(|_, len: u32| Some(len > 0))
            .sync_create()
            .build();
        recent_dyn_playlists_model
            .bind_property("n-items", &recent_dyn_playlists_widget, "visible")
            .transform_to(|_, len: u32| Some(len > 0))
            .sync_create()
            .build();

        client_state
            .bind_property(
                "supports-playlists",
                &self.imp().playlists_section.get(),
                "visible",
            )
            .sync_create()
            .build();

        // Dynamic playlists may rely on stickers.
        client_state
            .bind_property(
                "stickers-support-level",
                &self.imp().dyn_playlists_section.get(),
                "visible",
            )
            .transform_to(|_, lvl: StickersSupportLevel| Some(lvl == StickersSupportLevel::All))
            .sync_create()
            .build();

        // Incoming: what flacli is fetching. Rebuilt from the controller's snapshot on every refresh.
        let flacli_ctl = flacli();
        let flacli_state = flacli_ctl.state();
        flacli_state
            .bind_property("available", &self.imp().flacli_section.get(), "visible")
            .sync_create()
            .build();
        flacli_state
            .bind_property("active", &self.imp().incoming_spinner.get(), "visible")
            .sync_create()
            .build();
        self.imp().incoming_refresh.connect_clicked(clone!(
            #[strong]
            flacli_ctl,
            move |_| flacli_ctl.refresh()
        ));
        flacli_state.connect_closure(
            "refreshed",
            false,
            closure_local!(
                #[weak(rename_to = this)]
                self,
                #[weak]
                win,
                #[weak]
                library,
                #[weak]
                playlist_view,
                #[weak]
                stack,
                #[weak]
                split_view,
                move |_: FlacliState| {
                    let incoming: Vec<PlaylistStatus> = flacli()
                        .playlists()
                        .into_iter()
                        .filter(PlaylistStatus::is_incoming)
                        .collect();
                    let list = this.imp().incoming_list.get();
                    list.remove_all();
                    for playlist in incoming.iter() {
                        let (row, open) = incoming_row(&win, playlist);
                        let stored_name = playlist.mpd_playlist.clone();
                        let playlist_id = playlist.playlist_id;
                        let name = playlist.name.clone();
                        let is_requests = playlist.is_requests();
                        open.connect_clicked(clone!(
                            #[weak]
                            this,
                            #[weak]
                            win,
                            #[weak]
                            library,
                            #[weak]
                            playlist_view,
                            #[weak]
                            stack,
                            #[weak]
                            split_view,
                            move |_| {
                                // Open the stored playlist; have flacli store it in MPD first
                                // when it has not yet (it stores from the tracks on disk).
                                let open_stored = clone!(
                                    #[weak]
                                    this,
                                    #[weak]
                                    library,
                                    #[weak]
                                    playlist_view,
                                    #[weak]
                                    stack,
                                    #[weak]
                                    split_view,
                                    #[strong]
                                    stored_name,
                                    #[upgrade_or]
                                    false,
                                    move || -> bool {
                                        let found = library
                                            .playlists()
                                            .iter::<INode>()
                                            .filter_map(Result::ok)
                                            .find(|p| p.get_uri() == stored_name);
                                        let Some(inode) = found else {
                                            return false;
                                        };
                                        this.imp().playlists_btn.set_active(true);
                                        playlist_view.on_playlist_clicked(&inode);
                                        if stack
                                            .visible_child_name()
                                            .is_none_or(|name| name.as_str() != "playlists")
                                        {
                                            stack.set_visible_child_name("playlists");
                                        }
                                        split_view.set_show_sidebar(!split_view.is_collapsed());
                                        true
                                    }
                                );
                                // Requests is flacli's own list, never an MPD playlist: its page.
                                if is_requests {
                                    this.set_view("requests");
                                    split_view.set_show_sidebar(!split_view.is_collapsed());
                                    return;
                                }
                                if open_stored() {
                                    return;
                                }
                                let (playlist_id, name) = (playlist_id, name.clone());
                                glib::spawn_future_local(clone!(
                                    #[weak]
                                    win,
                                    #[weak]
                                    library,
                                    async move {
                                        win.send_simple_toast(&format!("Asking flacli to store “{name}” in MPD…"), 3);
                                        match flacli().store_in_mpd(playlist_id).await {
                                            Ok(line) => {
                                                let _ = library.init_playlists(true).await;
                                                if !open_stored() {
                                                    win.send_simple_toast(&format!("{line}, but the player does not list it yet. Try again in a moment."), 6);
                                                }
                                            }
                                            Err(e) => win.send_simple_toast(&format!("“{name}” is {e}"), 6),
                                        }
                                    }
                                ));
                            }
                        ));
                        list.append(&row);
                    }
                    let mut idx = 0;
                    while let Some(row) = list.row_at_index(idx) {
                        row.set_activatable(false);
                        idx += 1;
                    }
                    this.imp()
                        .incoming_section
                        .set_visible(flacli().state().available() && !incoming.is_empty());
                }
            ),
        );

        self.imp().queue_btn.connect_toggled(clone!(
            #[weak]
            stack,
            move |btn| {
                if btn.is_active() {
                    stack.set_visible_child_name("queue");
                }
            }
        ));

        // Connect the raw "clicked" signals to show-content
        self.imp()
            .queue_btn
            .upcast_ref::<gtk::Button>()
            .connect_clicked(clone!(
                #[weak]
                split_view,
                move |_| split_view.set_show_sidebar(!split_view.is_collapsed())
            ));
        for btn in [
            &self.imp().recent_btn.get(),
            &self.imp().albums_btn.get(),
            &self.imp().artists_btn.get(),
            &self.imp().folders_btn.get(),
            &self.imp().playlists_btn.get(),
            &self.imp().dyn_playlists_btn.get(),
        ] {
            btn.upcast_ref::<gtk::ToggleButton>()
                .upcast_ref::<gtk::Button>()
                .connect_clicked(clone!(
                    #[weak]
                    split_view,
                    move |_| split_view.set_show_sidebar(!split_view.is_collapsed())
                ));
        }

        player
            .bind_property("queue-len", &self.imp().queue_len.get(), "label")
            .transform_to(|_, size: u32| Some(size.to_string()))
            .sync_create()
            .build();
        // Set startup view.
        // If playlists or dynamic playlists were selected as startup view or was the last
        // view but are now not available, that view will still be displayed at first but will
        // be empty & can't be navigated back to once moved away.
        let state = utils::settings_manager().child("state");
        let mut view_to_show = View::try_from(state.enum_("startup-view") as u32).expect("Invalid startup-view setting value");
        if matches!(view_to_show, View::Last) {
            view_to_show = View::try_from(state.enum_("last-view") as u32).expect("Invalid last-view setting value");
        }
        self.set_view(view_to_show.as_str());
    }

    pub fn set_view(&self, view_name: &str) {
        match view_name {
            "albums" => self.imp().albums_btn.set_active(true),
            "artists" => self.imp().artists_btn.set_active(true),
            "folders" => self.imp().folders_btn.set_active(true),
            "playlists" => self.imp().playlists_btn.set_active(true),
            "dyn-playlists" => self.imp().dyn_playlists_btn.set_active(true),
            "recent" => self.imp().recent_btn.set_active(true),
            "queue" => self.imp().queue_btn.set_active(true),
            "get" => self.imp().get_btn.set_active(true),
            "requests" => self.imp().requests_btn.set_active(true),
            "tidy" => self.imp().tidy_btn.set_active(true),
            "ask" => self.imp().ask_btn.set_active(true),
            _ => {
                eprintln!("Unknown view: {}", view_name);
            }
        }
    }
}
