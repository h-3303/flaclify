//! Free search of MusicBrainz for the "Get music" dialog: songs, albums and artists by name,
//! and the tracklist of a release group. MusicBrainz supplies the names only; flacli does the
//! fetching. Blocking: call from a worker thread, and leave a second between calls.

use musicbrainz_rs::{
    entity::{
        artist::Artist,
        artist_credit::ArtistCredit,
        recording::Recording,
        release::{Release, ReleaseStatus},
        release_group::{ReleaseGroup, ReleaseGroupPrimaryType},
    },
    error::Error as MBError,
    prelude::*,
};

use super::release_groups::{client, enabled};

/// How many results one search shows. MusicBrainz serves at most 100.
const SONG_LIMIT: u8 = 50;
const ALBUM_LIMIT: u8 = 40;
const ARTIST_LIMIT: u8 = 25;

#[derive(Debug, Clone)]
pub struct FoundSong {
    pub mbid: String,
    pub title: String,
    pub artist: String,
    /// The first release MusicBrainz lists the recording on.
    pub album: String,
    pub year: Option<String>,
    pub length_ms: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct FoundAlbum {
    pub mbid: String,
    pub title: String,
    pub artist: String,
    /// "Album", "EP", "Single", "Compilation", ...
    pub kind: String,
    pub year: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FoundArtist {
    pub mbid: String,
    pub name: String,
    /// Disambiguation, type and country, whichever MusicBrainz has.
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct FoundTrack {
    pub position: u32,
    pub title: String,
    pub artist: String,
    pub length_ms: Option<u32>,
}

impl FoundAlbum {
    /// "2019 · Album", as a row subtitle.
    pub fn describe(&self) -> String {
        match self.year.as_deref() {
            Some(year) => format!("{} · {year} · {}", self.artist, self.kind),
            None => format!("{} · {}", self.artist, self.kind),
        }
    }
}

/// "3:42" for a row suffix.
pub fn format_length(length_ms: Option<u32>) -> String {
    match length_ms {
        Some(ms) => {
            let secs = ms / 1000;
            format!("{}:{:02}", secs / 60, secs % 60)
        }
        None => String::new(),
    }
}

/// A search term as a quoted Lucene phrase. Only the quote and the backslash need escaping
/// inside a phrase.
fn phrase(term: &str) -> String {
    let mut out = String::with_capacity(term.len() + 2);
    out.push('"');
    for c in term.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// "Artist - Title" splits into its two halves; anything else is one term.
pub fn split_term(term: &str) -> (Option<String>, String) {
    for separator in [" - ", " – ", " — "] {
        if let Some((artist, rest)) = term.split_once(separator) {
            let (artist, rest) = (artist.trim(), rest.trim());
            if !artist.is_empty() && !rest.is_empty() {
                return (Some(artist.to_owned()), rest.to_owned());
            }
        }
    }
    (None, term.trim().to_owned())
}

fn year_of(date: Option<&str>) -> Option<String> {
    date.filter(|d| d.len() >= 4).map(|d| d[..4].to_owned())
}

/// The credited artist line, join phrases included ("A feat. B").
fn credit(credits: Option<&Vec<ArtistCredit>>) -> String {
    credits
        .map(|cs| {
            cs.iter()
                .map(|c| format!("{}{}", c.name, c.joinphrase.as_deref().unwrap_or("")))
                .collect::<String>()
        })
        .unwrap_or_default()
}

fn kind_of(group: &ReleaseGroup) -> String {
    let primary = match group.primary_type {
        Some(ReleaseGroupPrimaryType::Album) => "Album",
        Some(ReleaseGroupPrimaryType::Ep) => "EP",
        Some(ReleaseGroupPrimaryType::Single) => "Single",
        Some(ReleaseGroupPrimaryType::Broadcast) => "Broadcast",
        Some(_) => "Other",
        None => "Release",
    };
    if group.secondary_types.is_empty() {
        return primary.to_owned();
    }
    let secondary: Vec<String> = group.secondary_types.iter().map(|t| format!("{t:?}")).collect();
    format!("{primary} · {}", secondary.join(", "))
}

fn song_of(recording: Recording) -> FoundSong {
    // Prefer a release in an album-typed group, else the first listed.
    let release = recording.releases.as_ref().and_then(|releases| {
        releases
            .iter()
            .find(|r| {
                r.release_group
                    .as_ref()
                    .is_some_and(|g| g.primary_type == Some(ReleaseGroupPrimaryType::Album) && g.secondary_types.is_empty())
            })
            .or_else(|| releases.first())
    });
    let year = year_of(recording.first_release_date.as_ref().map(|d| d.0.as_str()))
        .or_else(|| year_of(release.and_then(|r| r.date.as_ref()).map(|d| d.0.as_str())));
    FoundSong {
        mbid: recording.id.clone(),
        title: recording.title.clone(),
        artist: credit(recording.artist_credit.as_ref()),
        album: release.map(|r| r.title.clone()).unwrap_or_default(),
        year,
        length_ms: recording.length,
    }
}

fn album_of(group: ReleaseGroup) -> FoundAlbum {
    FoundAlbum {
        mbid: group.id.clone(),
        title: group.title.clone(),
        artist: credit(group.artist_credit.as_ref()),
        kind: kind_of(&group),
        year: year_of(group.first_release_date.as_ref().map(|d| d.0.as_str())),
    }
}

fn artist_of(artist: Artist) -> FoundArtist {
    let mut bits: Vec<String> = Vec::new();
    if !artist.disambiguation.is_empty() {
        bits.push(artist.disambiguation.clone());
    }
    if let Some(kind) = artist.artist_type.as_ref() {
        bits.push(format!("{kind:?}"));
    }
    if let Some(area) = artist.area.as_ref() {
        bits.push(area.name.clone());
    }
    FoundArtist {
        mbid: artist.id,
        name: artist.name,
        note: bits.join(" · "),
    }
}

/// Songs by title, or by the artist's name, or "Artist - Title" for both.
pub fn search_songs(term: &str) -> Result<Vec<FoundSong>, MBError> {
    if !enabled() {
        return Ok(Vec::new());
    }
    let lucene = match split_term(term) {
        (Some(artist), title) => format!("artist:{} AND recording:{}", phrase(&artist), phrase(&title)),
        // The artist clause is boosted: a band's name should list the band's songs before
        // other people's songs that happen to bear that name.
        (None, whole) => format!("(artist:{0}^3 OR recording:{0})", phrase(&whole)),
    };
    println!("[MusicBrainz] Searching recordings: {lucene}");
    let found = Recording::search(format!("query={lucene}"))
        .limit(SONG_LIMIT)
        .execute_with_client(&client())?;
    Ok(found.entities.into_iter().map(song_of).collect())
}

/// Albums (release groups) by title, or by the artist's name, or "Artist - Album" for both.
pub fn search_albums(term: &str) -> Result<Vec<FoundAlbum>, MBError> {
    if !enabled() {
        return Ok(Vec::new());
    }
    let lucene = match split_term(term) {
        (Some(artist), title) => format!("artist:{} AND releasegroup:{}", phrase(&artist), phrase(&title)),
        (None, whole) => format!("(artist:{0}^3 OR releasegroup:{0})", phrase(&whole)),
    };
    println!("[MusicBrainz] Searching release groups: {lucene}");
    let found = ReleaseGroup::search(format!("query={lucene}"))
        .limit(ALBUM_LIMIT)
        .execute_with_client(&client())?;
    Ok(found.entities.into_iter().map(album_of).collect())
}

/// Artists by name or alias. For "Artist - Title" only the artist half is used.
pub fn search_artists(term: &str) -> Result<Vec<FoundArtist>, MBError> {
    if !enabled() {
        return Ok(Vec::new());
    }
    let name = match split_term(term) {
        (Some(artist), _) => artist,
        (None, whole) => whole,
    };
    let lucene = format!("(artist:{0} OR alias:{0})", phrase(&name));
    println!("[MusicBrainz] Searching artists: {lucene}");
    let found = Artist::search(format!("query={lucene}"))
        .limit(ARTIST_LIMIT)
        .execute_with_client(&client())?;
    Ok(found.entities.into_iter().map(artist_of).collect())
}

/// The tracklist of a release group, from its first official release (earliest date), or the
/// first release of any status when none is marked official. `artist` fills in for tracks
/// without their own credit.
pub fn release_group_tracks(group_mbid: &str, artist: &str) -> Result<Vec<FoundTrack>, MBError> {
    if !enabled() {
        return Ok(Vec::new());
    }
    println!("[MusicBrainz] Browsing releases of release group {group_mbid}");
    let page = Release::browse()
        .by_release_group(group_mbid)
        .with_recordings()
        .with_artist_credits()
        .limit(100)
        .execute_with_client(&client())?;
    let mut releases: Vec<Release> = page.entities;
    if releases.is_empty() {
        return Ok(Vec::new());
    }
    // Official first, then by date, undated last.
    releases.sort_by(|a, b| {
        let official = |r: &Release| r.status != Some(ReleaseStatus::Official);
        official(a).cmp(&official(b)).then_with(|| match (&a.date, &b.date) {
            (Some(x), Some(y)) => x.0.cmp(&y.0),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        })
    });
    let release = releases.swap_remove(0);
    let mut tracks: Vec<FoundTrack> = Vec::new();
    let mut position: u32 = 0;
    for medium in release.media.unwrap_or_default() {
        for track in medium.tracks.unwrap_or_default() {
            position += 1;
            let own_credit = credit(track.artist_credit.as_ref());
            let credited = if own_credit.is_empty() {
                credit(track.recording.as_ref().and_then(|r| r.artist_credit.as_ref()))
            } else {
                own_credit
            };
            tracks.push(FoundTrack {
                position,
                title: track.title,
                artist: if credited.is_empty() { artist.to_owned() } else { credited },
                length_ms: track.length.or_else(|| track.recording.as_ref().and_then(|r| r.length)),
            });
        }
    }
    Ok(tracks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrases_escape_quotes() {
        assert_eq!(phrase(r#"Say "Hi""#), r#""Say \"Hi\"""#);
    }

    #[test]
    fn artist_title_splits_once() {
        assert_eq!(
            split_term("Black Box Recorder - Child Psychology"),
            (Some("Black Box Recorder".to_owned()), "Child Psychology".to_owned())
        );
        assert_eq!(split_term("black box recorder"), (None, "black box recorder".to_owned()));
        assert_eq!(split_term(" - x"), (None, "- x".to_owned()));
    }

    #[test]
    fn lengths_format_as_minutes() {
        assert_eq!(format_length(Some(222_000)), "3:42");
        assert_eq!(format_length(Some(59_999)), "0:59");
        assert_eq!(format_length(None), "");
    }
}
