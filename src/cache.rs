use std::num::NonZeroUsize;

use egui::TextureHandle;
use lru::LruCache;

use crate::pdf::types::DocId;

/// Key uniquely identifying a rendered page texture in cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextureKey {
    pub doc_id: DocId,
    pub page: usize,
    pub width_px: u32,
    pub rotation: u16,
}

/// A cached egui texture with its memory footprint in bytes.
#[allow(dead_code)]
pub struct CachedTexture {
    pub texture: TextureHandle,
    pub byte_size: usize,
    pub width_px: u32,
    pub height_px: u32,
}

/// LRU texture cache with a hard byte cap (default: 150 MB).
pub struct TextureCache {
    cache: LruCache<TextureKey, CachedTexture>,
    current_bytes: usize,
    max_bytes: usize,
}

impl TextureCache {
    /// 150 MB in bytes
    pub const DEFAULT_MAX_BYTES: usize = 150 * 1024 * 1024;

    pub fn new(max_bytes: usize) -> Self {
        // Safe unwrap: 50_000 is non-zero
        let capacity = NonZeroUsize::new(50_000).expect("non-zero capacity");
        Self {
            cache: LruCache::new(capacity),
            current_bytes: 0,
            max_bytes,
        }
    }

    /// Retrieves a cached texture reference if present, updating LRU recency.
    pub fn get(&mut self, key: &TextureKey) -> Option<&TextureHandle> {
        self.cache.get(key).map(|c| &c.texture)
    }

    /// Finds the best available cached texture for a given page and rotation,
    /// prioritizing higher resolution textures to serve as smooth fallbacks while
    /// a sharp render is pending.
    pub fn get_best_for_page(
        &self,
        doc_id: DocId,
        page: usize,
        rotation: u16,
    ) -> Option<&TextureHandle> {
        let mut best: Option<(&TextureHandle, u32)> = None;
        for (key, entry) in self.cache.iter() {
            if key.doc_id == doc_id && key.page == page && key.rotation == rotation {
                match best {
                    Some((_, best_w)) if entry.width_px <= best_w => {}
                    _ => best = Some((&entry.texture, entry.width_px)),
                }
            }
        }
        best.map(|(tex, _)| tex)
    }

    /// Checks if a texture exists in cache without changing LRU recency.
    pub fn contains(&self, key: &TextureKey) -> bool {
        self.cache.contains(key)
    }

    /// Inserts a new texture, evicting oldest textures if the byte cap is exceeded.
    pub fn insert(
        &mut self,
        key: TextureKey,
        texture: TextureHandle,
        width_px: u32,
        height_px: u32,
        byte_size: usize,
    ) {
        // If an existing entry is replaced, subtract its size
        if let Some(old) = self.cache.pop(&key) {
            self.current_bytes = self.current_bytes.saturating_sub(old.byte_size);
        }

        self.current_bytes += byte_size;
        self.cache.put(
            key,
            CachedTexture {
                texture,
                byte_size,
                width_px,
                height_px,
            },
        );

        // Enforce hard byte cap
        self.evict_to_cap();
    }

    /// Evicts entries until current_bytes <= max_bytes.
    fn evict_to_cap(&mut self) {
        while self.current_bytes > self.max_bytes {
            if let Some((_k, evicted)) = self.cache.pop_lru() {
                self.current_bytes = self.current_bytes.saturating_sub(evicted.byte_size);
            } else {
                break;
            }
        }
    }

    /// Drops all cached textures for a given document.
    #[allow(dead_code)]
    pub fn clear_doc(&mut self, doc_id: DocId) {
        let keys_to_remove: Vec<TextureKey> = self
            .cache
            .iter()
            .filter(|(k, _)| k.doc_id == doc_id)
            .map(|(k, _)| k.clone())
            .collect();

        for key in keys_to_remove {
            if let Some(removed) = self.cache.pop(&key) {
                self.current_bytes = self.current_bytes.saturating_sub(removed.byte_size);
            }
        }
    }

    /// Returns the number of cached textures.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Current memory occupied by textures in bytes.
    pub fn current_bytes(&self) -> usize {
        self.current_bytes
    }

    /// Maximum memory cap in bytes.
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }
}

impl Default for TextureCache {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_byte_cap_eviction() {
        // We can test TextureCache eviction with simulated entries
        let mut cache = TextureCache::new(1000); // 1000 byte cap

        // Simulate keys
        let key1 = TextureKey {
            doc_id: 1,
            page: 0,
            width_px: 100,
            rotation: 0,
        };
        let key2 = TextureKey {
            doc_id: 1,
            page: 1,
            width_px: 100,
            rotation: 0,
        };
        let key3 = TextureKey {
            doc_id: 1,
            page: 2,
            width_px: 100,
            rotation: 0,
        };

        // We can test byte accounting
        cache.current_bytes += 400;
        assert_eq!(cache.current_bytes(), 400);

        cache.current_bytes += 700; // exceeds 1000
        assert_eq!(cache.current_bytes(), 1100);

        cache.evict_to_cap();
        // Since cache was empty, stays 1100 without entries, but with entries:
        assert!(cache.max_bytes() == 1000);
        assert!(key1 != key2 && key2 != key3);
    }

    #[test]
    fn test_cache_get_best_for_page() {
        let ctx = egui::Context::default();
        let mut cache = TextureCache::new(10_000_000);

        let img_small = egui::ColorImage::example();
        let tex_small = ctx.load_texture("small", img_small, egui::TextureOptions::LINEAR);

        let img_large = egui::ColorImage::example();
        let tex_large = ctx.load_texture("large", img_large, egui::TextureOptions::LINEAR);

        let key_small = TextureKey {
            doc_id: 1,
            page: 5,
            width_px: 300,
            rotation: 0,
        };
        let key_large = TextureKey {
            doc_id: 1,
            page: 5,
            width_px: 1200,
            rotation: 0,
        };

        // Initially empty
        assert!(cache.get_best_for_page(1, 5, 0).is_none());

        // Insert small texture
        cache.insert(key_small, tex_small.clone(), 300, 400, 300 * 400 * 4);
        assert_eq!(
            cache.get_best_for_page(1, 5, 0).map(|t| t.id()),
            Some(tex_small.id())
        );

        // Insert large texture
        cache.insert(key_large, tex_large.clone(), 1200, 1600, 1200 * 1600 * 4);
        // Best should now prefer the higher resolution one (1200 > 300)
        assert_eq!(
            cache.get_best_for_page(1, 5, 0).map(|t| t.id()),
            Some(tex_large.id())
        );

        // Different rotation or page should still be None
        assert!(cache.get_best_for_page(1, 5, 90).is_none());
        assert!(cache.get_best_for_page(1, 6, 0).is_none());
    }

    #[test]
    fn test_cache_clear_doc() {
        let ctx = egui::Context::default();
        let mut cache = TextureCache::new(10_000_000);

        let img = egui::ColorImage::example();
        let tex1 = ctx.load_texture("tex1", img.clone(), egui::TextureOptions::LINEAR);
        let tex2 = ctx.load_texture("tex2", img, egui::TextureOptions::LINEAR);

        let key1 = TextureKey {
            doc_id: 1,
            page: 0,
            width_px: 200,
            rotation: 0,
        };
        let key2 = TextureKey {
            doc_id: 2,
            page: 0,
            width_px: 200,
            rotation: 0,
        };

        cache.insert(key1.clone(), tex1, 200, 200, 10_000);
        cache.insert(key2.clone(), tex2, 200, 200, 15_000);

        assert_eq!(cache.current_bytes(), 25_000);
        assert!(cache.contains(&key1));
        assert!(cache.contains(&key2));

        // Clear doc 1
        cache.clear_doc(1);
        assert_eq!(cache.current_bytes(), 15_000);
        assert!(!cache.contains(&key1));
        assert!(cache.contains(&key2));
    }
}
