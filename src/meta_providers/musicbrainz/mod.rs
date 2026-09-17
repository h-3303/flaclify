static PROVIDER_KEY: &str = "musicbrainz";

mod controller;
mod models;
mod release_groups;
pub use controller::MusicBrainzWrapper;
pub use release_groups::{browse_release_groups, fold_title, library_has};
