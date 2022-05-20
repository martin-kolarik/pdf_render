mod font;

mod render;
pub use render::*;

pub use font::FontSources;

pub fn new_font_sources() -> FontSources {
    FontSources::new()
}
