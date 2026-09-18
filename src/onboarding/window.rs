/* window.rs
 *
 * Copyright 2026 htkhiem2000
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

//! The first-run wizard. Five pages on a carousel: welcome; where the music is (the built-in
//! player on a folder, or an MPD the person already runs); how the library is laid out
//! (upstream's page); flacli, optional; and a short map of the player. Settings are written as
//! each page is left, so Back and forth costs nothing, and the main window opens on the last
//! button.

use crate::{application::EuphonicaApplication, local_mpd, utils::settings_manager};
use adw::{prelude::*, subclass::prelude::*};
use glib::WeakRef;
use gtk::{
    gio::{self},
    glib::{self, clone},
};
use std::cell::Cell;

use glib::Properties;

const PAGE_WELCOME: u32 = 0;
const PAGE_MUSIC: u32 = 1;
const PAGE_LAYOUT: u32 = 2;
const PAGE_FLACLI: u32 = 3;
const PAGE_READY: u32 = 4;

const FLACLI_GUIDE_URL: &str = "https://flacli.vercel.app/#install";

/// One of the status marks the preferences pages also use.
#[derive(Clone, Copy)]
enum Mark {
    Loading,
    Good,
    Warn,
    Neutral,
}

fn set_mark(img: &gtk::Image, mark: Mark) {
    let (icon, class) = match mark {
        Mark::Loading => ("fl-content-loading-symbolic", "dim-label"),
        Mark::Good => ("enabled-feature-symbolic", "success"),
        Mark::Warn => ("exclamation-mark-symbolic", "warning"),
        Mark::Neutral => ("dot-symbolic", "dim-label"),
    };
    img.set_icon_name(Some(icon));
    img.set_css_classes(&[class]);
}

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties, gtk::CompositeTemplate)]
    #[properties(wrapper_type = super::EuphonicaOnboardingWindow)]
    #[template(resource = "/io/github/h3303/Flaclify/gtk/onboarding-window.ui")]
    pub struct EuphonicaOnboardingWindow {
        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub carousel: TemplateChild<adw::Carousel>,
        #[template_child]
        pub back_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub next_btn: TemplateChild<gtk::Button>,

        // Page 2: the music
        #[template_child]
        pub music_dir_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub music_dir_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub managed_status_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub managed_status_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub own_mpd_row: TemplateChild<adw::ExpanderRow>,
        #[template_child]
        pub own_use_unix_socket: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub own_unix_socket: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub own_host: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub own_port: TemplateChild<adw::EntryRow>,

        // Page 3: library organisation
        #[template_child]
        pub release_folder_library_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub mixed_library_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub release_folder_library_mode: TemplateChild<gtk::CheckButton>,
        #[template_child]
        pub mixed_library_mode: TemplateChild<gtk::CheckButton>,

        // Page 4: flacli
        #[template_child]
        pub flacli_status_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub flacli_status_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub nicotine_status_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub nicotine_status_icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub flacli_guide_btn: TemplateChild<adw::ButtonRow>,
        #[template_child]
        pub flacli_recheck_btn: TemplateChild<adw::ButtonRow>,

        // Page 5
        #[template_child]
        pub ready_get_row: TemplateChild<adw::ActionRow>,

        pub app: WeakRef<EuphonicaApplication>,
        pub onboard_success: Cell<bool>,
        pub managed_available: Cell<bool>,
        pub layout_chosen: Cell<bool>,
        pub flacli_found: Cell<bool>,
        /// The page the carousel is on or heading to; the carousel's own position is
        /// fractional while it animates, so a quick second click would misread it.
        pub page: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EuphonicaOnboardingWindow {
        const NAME: &'static str = "EuphonicaOnboardingWindow";
        type Type = super::EuphonicaOnboardingWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for EuphonicaOnboardingWindow {
        fn constructed(&self) {
            self.parent_constructed();

            let library_settings = settings_manager().child("library");
            library_settings
                .bind(
                    "optimize-embedded-cover-loading",
                    &self.release_folder_library_mode.get(),
                    "active",
                )
                .flags(gio::SettingsBindFlags::SET)
                .build();

            self.own_use_unix_socket
                .bind_property("active", &self.own_unix_socket.get(), "visible")
                .sync_create()
                .build();
            self.own_use_unix_socket
                .bind_property("active", &self.own_host.get(), "visible")
                .invert_boolean()
                .sync_create()
                .build();
            self.own_use_unix_socket
                .bind_property("active", &self.own_port.get(), "visible")
                .invert_boolean()
                .sync_create()
                .build();
        }
    }
    impl WidgetImpl for EuphonicaOnboardingWindow {}
    impl WindowImpl for EuphonicaOnboardingWindow {}
    impl ApplicationWindowImpl for EuphonicaOnboardingWindow {}
    impl AdwApplicationWindowImpl for EuphonicaOnboardingWindow {}
}

glib::wrapper! {
    pub struct EuphonicaOnboardingWindow(ObjectSubclass<imp::EuphonicaOnboardingWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow,
    adw::ApplicationWindow,
    @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible,
    gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root,
    gtk::ShortcutManager;
}

impl EuphonicaOnboardingWindow {
    pub fn new(application: &EuphonicaApplication) -> Self {
        let win: Self = glib::Object::builder()
            .property("application", application)
            .build();
        let imp = win.imp();
        imp.app.set(Some(application));
        imp.onboard_success.set(false);

        win.setup_music_page();
        win.setup_layout_page();
        win.setup_flacli_page();

        imp.back_btn.connect_clicked(clone!(
            #[weak]
            win,
            move |_| {
                let page = win.current_page();
                if page > 0 {
                    win.scroll_to(page - 1);
                }
            }
        ));
        imp.next_btn.connect_clicked(clone!(
            #[weak]
            win,
            move |_| {
                let page = win.current_page();
                win.leave_page(page);
                if page >= PAGE_READY {
                    win.imp().onboard_success.set(true);
                    win.close();
                } else {
                    win.scroll_to(page + 1);
                }
            }
        ));
        imp.carousel.connect_page_changed(clone!(
            #[weak]
            win,
            move |_, page| {
                win.imp().page.set(page);
                win.on_page_shown(page);
            }
        ));
        win.on_page_shown(PAGE_WELCOME);

        // Only emitted when the close button itself is clicked
        win.connect_close_request(|win| {
            win.imp()
                .app
                .upgrade()
                .unwrap()
                .conclude_onboarding(win.imp().onboard_success.get());
            glib::Propagation::Proceed
        });

        win
    }

    fn current_page(&self) -> u32 {
        self.imp().page.get()
    }

    fn scroll_to(&self, page: u32) {
        let carousel = self.imp().carousel.get();
        let page = page.min(carousel.n_pages().saturating_sub(1));
        self.imp().page.set(page);
        carousel.scroll_to(&carousel.nth_page(page), true);
        // The button reflects the page being headed for, so a click during the animation
        // acts on the right one.
        self.on_page_shown(page);
    }

    /// Enable the bottom button for what the page needs, and word it.
    fn on_page_shown(&self, page: u32) {
        let imp = self.imp();
        imp.back_btn.set_visible(page > PAGE_WELCOME);
        let (label, enabled) = match page {
            PAGE_WELCOME => ("Continue", true),
            PAGE_MUSIC => (
                "Continue",
                imp.managed_available.get() || imp.own_mpd_row.enables_expansion(),
            ),
            PAGE_LAYOUT => ("Continue", imp.layout_chosen.get()),
            PAGE_FLACLI => (
                if imp.flacli_found.get() {
                    "Continue"
                } else {
                    "Skip for now"
                },
                true,
            ),
            _ => ("Start using Flaclify", true),
        };
        imp.next_btn.set_label(label);
        imp.next_btn.set_sensitive(enabled);
        if page == PAGE_READY {
            imp.ready_get_row.set_visible(imp.flacli_found.get());
        }
    }

    /// Write what a page decided as it is left.
    fn leave_page(&self, page: u32) {
        let imp = self.imp();
        if page != PAGE_MUSIC {
            return;
        }
        let client = settings_manager().child("client");
        if imp.own_mpd_row.enables_expansion() {
            let _ = client.set_boolean("managed-mpd", false);
            let _ = client.set_boolean("mpd-use-unix-socket", imp.own_use_unix_socket.is_active());
            let socket = imp.own_unix_socket.text();
            if !socket.trim().is_empty() {
                let _ = client.set_string("mpd-unix-socket", socket.trim());
            }
            let host = imp.own_host.text();
            if !host.trim().is_empty() {
                let _ = client.set_string("mpd-host", host.trim());
            }
            if let Ok(port) = imp.own_port.text().trim().parse::<u32>() {
                let _ = client.set_uint("mpd-port", port);
            }
        } else {
            let _ = client.set_boolean("managed-mpd", true);
            let chosen = local_mpd::music_dir();
            let value = if chosen == local_mpd::default_music_dir() {
                String::new()
            } else {
                chosen.to_string_lossy().into_owned()
            };
            let _ = client.set_string("managed-mpd-music-dir", &value);
            local_mpd::apply_connection_settings();
        }
    }

    // Page 2 -------------------------------------------------------------------------------

    fn setup_music_page(&self) {
        let imp = self.imp();
        let client = settings_manager().child("client");
        imp.own_use_unix_socket
            .set_active(client.boolean("mpd-use-unix-socket"));
        imp.own_unix_socket
            .set_text(&client.string("mpd-unix-socket"));
        imp.own_host.set_text(&client.string("mpd-host"));
        imp.own_port.set_text(&client.uint("mpd-port").to_string());

        self.refresh_music_page();

        imp.music_dir_btn.connect_clicked(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.pick_music_dir()
        ));
        imp.own_mpd_row.connect_enable_expansion_notify(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.refresh_music_page()
        ));
    }

    fn refresh_music_page(&self) {
        let imp = self.imp();
        let dir = local_mpd::music_dir();
        imp.music_dir_row.set_subtitle(&dir.to_string_lossy());

        let available = local_mpd::binary().is_some();
        imp.managed_available.set(available);
        let own = imp.own_mpd_row.enables_expansion();
        let (mark, text) = if own {
            (
                Mark::Neutral,
                "Not started; Flaclify connects to yours.".to_owned(),
            )
        } else if available {
            (Mark::Good, local_mpd::describe())
        } else {
            (Mark::Warn, local_mpd::describe())
        };
        set_mark(&imp.managed_status_icon, mark);
        imp.managed_status_row.set_subtitle(&text);
        imp.music_dir_row.set_sensitive(!own);
        if self.current_page() == PAGE_MUSIC {
            self.on_page_shown(PAGE_MUSIC);
        }
    }

    fn pick_music_dir(&self) {
        let dialog = gtk::FileDialog::builder()
            .title("Music folder")
            .modal(true)
            .initial_folder(&gio::File::for_path(local_mpd::music_dir()))
            .build();
        dialog.select_folder(
            Some(self),
            gio::Cancellable::NONE,
            clone!(
                #[weak(rename_to = this)]
                self,
                move |result| {
                    if let Ok(folder) = result
                        && let Some(path) = folder.path()
                    {
                        let client = settings_manager().child("client");
                        let _ = client.set_string("managed-mpd-music-dir", &path.to_string_lossy());
                        this.refresh_music_page();
                    }
                }
            ),
        );
    }

    // Page 3 -------------------------------------------------------------------------------

    fn setup_layout_page(&self) {
        let imp = self.imp();
        for (button, option) in [
            (
                imp.release_folder_library_button.get(),
                imp.release_folder_library_mode.get(),
            ),
            (imp.mixed_library_button.get(), imp.mixed_library_mode.get()),
        ] {
            button.connect_clicked(move |_| option.set_active(true));
        }
        for button in [
            imp.release_folder_library_mode.get(),
            imp.mixed_library_mode.get(),
        ] {
            button.connect_toggled(clone!(
                #[weak(rename_to = this)]
                self,
                move |_| {
                    this.imp().layout_chosen.set(true);
                    if this.current_page() == PAGE_LAYOUT {
                        this.on_page_shown(PAGE_LAYOUT);
                    }
                }
            ));
        }
    }

    // Page 4 -------------------------------------------------------------------------------

    fn setup_flacli_page(&self) {
        let imp = self.imp();
        imp.flacli_guide_btn.connect_activated(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                gtk::UriLauncher::new(FLACLI_GUIDE_URL).launch(
                    Some(&this),
                    gio::Cancellable::NONE,
                    |_| {},
                );
            }
        ));
        imp.flacli_recheck_btn.connect_activated(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| this.probe_flacli()
        ));
        self.probe_flacli();
    }

    fn probe_flacli(&self) {
        let imp = self.imp();
        set_mark(&imp.flacli_status_icon, Mark::Loading);
        imp.flacli_status_row.set_subtitle("Looking…");
        set_mark(&imp.nicotine_status_icon, Mark::Loading);
        imp.nicotine_status_row.set_subtitle("Looking…");
        glib::spawn_future_local(clone!(
            #[weak(rename_to = this)]
            self,
            async move {
                let verdict = crate::flacli::probe().await;
                let imp = this.imp();
                let mut found = false;
                match verdict {
                    None => {
                        set_mark(&imp.flacli_status_icon, Mark::Neutral);
                        imp.flacli_status_row.set_subtitle(
                            "Not installed. The guide takes a few minutes: Nicotine+, then one script.",
                        );
                        set_mark(&imp.nicotine_status_icon, Mark::Neutral);
                        imp.nicotine_status_row.set_subtitle("Comes with flacli.");
                    }
                    Some(Err(e)) => {
                        set_mark(&imp.flacli_status_icon, Mark::Warn);
                        imp.flacli_status_row
                            .set_subtitle(&format!("Found, but flacli doctor failed: {e}"));
                        set_mark(&imp.nicotine_status_icon, Mark::Neutral);
                        imp.nicotine_status_row
                            .set_subtitle("Unknown until flacli answers.");
                    }
                    Some(Ok(doctor)) => {
                        found = true;
                        set_mark(&imp.flacli_status_icon, Mark::Good);
                        let version = if doctor.version.is_empty() {
                            String::new()
                        } else {
                            format!(" {}", doctor.version)
                        };
                        imp.flacli_status_row.set_subtitle(&format!(
                            "Found flacli{version}; it files music into {}.",
                            doctor.music_dir
                        ));
                        if doctor.nicotine.reachable {
                            set_mark(&imp.nicotine_status_icon, Mark::Good);
                            imp.nicotine_status_row
                                .set_subtitle(if doctor.nicotine.online {
                                    "Answers, and Nicotine+ is logged in."
                                } else {
                                    "Answers; Nicotine+ is not logged in yet."
                                });
                        } else {
                            set_mark(&imp.nicotine_status_icon, Mark::Warn);
                            imp.nicotine_status_row.set_subtitle(
                                "Not reachable. Start Nicotine+ and tick MCP Bridge under Preferences → Plugins; fetching needs it, browsing does not.",
                            );
                        }
                    }
                }
                imp.flacli_found.set(found);
                if this.current_page() == PAGE_FLACLI {
                    this.on_page_shown(PAGE_FLACLI);
                }
            }
        ));
    }
}
