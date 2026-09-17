/* cache/placeholders.rs
 *
 * Album-art placeholders that follow the active theme.
 *
 * A `ThemedPlaceholder` is a `gdk::Paintable` wrapping whichever texture the
 * current theme's icon set provides (`/iconsets/<set>/albumart-placeholder.svg`),
 * falling back to the stock artwork. Widgets hold the paintable, not the
 * texture, so when the theme changes `refresh()` swaps the texture underneath
 * and every cell showing a placeholder repaints on its own.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use gtk::{
    gdk::{self, prelude::*, subclass::paintable::*},
    gio, glib,
    subclass::prelude::*,
};
use once_cell::sync::Lazy;
use std::cell::{OnceCell, RefCell};

const STOCK_FULL: &str = "/io/github/h3303/Flaclify/albumart-placeholder.svg";
const STOCK_THUMB: &str = "/io/github/h3303/Flaclify/albumart-placeholder-thumb.png";
const ICONSET_DIR: &str = "/io/github/h3303/Flaclify/iconsets/";

pub static EMPTY_ALBUM_STRING: Lazy<&str> = Lazy::new(|| "(untitled album)");
pub static EMPTY_ARTIST_STRING: Lazy<&str> = Lazy::new(|| "(unknown artist)");

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ThemedPlaceholder {
        pub texture: RefCell<Option<gdk::Texture>>,
        pub stock: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ThemedPlaceholder {
        const NAME: &'static str = "FlaclifyThemedPlaceholder";
        type Type = super::ThemedPlaceholder;
        type Interfaces = (gdk::Paintable,);
    }

    impl ObjectImpl for ThemedPlaceholder {}

    impl PaintableImpl for ThemedPlaceholder {
        fn flags(&self) -> gdk::PaintableFlags {
            // Contents and size both change with the theme.
            gdk::PaintableFlags::empty()
        }

        fn current_image(&self) -> gdk::Paintable {
            match self.texture.borrow().as_ref() {
                Some(t) => t.clone().upcast(),
                None => gdk::Paintable::new_empty(1, 1),
            }
        }

        fn intrinsic_width(&self) -> i32 {
            self.texture.borrow().as_ref().map(|t| t.width()).unwrap_or(1)
        }

        fn intrinsic_height(&self) -> i32 {
            self.texture.borrow().as_ref().map(|t| t.height()).unwrap_or(1)
        }

        fn intrinsic_aspect_ratio(&self) -> f64 {
            match self.texture.borrow().as_ref() {
                Some(t) if t.height() > 0 => t.width() as f64 / t.height() as f64,
                _ => 1.0,
            }
        }

        fn snapshot(&self, snapshot: &gdk::Snapshot, width: f64, height: f64) {
            if let Some(t) = self.texture.borrow().as_ref() {
                t.snapshot(snapshot, width, height);
            }
        }
    }
}

glib::wrapper! {
    pub struct ThemedPlaceholder(ObjectSubclass<imp::ThemedPlaceholder>)
        @implements gdk::Paintable;
}

impl ThemedPlaceholder {
    fn new(stock: &str, set: Option<&str>) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().stock.replace(stock.to_owned());
        obj.load(set);
        obj
    }

    /// Load the theme set's placeholder if it ships one, else the stock artwork.
    fn load(&self, set: Option<&str>) {
        let themed = set.map(|s| format!("{ICONSET_DIR}{s}/albumart-placeholder.svg"));
        let path = themed
            .filter(|p| gio::resources_get_info(p, gio::ResourceLookupFlags::NONE).is_ok())
            .unwrap_or_else(|| self.imp().stock.borrow().clone());
        match gdk::Texture::from_resource(&path) {
            texture => {
                self.imp().texture.replace(Some(texture));
            }
        }
        self.invalidate_size();
        self.invalidate_contents();
    }
}

thread_local! {
    static CURRENT_SET: RefCell<Option<String>> = const { RefCell::new(None) };
    static FULL: OnceCell<ThemedPlaceholder> = const { OnceCell::new() };
    static THUMB: OnceCell<ThemedPlaceholder> = const { OnceCell::new() };
}

/// The shared album-art placeholder paintable (full size or thumbnail).
pub fn albumart(thumb: bool) -> ThemedPlaceholder {
    let set = CURRENT_SET.with(|s| s.borrow().clone());
    let cell = if thumb { &THUMB } else { &FULL };
    cell.with(|c| {
        c.get_or_init(|| {
            ThemedPlaceholder::new(if thumb { STOCK_THUMB } else { STOCK_FULL }, set.as_deref())
        })
        .clone()
    })
}

/// Called by the theme manager whenever the active icon set changes.
pub fn refresh(set: Option<&str>) {
    CURRENT_SET.with(|s| *s.borrow_mut() = set.map(str::to_owned));
    for cell in [&FULL, &THUMB] {
        cell.with(|c| {
            if let Some(p) = c.get() {
                p.load(set);
            }
        });
    }
}
