//! LRU texture cache keyed by (cover, size). Decoding happens in the backend.

use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};

type Key = (String, u32);

pub struct TextureCache {
    capacity: usize,
    tick: u64,
    textures: HashMap<Key, (TextureHandle, u64)>,
    requested: HashSet<Key>,
}

impl TextureCache {
    pub fn new(capacity: usize) -> TextureCache {
        TextureCache { capacity, tick: 0, textures: HashMap::new(), requested: HashSet::new() }
    }

    /// The texture if cached; otherwise calls `request` once per key and returns `None`.
    pub fn get(
        &mut self,
        _ctx: &Context,
        cover: &str,
        size: u32,
        request: impl FnOnce(),
    ) -> Option<TextureHandle> {
        let key = (cover.to_string(), size);
        self.tick += 1;
        if let Some((tex, used)) = self.textures.get_mut(&key) {
            *used = self.tick;
            return Some(tex.clone());
        }
        if self.requested.insert(key) {
            request();
        }
        None
    }

    pub fn insert(&mut self, ctx: &Context, cover: String, size: u32, image: ColorImage) {
        let key = (cover, size);
        let tex = ctx.load_texture(format!("{}@{size}", key.0), image, TextureOptions::LINEAR);
        self.tick += 1;
        self.requested.insert(key.clone());
        self.textures.insert(key, (tex, self.tick));
        while self.textures.len() > self.capacity {
            // ponytail: O(n) scan per eviction; fine at capacity 512.
            let Some(oldest) = self.textures.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone())
            else {
                break;
            };
            self.textures.remove(&oldest);
            self.requested.remove(&oldest); // may be requested again
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img() -> ColorImage {
        ColorImage::filled([2, 2], egui::Color32::RED)
    }

    #[test]
    fn cache_requests_once_per_key() {
        let ctx = Context::default();
        let mut c = TextureCache::new(4);
        let mut n = 0;
        assert!(c.get(&ctx, "a", 160, || n += 1).is_none());
        assert!(c.get(&ctx, "a", 160, || n += 1).is_none());
        assert_eq!(n, 1);
        c.insert(&ctx, "a".into(), 160, img());
        assert!(c.get(&ctx, "a", 160, || n += 1).is_some());
        assert_eq!(n, 1);
    }

    #[test]
    fn cache_evicts_least_recently_used() {
        let ctx = Context::default();
        let mut c = TextureCache::new(2);
        c.insert(&ctx, "a".into(), 160, img());
        c.insert(&ctx, "b".into(), 160, img());
        assert!(c.get(&ctx, "a", 160, || {}).is_some());
        c.insert(&ctx, "c".into(), 160, img());
        assert!(c.get(&ctx, "b", 160, || {}).is_none());
        assert!(c.get(&ctx, "a", 160, || {}).is_some());
        assert!(c.get(&ctx, "c", 160, || {}).is_some());
    }
}
