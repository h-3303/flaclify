use gtk::gdk::Texture;
use once_cell::sync::Lazy;

pub static ALBUMART_PLACEHOLDER: Lazy<Texture> = Lazy::new(|| {
    Texture::from_resource("/io/github/h3303/Flaclify/albumart-placeholder.svg")
});

pub static ALBUMART_THUMBNAIL_PLACEHOLDER: Lazy<Texture> = Lazy::new(|| {
    Texture::from_resource("/io/github/h3303/Flaclify/albumart-placeholder-thumb.png")
});

pub static EMPTY_ALBUM_STRING: Lazy<&str> = Lazy::new(|| "(untitled album)");
pub static EMPTY_ARTIST_STRING: Lazy<&str> = Lazy::new(|| "(unknown artist)");
