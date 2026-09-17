//! The flacli integration: surfaces fed by `flacli --compact status` (Tier 1) and the first
//! write actions into the shell, through the same CLI (Tier 2, issue #7: CLI first, D-Bus later).
mod actions;
mod ask_view;
mod controller;
mod finder;
mod get_view;
mod requests_view;
mod tidy_view;
mod state;

pub use actions::{cancel_job, fetch, import_from_service, queue, review, skip_rest};
pub use ask_view::AskView;
pub use finder::get_music;
pub use get_view::GetView;
pub use requests_view::RequestsView;
pub use tidy_view::TidyView;
pub use controller::{MissingTrack, PlaylistStatus, flacli, init, status_label};
pub use state::FlacliState;
