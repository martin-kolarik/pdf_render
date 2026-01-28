mod font;
pub use font::FontCache;

mod render;
pub use render::*;

pub fn new_font_cache() -> FontCache {
    FontCache::new()
}
