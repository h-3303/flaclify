static PROVIDER_KEY: &str = "musicbrainz";

mod controller;
mod models;
mod finder;
mod release_groups;
pub use controller::MusicBrainzWrapper;
pub use finder::{FoundAlbum, FoundArtist, FoundSong, format_length, release_group_tracks, search_albums, search_artists, search_songs};
pub use release_groups::{browse_release_groups, enabled as musicbrainz_enabled, fold_title, library_has};
