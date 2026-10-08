//! Artwork: a TIDAL image uuid and the renditions its entity has, or a mix's full URLs.

use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageKind {
    Album,
    Artist,
    Playlist,
    PlaylistWide,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Image {
    Tidal {
        id: String,
        kind: ImageKind,
    },
    /// Mix artwork: 320, 640 and 1500 square, signed URLs.
    Urls {
        small: String,
        medium: String,
        large: String,
    },
}

impl ImageKind {
    /// Width x height, ascending: the only sizes the image server has (S4).
    fn sizes(self) -> &'static [(u32, u32)] {
        match self {
            Self::Album => &[(80, 80), (160, 160), (320, 320), (640, 640), (1280, 1280)],
            Self::Artist => &[(160, 160), (320, 320), (480, 480), (750, 750)],
            Self::Playlist => &[
                (160, 160),
                (320, 320),
                (480, 480),
                (640, 640),
                (750, 750),
                (1080, 1080),
            ],
            Self::PlaylistWide => &[
                (160, 107),
                (320, 214),
                (480, 320),
                (640, 428),
                (750, 500),
                (1080, 720),
            ],
        }
    }
}

impl Image {
    /// The smallest rendition at least `px` wide, else the largest.
    pub fn url(&self, image_base: &Url, px: u32) -> String {
        match self {
            Self::Tidal { id, kind } => {
                let sizes = kind.sizes();
                let (w, h) = sizes
                    .iter()
                    .find(|(w, _)| *w >= px)
                    .unwrap_or(&sizes[sizes.len() - 1]);
                format!("{image_base}{}/{w}x{h}.jpg", id.replace('-', "/"))
            }
            Self::Urls {
                small,
                medium,
                large,
            } => match px {
                0..=320 => small,
                321..=640 => medium,
                _ => large,
            }
            .clone(),
        }
    }

    /// Identifies the artwork whatever the size: the uuid, or the medium URL.
    pub fn key(&self) -> &str {
        match self {
            Self::Tidal { id, .. } => id,
            Self::Urls { medium, .. } => medium,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Image, ImageKind};
    use url::Url;

    fn base() -> Url {
        crate::ClientConfig::default().image_base
    }

    fn tidal(kind: ImageKind) -> Image {
        Image::Tidal {
            id: "ab12-cd34".into(),
            kind,
        }
    }

    /// The `WxH.jpg` file `url` picks.
    fn file(kind: ImageKind, px: u32) -> String {
        let url = tidal(kind).url(&base(), px);
        url.rsplit('/').next().unwrap_or_default().to_owned()
    }

    #[test]
    fn album_picks_smallest_rendition_at_least_px() {
        assert_eq!(file(ImageKind::Album, 200), "320x320.jpg");
        assert_eq!(file(ImageKind::Album, 2000), "1280x1280.jpg");
    }

    #[test]
    fn artist_sizes() {
        assert_eq!(file(ImageKind::Artist, 500), "750x750.jpg");
    }

    #[test]
    fn playlist_wide_is_w_by_h() {
        assert_eq!(file(ImageKind::PlaylistWide, 300), "320x214.jpg");
    }

    #[test]
    fn mix_urls_by_px() {
        let mix = Image::Urls {
            small: "s".into(),
            medium: "m".into(),
            large: "l".into(),
        };
        let at = |px| mix.url(&base(), px);
        assert_eq!([at(320), at(321), at(640), at(641)], ["s", "m", "m", "l"]);
        assert_eq!(mix.key(), "m");
    }

    #[test]
    fn uuid_dashes_become_slashes() {
        let album = tidal(ImageKind::Album);
        assert_eq!(
            album.url(&base(), 320),
            "https://resources.tidal.com/images/ab12/cd34/320x320.jpg"
        );
        assert_eq!(album.key(), "ab12-cd34");
    }
}
