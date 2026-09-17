//! The flacli integration: surfaces fed by `flacli --compact status` (Tier 1) and the first
//! write actions into the shell, through the same CLI (Tier 2, issue #7: CLI first, D-Bus later).
mod actions;
mod controller;
mod state;

pub use actions::{cancel_job, fetch, import_from_service, queue, review};
pub use controller::{MissingTrack, PlaylistStatus, flacli, init, status_label};
pub use state::FlacliState;
