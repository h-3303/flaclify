/* theme/mod.rs
 *
 * Flaclify theme engine.
 *
 * A theme is a plain GTK CSS file with a header comment carrying metadata:
 *
 *     @name: Folio
 *     @scheme: light          (light | dark | follow — optional)
 *     @auto-accent: off       (off | on — optional; off suppresses album-art accents)
 *     @art-background: off    (off | on — optional; off hides the blurred album-art wash)
 *
 * inside the leading comment block, followed by ordinary rules such as
 *     :root { --window-bg-color: #ece3cc; ... }
 *
 * Bundled themes live in the gresource under /themes/. User themes are read from
 * any .css file in $XDG_CONFIG_HOME/flaclify/themes, which is watched so edits apply live.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use adw::{ColorScheme, prelude::*};
use gtk::{gdk, gio, glib};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    time::Duration,
};

use crate::utils::settings_manager;

const RESOURCE_DIR: &str = "/io/github/h3303/Flaclify/themes/";
pub const DEFAULT_ID: &str = "default";

#[derive(Clone, Debug)]
pub struct Theme {
    /// Stable identifier stored in settings. `default` for stock Adwaita, the file
    /// stem for bundled themes, `user/<stem>` for user themes.
    pub id: String,
    pub name: String,
    pub scheme: Option<ColorScheme>,
    /// `Some(false)` means the theme owns its accent and album-art accents are suppressed.
    pub auto_accent: Option<bool>,
    /// `Some(false)` means the blurred album-art background is hidden while this theme is on.
    pub art_background: Option<bool>,
    pub css: String,
    pub user: bool,
}

impl Theme {
    fn stock() -> Self {
        Self {
            id: DEFAULT_ID.to_owned(),
            name: "Adwaita".to_owned(),
            scheme: None,
            auto_accent: None,
            art_background: None,
            css: String::new(),
            user: false,
        }
    }

    fn parse(id: String, fallback_name: &str, css: String, user: bool) -> Self {
        let mut name = fallback_name.to_owned();
        let mut scheme = None;
        let mut auto_accent = None;
        let mut art_background = None;

        // Only the leading comment block is inspected.
        if let Some(start) = css.find("/*") {
            let end = css[start..].find("*/").map(|e| start + e).unwrap_or(css.len());
            for line in css[start + 2..end].lines() {
                let line = line.trim().trim_start_matches('*').trim();
                let Some(rest) = line.strip_prefix('@') else {
                    continue;
                };
                let Some((key, value)) = rest.split_once(':') else {
                    continue;
                };
                let value = value.trim();
                match key.trim() {
                    "name" if !value.is_empty() => name = value.to_owned(),
                    "scheme" => {
                        scheme = match value {
                            "dark" => Some(ColorScheme::ForceDark),
                            "light" => Some(ColorScheme::ForceLight),
                            "follow" | "system" | "default" => Some(ColorScheme::Default),
                            _ => None,
                        }
                    }
                    "auto-accent" => {
                        auto_accent = match value {
                            "off" | "false" | "no" => Some(false),
                            "on" | "true" | "yes" => Some(true),
                            _ => None,
                        }
                    }
                    "art-background" | "album-art-bg" => {
                        art_background = match value {
                            "off" | "false" | "no" => Some(false),
                            "on" | "true" | "yes" => Some(true),
                            _ => None,
                        }
                    }
                    _ => {}
                }
            }
        }

        Self {
            id,
            name,
            scheme,
            auto_accent,
            art_background,
            css,
            user,
        }
    }

    pub fn is_default(&self) -> bool {
        self.id == DEFAULT_ID
    }

    /// True when album-art / system accent injection must not override this theme.
    pub fn suppresses_auto_accent(&self) -> bool {
        self.auto_accent == Some(false)
    }

    /// True when the blurred album-art background must stay hidden under this theme.
    pub fn suppresses_art_background(&self) -> bool {
        self.art_background == Some(false)
    }
}

type AppliedListener = Box<dyn Fn(&Theme)>;
type ListListener = Box<dyn Fn()>;

pub struct ThemeManager {
    provider: gtk::CssProvider,
    themes: RefCell<Vec<Theme>>,
    current: RefCell<Theme>,
    applied_listeners: RefCell<Vec<AppliedListener>>,
    list_listeners: RefCell<Vec<ListListener>>,
    monitor: RefCell<Option<gio::FileMonitor>>,
    reload_source: RefCell<Option<glib::SourceId>>,
    /// Kept alive so external changes to the `theme` key (gsettings, dconf) apply live.
    settings: gio::Settings,
}

thread_local! {
    static MANAGER: RefCell<Option<Rc<ThemeManager>>> = const { RefCell::new(None) };
}

/// Access the process-wide theme manager. Panics if `init` has not run.
pub fn theme_manager() -> Rc<ThemeManager> {
    MANAGER.with(|m| {
        m.borrow()
            .clone()
            .expect("theme::init() must run before theme_manager()")
    })
}

/// Directory scanned for user themes.
pub fn user_theme_dir() -> PathBuf {
    let mut dir = glib::user_config_dir();
    dir.push("flaclify");
    dir.push("themes");
    dir
}

/// Install the theme CSS provider on the default display and apply the saved theme.
/// Must run on the main thread once a display exists (i.e. from the app's startup handler),
/// and before the main window is constructed so that the window's accent provider is
/// added after (and therefore cascades over) the theme provider.
pub fn init() {
    let provider = gtk::CssProvider::new();
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }

    let manager = Rc::new(ThemeManager {
        provider,
        themes: RefCell::new(Vec::new()),
        current: RefCell::new(Theme::stock()),
        applied_listeners: RefCell::new(Vec::new()),
        list_listeners: RefCell::new(Vec::new()),
        monitor: RefCell::new(None),
        reload_source: RefCell::new(None),
        settings: settings_manager().child("ui"),
    });
    MANAGER.with(|m| *m.borrow_mut() = Some(manager.clone()));

    manager.rescan();
    manager.start_watching();

    let saved = manager.settings.string("theme");
    manager.apply(saved.as_str());

    let weak = Rc::downgrade(&manager);
    manager.settings.connect_changed(Some("theme"), move |settings, _| {
        if let Some(this) = weak.upgrade() {
            let wanted = settings.string("theme");
            if this.current().id != wanted.as_str() {
                this.apply(wanted.as_str());
            }
        }
    });
}

impl ThemeManager {
    /// All known themes: stock first, then bundled, then user themes.
    pub fn themes(&self) -> Vec<Theme> {
        self.themes.borrow().clone()
    }

    pub fn current(&self) -> Theme {
        self.current.borrow().clone()
    }

    pub fn connect_applied<F: Fn(&Theme) + 'static>(&self, f: F) {
        self.applied_listeners.borrow_mut().push(Box::new(f));
    }

    pub fn connect_list_changed<F: Fn() + 'static>(&self, f: F) {
        self.list_listeners.borrow_mut().push(Box::new(f));
    }

    /// Apply a theme by id. Unknown ids fall back to stock Adwaita.
    pub fn apply(&self, id: &str) {
        let theme = self
            .themes
            .borrow()
            .iter()
            .find(|t| t.id == id)
            .cloned()
            .unwrap_or_else(Theme::stock);

        self.provider.load_from_string(&theme.css);

        let settings = &self.settings;
        if let Some(scheme) = theme.scheme {
            adw::StyleManager::default().set_color_scheme(scheme);
            let _ = settings.set_string(
                "colorscheme",
                match scheme {
                    ColorScheme::ForceDark => "dark",
                    ColorScheme::ForceLight => "light",
                    _ => "follow",
                },
            );
        }
        if settings.string("theme").as_str() != theme.id {
            let _ = settings.set_string("theme", &theme.id);
        }

        *self.current.borrow_mut() = theme.clone();
        for f in self.applied_listeners.borrow().iter() {
            f(&theme);
        }
    }

    /// Step through the theme list. `delta` is +1 for next, -1 for previous.
    pub fn cycle(&self, delta: i32) {
        let next_id = {
            let themes = self.themes.borrow();
            if themes.is_empty() {
                return;
            }
            let current_id = self.current.borrow().id.clone();
            let idx = themes
                .iter()
                .position(|t| t.id == current_id)
                .unwrap_or(0) as i32;
            let n = themes.len() as i32;
            themes[((idx + delta) % n + n) as usize % themes.len()].id.clone()
        };
        self.apply(&next_id);
    }

    /// Re-read bundled and user themes. Re-applies the current theme if it is a user
    /// theme (so edits show up) or drops back to stock if it vanished.
    pub fn rescan(&self) {
        let mut themes = vec![Theme::stock()];
        themes.extend(load_bundled());
        themes.extend(load_user());
        *self.themes.borrow_mut() = themes;

        for f in self.list_listeners.borrow().iter() {
            f();
        }

        let current = self.current();
        if current.user || !self.themes.borrow().iter().any(|t| t.id == current.id) {
            self.apply(&current.id);
        }
    }

    fn start_watching(self: &Rc<Self>) {
        let dir = user_theme_dir();
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("Could not create user theme directory {:?}: {e}", dir);
            return;
        }
        let file = gio::File::for_path(&dir);
        match file.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(monitor) => {
                let weak = Rc::downgrade(self);
                monitor.connect_changed(move |_, _, _, _| {
                    if let Some(this) = weak.upgrade() {
                        this.schedule_reload();
                    }
                });
                *self.monitor.borrow_mut() = Some(monitor);
            }
            Err(e) => eprintln!("Could not watch user theme directory: {e}"),
        }
    }

    /// Editors write files in several steps; coalesce a burst of events into one rescan.
    fn schedule_reload(self: &Rc<Self>) {
        if let Some(src) = self.reload_source.borrow_mut().take() {
            src.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(Duration::from_millis(250), move || {
            if let Some(this) = weak.upgrade() {
                *this.reload_source.borrow_mut() = None;
                this.rescan();
            }
        });
        *self.reload_source.borrow_mut() = Some(id);
    }
}

fn load_bundled() -> Vec<Theme> {
    let mut out = Vec::new();
    let Ok(children) =
        gio::resources_enumerate_children(RESOURCE_DIR, gio::ResourceLookupFlags::NONE)
    else {
        return out;
    };
    for child in children {
        let Some(stem) = child.strip_suffix(".css") else {
            continue;
        };
        let path = format!("{RESOURCE_DIR}{child}");
        let Ok(bytes) = gio::resources_lookup_data(&path, gio::ResourceLookupFlags::NONE) else {
            continue;
        };
        let css = String::from_utf8_lossy(&bytes).into_owned();
        out.push(Theme::parse(stem.to_owned(), stem, css, false));
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

fn load_user() -> Vec<Theme> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(user_theme_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("css") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        match std::fs::read_to_string(&path) {
            Ok(css) => out.push(Theme::parse(format!("user/{stem}"), stem, css, true)),
            Err(e) => eprintln!("Could not read theme {:?}: {e}", path),
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}
