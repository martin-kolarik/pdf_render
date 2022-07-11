mod font;
pub use font::{FontSources, Fonts};

mod render;
pub use render::*;

pub fn new_font_sources() -> FontSources {
    FontSources::new()
}

pub fn new_fonts(sources: FontSources) -> Fonts {
    Fonts::new(sources)
}
