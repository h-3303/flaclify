//! Text and pictures kept beside the music, as flacli writes them:
//! `<Artist>/artist.md`, `<Artist>/<Album>/wiki.md`, `.wiki/<Artist>.md` for an artist without a
//! folder of their own, and `<Artist>/artist.{jpg,jpeg,png,webp}`.
//!
//! Not a link in the provider chain: the cache controller reads these before it asks anything
//! remote, and again whenever a file is newer than the cached copy, so an edit on disk shows on
//! the next visit without the app touching its own database from outside. The text is a small
//! front-matter block (`key: value` lines between `---` rules) and plain prose below it.

use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use gtk::glib;
use gtk::prelude::*;
use time::OffsetDateTime;

use super::models::{AlbumMeta, ArtistMeta, ImageMeta, ImageSize, Wiki};
use crate::{
    common::{AlbumInfo, ArtistInfo},
    utils::meta_provider_settings,
};

pub static PROVIDER_KEY: &str = "local";
const ARTIST_FILE: &str = "artist.md";
const ALBUM_FILE: &str = "wiki.md";
const LOOSE_DIR: &str = ".wiki";
const PICTURE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

pub fn enabled() -> bool {
    meta_provider_settings(PROVIDER_KEY).boolean("enabled")
}

/// Where the library lives on this machine: the setting, else the XDG music folder.
pub fn music_directory() -> Option<PathBuf> {
    let configured = meta_provider_settings(PROVIDER_KEY).string("music-directory");
    let configured = configured.trim();
    if configured.is_empty() {
        return glib::user_special_dir(glib::UserDirectory::Music);
    }
    if let Some(rest) = configured.strip_prefix("~/") {
        return Some(glib::home_dir().join(rest));
    }
    Some(PathBuf::from(configured))
}

/// Lower-case, runs of anything but letters and digits collapsed to one space (flacli's `_fold`).
fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(ch.to_lowercase());
        } else {
            pending_space = true;
        }
    }
    out
}

/// A name as a folder name, the way flacli files it.
fn safe_name(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 0x20 {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim_matches(|c| c == ' ' || c == '.');
    if trimmed.is_empty() {
        "_".to_owned()
    } else {
        trimmed.to_owned()
    }
}

pub struct Sidecar {
    pub wiki: Wiki,
    pub modified: OffsetDateTime,
}

fn parse(text: &str) -> Option<Wiki> {
    let mut attribution = String::new();
    let mut url = None;
    let mut body = text;
    if let Some(rest) = text.strip_prefix("---\n")
        && let Some(end) = rest.find("\n---\n")
    {
        for line in rest[..end].lines() {
            if let Some((key, value)) = line.split_once(':') {
                match key.trim() {
                    "attribution" => attribution = value.trim().to_owned(),
                    "url" => {
                        let value = value.trim();
                        if !value.is_empty() {
                            url = Some(value.to_owned());
                        }
                    }
                    _ => {}
                }
            }
        }
        body = &rest[end + 5..];
    }
    let content = body.trim();
    if content.is_empty() {
        return None;
    }
    if attribution.is_empty() {
        attribution = "Local file".to_owned();
    }
    let mut wiki = Wiki::default();
    wiki.content = content.to_owned();
    wiki.url = url;
    wiki.attribution = attribution;
    Some(wiki)
}

fn read(path: &Path) -> Option<Sidecar> {
    let text = fs::read_to_string(path).ok()?;
    let modified = fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Some(Sidecar {
        wiki: parse(&text)?,
        modified: OffsetDateTime::from(modified),
    })
}

/// `<root>/<album folder>/wiki.md`; the folder URI is MPD's, relative to its music directory.
pub fn album_sidecar(album: &AlbumInfo) -> Option<PathBuf> {
    let root = music_directory()?;
    let folder = album.folder_uri.trim_matches('/');
    let path = root.join(folder).join(ALBUM_FILE);
    path.is_file().then_some(path)
}

/// The artist's own folder: the top-level folder of one of their songs when its name is theirs,
/// else `<root>/<name>` if that exists.
pub fn artist_folder(artist: &ArtistInfo) -> Option<PathBuf> {
    let root = music_directory()?;
    let wanted = fold(&artist.name);
    for uri in artist.example_uris.iter() {
        let top = uri.split('/').next().unwrap_or("");
        if !top.is_empty() && fold(top) == wanted {
            return Some(root.join(top));
        }
    }
    let direct = root.join(safe_name(&artist.name));
    direct.is_dir().then_some(direct)
}

pub fn artist_sidecar(artist: &ArtistInfo) -> Option<PathBuf> {
    if let Some(folder) = artist_folder(artist) {
        let path = folder.join(ARTIST_FILE);
        if path.is_file() {
            return Some(path);
        }
    }
    let loose = music_directory()?
        .join(LOOSE_DIR)
        .join(format!("{}.md", safe_name(&artist.name)));
    loose.is_file().then_some(loose)
}

pub fn artist_picture(artist: &ArtistInfo) -> Option<PathBuf> {
    let folder = artist_folder(artist)?;
    PICTURE_EXTS
        .iter()
        .map(|ext| folder.join(format!("artist.{ext}")))
        .find(|p| p.is_file())
}

pub fn picture_meta(path: &Path) -> ImageMeta {
    ImageMeta {
        size: ImageSize::Mega,
        url: format!("file://{}", path.display()),
    }
}

/// The album's text from disk, with the file's modification time, if there is one.
pub fn album_meta(album: &AlbumInfo) -> Option<(AlbumMeta, OffsetDateTime)> {
    let sidecar = read(&album_sidecar(album)?)?;
    let mut meta = AlbumMeta::from_key(album);
    meta.artist = album.albumartist.clone();
    meta.url = sidecar.wiki.url.clone();
    meta.wiki = Some(sidecar.wiki);
    Some((meta, sidecar.modified))
}

/// The artist's bio from disk (and their picture beside it), with the newest file time, if any.
pub fn artist_meta(artist: &ArtistInfo) -> Option<(ArtistMeta, OffsetDateTime)> {
    let text = artist_sidecar(artist).and_then(|p| read(&p));
    let picture = artist_picture(artist);
    if text.is_none() && picture.is_none() {
        return None;
    }
    let mut meta = ArtistMeta::from_key(artist);
    let mut modified = OffsetDateTime::UNIX_EPOCH;
    if let Some(sidecar) = text {
        meta.url = sidecar.wiki.url.clone();
        meta.bio = Some(sidecar.wiki);
        modified = sidecar.modified;
    }
    if let Some(picture) = picture {
        if let Ok(m) = fs::metadata(&picture).and_then(|m| m.modified()) {
            modified = modified.max(OffsetDateTime::from(m));
        }
        meta.image.push(picture_meta(&picture));
    }
    Some((meta, modified))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_like_flacli() {
        assert_eq!(fold("The Cardigans"), "the cardigans");
        assert_eq!(fold("Sigur Rós!"), "sigur rós");
        assert_eq!(fold("  AC/DC  "), "ac dc");
    }

    #[test]
    fn safe_names_like_flacli() {
        assert_eq!(safe_name("AC/DC"), "AC_DC");
        assert_eq!(safe_name(" Who? "), "Who_");
        assert_eq!(safe_name("..."), "_");
    }

    #[test]
    fn parses_front_matter() {
        let wiki = parse("---\nkind: album\nattribution: Wikipedia contributors, CC BY-SA 4.0\nurl: https://x\n---\n\nSome text.\n").unwrap();
        assert_eq!(wiki.content, "Some text.");
        assert_eq!(wiki.attribution, "Wikipedia contributors, CC BY-SA 4.0");
        assert_eq!(wiki.url.as_deref(), Some("https://x"));
        let bare = parse("Just prose.").unwrap();
        assert_eq!(bare.attribution, "Local file");
        assert!(parse("---\nkind: album\n---\n\n\n").is_none());
    }
}
