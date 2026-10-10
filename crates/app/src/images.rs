//! Artwork at display size (spec §6, §10): the backend fetches a rendition through the images
//! disk cache, decodes it and scales it to the width it is drawn at; the UI holds the
//! textures within a byte budget.

use crate::backend::Emitter;
use crate::model::Event;
use crate::theme::{TINT_PX, dominant};
use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};
use image::imageops::{self, FilterType};
use reqwest::Url;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tidal::{DiskCache, Entry};
use tokio::sync::Semaphore;

/// How long artwork that failed to load keeps its placeholder.
const RETRY: Duration = Duration::from_secs(30);

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// TIDAL's renditions as downloaded, keyed by URL; kept across logouts (artwork is public).
/// Opening sweeps the directory, so the backend thread does it; `shared` then hands it to the
/// media controls. `None` without a cache dir, or if it fails to open.
pub fn open_cache(shared: &OnceLock<Arc<DiskCache>>) -> Option<Arc<DiskCache>> {
    let dir = crate::backend::cache_dir()?.join("images");
    let cache = DiskCache::open(dir, 256 << 20, "jpg")
        .inspect_err(|e| log::warn!("no image cache: {e}"))
        .ok()
        .map(Arc::new)?;
    let _ = shared.set(Arc::clone(&cache)); // opened once
    Some(cache)
}

/// The backend's artwork loads: at most 6 fetch or decode at once.
pub struct Artwork {
    cache: Option<Arc<DiskCache>>,
    permits: Arc<Semaphore>,
}

impl Artwork {
    pub fn new(cache: Option<Arc<DiskCache>>) -> Self {
        Self {
            cache,
            permits: Arc::new(Semaphore::new(6)),
        }
    }

    /// Loads `image` at most `px` wide on a task of its own: `Event::Image`, or after `RETRY`
    /// `Event::ImageFailed`, so that unavailable artwork is asked for again when drawn, not
    /// every frame.
    pub fn load(
        &self,
        http: &reqwest::Client,
        base: &Url,
        image: &tidal::Image,
        px: u32,
        emit: Arc<Emitter>,
    ) {
        let (url, key) = (image.url(base, px), image.key().to_owned());
        let (http, cache, permits) = (http.clone(), self.cache.clone(), self.permits.clone());
        tokio::spawn(async move {
            let loaded = async {
                let _permit = permits.acquire().await?;
                fetch(&http, cache, url.clone(), px).await
            };
            match loaded.await {
                Ok(image) => emit.send(Event::Image { key, px, image }),
                Err(e) => {
                    log::debug!("image {url}: {e}"); // the placeholder stays
                    tokio::time::sleep(RETRY).await;
                    emit.send(Event::ImageFailed { key, px });
                }
            }
        });
    }
}

/// `url`'s image at most `px` wide. TIDAL's bytes come from `cache`, else from the network,
/// and are saved there once they decode: any size that picks this rendition reuses them.
async fn fetch(
    http: &reqwest::Client,
    cache: Option<Arc<DiskCache>>,
    url: String,
    px: u32,
) -> Result<ColorImage, BoxError> {
    let saved = match cache.clone() {
        Some(c) => {
            let key = url.clone();
            tokio::task::spawn_blocking(move || c.read(&key)).await?
        }
        None => None,
    };
    let (bytes, fresh) = match saved {
        Some(e) => (e.body, false),
        None => {
            let body = http.get(&url).send().await?.error_for_status()?;
            (body.bytes().await?.to_vec(), true)
        }
    };
    tokio::task::spawn_blocking(move || {
        let image = decode(&bytes, px)?;
        if let Some(c) = cache.filter(|_| fresh) {
            let entry = Entry {
                etag: None,
                body: bytes,
            };
            if let Err(e) = c.write(&url, &entry) {
                log::debug!("image {url} not saved: {e}"); // downloaded again next time
            }
        }
        Ok(image)
    })
    .await?
}

/// Decodes a JPEG, scaled down to `px` wide (aspect kept) if wider.
fn decode(bytes: &[u8], px: u32) -> image::ImageResult<ColorImage> {
    let mut img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)?.to_rgba8();
    let (w, h) = img.dimensions();
    if w > px {
        let scaled = (f64::from(h) * f64::from(px) / f64::from(w)).round() as u32;
        img = imageops::resize(&img, px, scaled.max(1), FilterType::Triangle);
    }
    let size = [img.width() as usize, img.height() as usize];
    Ok(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

type Key = (String, u32);

/// The UI's artwork textures, keyed by (`Image::key`, px), within a byte budget.
pub struct TextureCache {
    budget: usize,
    /// What the textures held take, `w * h * 4` each.
    bytes: usize,
    /// Each texture, its bytes, and the pass it was last drawn in.
    textures: HashMap<Key, (TextureHandle, usize, u64)>,
    requested: HashSet<Key>,
    /// Each image's `dominant` colour, taken from its `TINT_PX` texture and dropped with it.
    tints: HashMap<String, Option<Color32>>,
}

impl TextureCache {
    pub fn new(budget_bytes: usize) -> TextureCache {
        TextureCache {
            budget: budget_bytes,
            bytes: 0,
            textures: HashMap::new(),
            requested: HashSet::new(),
            tints: HashMap::new(),
        }
    }

    /// The texture, now drawn in this pass, if cached. Otherwise calls `request` once per key
    /// and meanwhile returns the cached size of the same image nearest `px`, now drawn in this
    /// pass too; `None` with no size of it cached.
    pub fn get(
        &mut self,
        ctx: &Context,
        image: &str,
        px: u32,
        request: impl FnOnce(),
    ) -> Option<TextureHandle> {
        if let Some(tex) = self.get_exact(ctx, image, px, request) {
            return Some(tex);
        }
        // ponytail: O(n) scan, only while a size loads; n is a few hundred textures.
        let (_, (tex, _, used)) = self
            .textures
            .iter_mut()
            .filter(|((k, _), _)| k == image)
            .min_by_key(|((_, at), _)| at.abs_diff(px))?;
        *used = ctx.cumulative_pass_nr();
        Some(tex.clone())
    }

    /// As `get`, but only the texture `px` wide, never another size meanwhile: for the page
    /// backdrop and the tint, which must not show a sharp cover (ruling 20).
    pub fn get_exact(
        &mut self,
        ctx: &Context,
        image: &str,
        px: u32,
        request: impl FnOnce(),
    ) -> Option<TextureHandle> {
        let key = (image.to_owned(), px);
        if let Some((tex, _, used)) = self.textures.get_mut(&key) {
            *used = ctx.cumulative_pass_nr();
            return Some(tex.clone());
        }
        if self.requested.insert(key) {
            request();
        }
        None
    }

    /// The artwork could not be loaded: the next `get` requests it again.
    pub fn forget(&mut self, key: String, px: u32) {
        self.requested.remove(&(key, px));
    }

    /// `image`'s dominant colour, while its `TINT_PX` texture is cached (spec §5.9).
    pub fn tint(&self, image: &str) -> Option<Color32> {
        self.tints.get(image).copied().flatten()
    }

    /// Some requested artwork has no texture yet: it is on its way, or failed and waits out
    /// `RETRY`.
    pub fn loading(&self) -> bool {
        self.requested
            .iter()
            .any(|k| !self.textures.contains_key(k))
    }

    /// Adds a texture, then drops the least recently drawn beyond the budget, never one drawn
    /// in this pass or the last: the app inserts in `logic`, before this pass draws, so the
    /// last pass's are what is on screen. A screenful larger than the budget stays over it
    /// until scrolled away.
    pub fn insert(&mut self, ctx: &Context, key: String, px: u32, image: ColorImage) {
        let size = image.size[0] * image.size[1] * 4;
        if px == TINT_PX {
            self.tints.insert(key.clone(), dominant(&image));
        }
        let tex = ctx.load_texture(format!("{key}@{px}"), image, TextureOptions::LINEAR);
        let now = ctx.cumulative_pass_nr();
        let key = (key, px);
        self.requested.insert(key.clone());
        self.bytes += size;
        if let Some((_, old, _)) = self.textures.insert(key, (tex, size, now)) {
            self.bytes -= old;
        }
        while self.bytes > self.budget {
            // ponytail: O(n) scan per eviction; n is a few hundred textures.
            let oldest = self
                .textures
                .iter()
                .filter(|(_, (_, _, used))| *used + 1 < now)
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(k, _)| k.clone());
            let Some(oldest) = oldest else { break };
            if let Some((_, size, _)) = self.textures.remove(&oldest) {
                self.bytes -= size;
            }
            if oldest.1 == TINT_PX {
                self.tints.remove(&oldest.0);
            }
            self.requested.remove(&oldest); // may be requested again
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// 2 × 2 × 4 = 16 bytes.
    fn img() -> ColorImage {
        ColorImage::filled([2, 2], egui::Color32::RED)
    }

    /// Ends a pass: what is used after it is used in a later one.
    fn next_pass(ctx: &Context) {
        let mut out = ctx.run_ui(Default::default(), |_| {});
        out.textures_delta.clear(); // nothing renders them
    }

    /// A `w` × `h` JPEG.
    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 40]));
        image::codecs::jpeg::JpegEncoder::new(&mut out)
            .encode_image(&img)
            .expect("encoded");
        out
    }

    #[test]
    fn texture_budget_evicts_lru_bytes() {
        let ctx = Context::default();
        let mut c = TextureCache::new(3 * 16);
        for k in ["a", "b", "c"] {
            c.insert(&ctx, k.into(), 160, img());
            next_pass(&ctx);
        }
        assert!(c.get(&ctx, "a", 160, || {}).is_some()); // now b is the least recent
        c.insert(&ctx, "d".into(), 160, img());
        assert!(c.get(&ctx, "b", 160, || {}).is_none(), "evicted");
        for k in ["a", "c", "d"] {
            assert!(c.get(&ctx, k, 160, || {}).is_some(), "{k}");
        }
        assert_eq!(c.bytes, 3 * 16);
    }

    #[test]
    fn textures_used_this_pass_are_kept() {
        let ctx = Context::default();
        let mut c = TextureCache::new(3 * 16);
        for k in ["a", "b", "c", "d"] {
            c.insert(&ctx, k.into(), 160, img());
        }
        assert_eq!(c.bytes, 4 * 16, "all drawn this pass: over budget");
        for k in ["a", "b", "c", "d"] {
            assert!(c.get(&ctx, k, 160, || {}).is_some(), "{k}");
        }
        next_pass(&ctx); // still on screen from that pass
        next_pass(&ctx); // not drawn in it
        c.insert(&ctx, "e".into(), 160, img());
        assert_eq!(c.bytes, 3 * 16, "within budget once off screen");
    }

    /// As in the app: `insert` runs in `logic`, before this pass draws; what the last pass drew
    /// is still on screen.
    #[test]
    fn textures_drawn_last_pass_are_kept() {
        let ctx = Context::default();
        let mut c = TextureCache::new(3 * 16);
        for k in ["a", "b", "c"] {
            c.insert(&ctx, k.into(), 160, img());
        }
        next_pass(&ctx);
        for k in ["a", "b", "c"] {
            assert!(c.get(&ctx, k, 160, || {}).is_some(), "{k}");
        }
        next_pass(&ctx);
        c.insert(&ctx, "d".into(), 160, img());
        assert_eq!(
            c.bytes,
            4 * 16,
            "the last pass's are on screen: over budget"
        );
    }

    #[test]
    fn downscale_to_px() {
        let size = |w, h, px| decode(&jpeg(w, h), px).expect("decoded").size;
        assert_eq!(size(320, 320, 200), [200, 200]);
        assert_eq!(size(320, 320, 640), [320, 320], "never scaled up");
        assert_eq!(size(320, 214, 200), [200, 134], "aspect kept");
    }

    /// Two sizes served by one rendition: one download, the second load reads the disk.
    #[tokio::test]
    async fn disk_cache_hit_skips_network() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ab12/cd34/320x320.jpg"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(jpeg(320, 320), "image/jpeg"))
            .expect(1)
            .mount(&server)
            .await;
        let dir = std::env::temp_dir().join("tidash-images-hit");
        let _ = std::fs::remove_dir_all(&dir);
        let cache = Arc::new(DiskCache::open(dir, 1 << 20, "jpg").expect("a cache"));
        let base: Url = format!("{}/", server.uri()).parse().expect("a URL");
        let art = tidal::Image::Tidal {
            id: "ab12-cd34".into(),
            kind: tidal::ImageKind::Album,
        };
        let http = reqwest::Client::new();
        for px in [200, 300] {
            let url = art.url(&base, px);
            let image = fetch(&http, Some(cache.clone()), url, px).await;
            assert_eq!(image.expect("loaded").size, [px as usize; 2]);
        }
        server.verify().await;
    }

    #[test]
    fn cache_requests_once_per_key() {
        let ctx = Context::default();
        let mut c = TextureCache::new(1 << 20);
        let mut n = 0;
        assert!(c.get(&ctx, "a", 160, || n += 1).is_none());
        assert!(c.get(&ctx, "a", 160, || n += 1).is_none());
        assert_eq!(n, 1);
        c.insert(&ctx, "a".into(), 160, img());
        assert!(c.get(&ctx, "a", 160, || n += 1).is_some());
        assert_eq!(n, 1);
    }

    /// While a new size loads, the nearest cached one of the same image stands in.
    #[test]
    fn a_missing_size_shows_the_nearest_cached_one() {
        let ctx = Context::default();
        let mut c = TextureCache::new(1 << 20);
        c.insert(&ctx, "a".into(), 64, img());
        c.insert(&ctx, "a".into(), 128, img());
        let near = c.get(&ctx, "a", 128, || {}).map(|t| t.id());
        let mut n = 0;
        for _ in 0..2 {
            let shown = c.get(&ctx, "a", 192, || n += 1).map(|t| t.id());
            assert_eq!(shown, near, "128 px meanwhile");
        }
        assert_eq!(n, 1, "192 px asked for once");
        assert!(c.get(&ctx, "b", 192, || {}).is_none(), "no size of b");
        c.insert(&ctx, "a".into(), 192, img());
        let exact = c.get(&ctx, "a", 192, || n += 1).map(|t| t.id());
        assert!(exact.is_some() && exact != near, "the 192 px one");
        assert_eq!(n, 1);
    }

    /// Ruling 20: the exact lookup never stands another size in, but still asks once.
    #[test]
    fn the_exact_size_or_nothing() {
        let ctx = Context::default();
        let mut c = TextureCache::new(1 << 20);
        c.insert(&ctx, "a".into(), 256, img());
        let mut n = 0;
        for _ in 0..2 {
            assert!(
                c.get_exact(&ctx, "a", 16, || n += 1).is_none(),
                "not 256 px"
            );
        }
        assert_eq!(n, 1, "16 px asked for once");
        c.insert(&ctx, "a".into(), 16, img());
        assert!(c.get_exact(&ctx, "a", 16, || n += 1).is_some());
        assert_eq!(n, 1);
    }

    /// Spec §5.9: the `TINT_PX` texture's dominant colour is kept by its key and goes with it;
    /// no other size gives one.
    #[test]
    fn a_small_image_keeps_its_tint() {
        let ctx = Context::default();
        let px = TINT_PX;
        let red = |w: u32| ColorImage::filled([w as usize; 2], Color32::RED);
        let mut c = TextureCache::new(64 * 64 * 4); // only b's 64 px
        c.insert(&ctx, "a".into(), px, red(px));
        assert_eq!(c.tint("a"), Some(Color32::RED));
        next_pass(&ctx);
        next_pass(&ctx); // a is off screen
        c.insert(&ctx, "b".into(), 64, red(64));
        assert_eq!(c.tint("b"), None, "not the tint's size");
        assert!(c.get_exact(&ctx, "a", px, || {}).is_none(), "a evicted");
        assert_eq!(c.tint("a"), None, "gone with its texture");
    }

    #[test]
    fn a_failed_cover_is_requested_again() {
        let ctx = Context::default();
        let mut c = TextureCache::new(1 << 20);
        let mut n = 0;
        c.get(&ctx, "a", 160, || n += 1);
        c.forget("a".into(), 160);
        c.get(&ctx, "a", 160, || n += 1);
        assert_eq!(n, 2);
    }

    #[test]
    fn loading_until_inserted_or_forgotten() {
        let ctx = Context::default();
        let mut c = TextureCache::new(1 << 20);
        assert!(!c.loading(), "nothing asked for");
        c.get(&ctx, "a", 160, || {});
        assert!(c.loading(), "requested");
        c.insert(&ctx, "a".into(), 160, img());
        assert!(!c.loading(), "inserted");
        c.get(&ctx, "b", 160, || {});
        assert!(c.loading(), "requested");
        c.forget("b".into(), 160);
        assert!(!c.loading(), "forgotten");
    }
}
