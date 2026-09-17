use gtk::glib::{
    self, Properties, derived_properties,
    prelude::*,
    subclass::{Signal, prelude::*},
};
use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};

mod imp {
    use super::*;

    /// What the player knows about flacli. Views bind to these; the controller sets them.
    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::FlacliState)]
    pub struct FlacliState {
        /// `flacli` was found on the PATH.
        #[property(get, set)]
        pub on_path: Cell<bool>,
        /// MPD's music_directory and flacli's music_dir are the same folder, or one is inside the other.
        #[property(get, set)]
        pub same_library: Cell<bool>,
        /// The Nicotine+ bridge answers `flacli doctor`. Tier 2 gates fetch actions on this.
        #[property(get, set)]
        pub bridge_reachable: Cell<bool>,
        /// The gate every flacli-aware feature checks: on the PATH and the same library.
        #[property(get, set)]
        pub available: Cell<bool>,
        /// Why `available` is false, in a sentence; empty when it is true.
        #[property(get, set)]
        pub reason: RefCell<String>,
        /// A job is running or downloads are in flight; the controller polls while this holds.
        #[property(get, set)]
        pub active: Cell<bool>,
        /// flacli's music_dir, as `flacli config` reports it.
        #[property(get, set)]
        pub music_dir: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FlacliState {
        const NAME: &'static str = "FlaclifyFlacliState";
        type Type = super::FlacliState;
    }

    #[derived_properties]
    impl ObjectImpl for FlacliState {
        fn signals() -> &'static [Signal] {
            static SIGNALS: OnceLock<Vec<Signal>> = OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    // The playlist snapshot changed; read it again from the controller.
                    Signal::builder("refreshed").build(),
                ]
            })
        }
    }
}

glib::wrapper! {
    pub struct FlacliState(ObjectSubclass<imp::FlacliState>);
}

impl Default for FlacliState {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl FlacliState {
    pub fn emit_refreshed(&self) {
        self.emit_by_name::<()>("refreshed", &[]);
    }
}
