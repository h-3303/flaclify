//! The release groups MusicBrainz lists for an artist, for the "not in the library" list under
//! a discography (roadmap Tier 1, issue #3). Blocking: call from a worker thread.

use gtk::prelude::SettingsExt;
use std::{thread::sleep, time::Duration};

use musicbrainz_rs::{
    client::MusicBrainzClient,
    entity::release_group::{ReleaseGroup, ReleaseGroupPrimaryType},
    error::Error as MBError,
    prelude::*,
};

use crate::{
    config::APPLICATION_USER_AGENT,
    utils::{meta_provider_settings, settings_manager},
};

use super::PROVIDER_KEY;

/// The largest page MusicBrainz serves.
const PAGE: u8 = 100;

#[derive(Debug, Clone)]
pub struct ReleaseGroupSummary {
    pub mbid: String,
    pub title: String,
    /// "Album" or "EP".
    pub kind: &'static str,
    /// The year of the first release, if MusicBrainz knows it.
    pub year: Option<String>,
}

impl ReleaseGroupSummary {
    /// "2019 · Album", as a row subtitle.
    pub fn describe(&self) -> String {
        match self.year.as_deref() {
            Some(year) => format!("{year} · {}", self.kind),
            None => self.kind.to_owned(),
        }
    }

    pub fn url(&self) -> String {
        format!("https://musicbrainz.org/release-group/{}", self.mbid)
    }
}

/// Studio albums and EPs only: a group with any secondary type (live, compilation, remix,
/// soundtrack, ...) is left out, as is anything without a primary type.
fn summarise(group: ReleaseGroup) -> Option<ReleaseGroupSummary> {
    let kind = match group.primary_type? {
        ReleaseGroupPrimaryType::Album => "Album",
        ReleaseGroupPrimaryType::Ep => "EP",
        _ => return None,
    };
    if !group.secondary_types.is_empty() {
        return None;
    }
    Some(ReleaseGroupSummary {
        mbid: group.id,
        title: group.title,
        kind,
        year: group
            .first_release_date
            .map(|d| d.0)
            .filter(|d| d.len() >= 4)
            .map(|d| d[..4].to_owned()),
    })
}

/// Whether the MusicBrainz provider is switched on in Preferences.
pub fn enabled() -> bool {
    meta_provider_settings(PROVIDER_KEY).boolean("enabled")
}

/// A blocking client with the app's user agent and the configured retry count.
pub(super) fn client() -> MusicBrainzClient {
    let mut client = MusicBrainzClient::default();
    client.max_retries = settings_manager()
        .child("metaprovider")
        .uint("n-tries")
        .max(1);
    let _ = client.set_user_agent(APPLICATION_USER_AGENT);
    client
}

/// Every studio album and EP MusicBrainz credits to the artist, newest first. Respects the
/// MusicBrainz provider's "enabled" switch: off means an empty list, no request.
pub fn browse_release_groups(artist_mbid: &str) -> Result<Vec<ReleaseGroupSummary>, MBError> {
    if !enabled() {
        return Ok(Vec::new());
    }
    let client = client();

    let mut groups: Vec<ReleaseGroupSummary> = Vec::new();
    let mut offset: u16 = 0;
    loop {
        println!("[MusicBrainz] Browsing release groups of artist {artist_mbid}, offset {offset}");
        let page = ReleaseGroup::browse()
            .by_artist(artist_mbid)
            .limit(PAGE)
            .offset(offset)
            .execute_with_client(&client)?;
        let fetched = page.entities.len();
        groups.extend(page.entities.into_iter().filter_map(summarise));
        offset = offset.saturating_add(fetched as u16);
        if fetched == 0 || (offset as i32) >= page.count || offset == u16::MAX {
            break;
        }
        // MusicBrainz asks for one request a second.
        sleep(Duration::from_millis(1100));
    }
    // Newest first, undated last.
    groups.sort_by(|a, b| match (&a.year, &b.year) {
        (Some(x), Some(y)) => y.cmp(x).then_with(|| a.title.cmp(&b.title)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.title.cmp(&b.title),
    });
    Ok(groups)
}

/// Case- and punctuation-blind form of a title, for matching library albums to release groups.
pub fn fold_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A library album counts as this release group when the folded titles agree, or the album's
/// title extends the group's ("Album" against "Album (Deluxe Edition)").
pub fn library_has(folded_library_titles: &[String], group_title: &str) -> bool {
    let wanted = fold_title(group_title);
    if wanted.is_empty() {
        return true;
    }
    folded_library_titles
        .iter()
        .any(|have| have == &wanted || (wanted.len() >= 3 && have.starts_with(&wanted)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_ignores_case_and_punctuation() {
        assert_eq!(fold_title("In Rainbows"), "inrainbows");
        assert_eq!(fold_title("OK Computer: OKNOTOK 1997–2017"), "okcomputeroknotok19972017");
    }

    #[test]
    fn library_matches_exact_and_extended_titles() {
        let have = vec![fold_title("Kid A"), fold_title("Amnesiac (Deluxe Edition)")];
        assert!(library_has(&have, "Kid A"));
        assert!(library_has(&have, "Amnesiac"));
        assert!(!library_has(&have, "Hail to the Thief"));
        // Too short to count as a prefix match
        assert!(!library_has(&vec![fold_title("Kid A")], "Ki"));
    }
}
