use allsorts::{
    binary::read::ReadScope,
    font::MatchingPresentation,
    font_data::{DynamicFontTableProvider, FontData},
    glyph_position,
    subset::subset,
    tag,
};
use layout::{Error, Features, GlyphPosition, TextPosition, unit::Em};
use ouroboros::self_referencing;
use rtext::{
    hash_map::{self, HashMap},
    index_set::IndexSet,
};
use smol_str::{SmolStr, ToSmolStr};
use std::{
    borrow::Cow,
    collections::hash_map::Entry,
    sync::{Arc, Mutex, RwLock},
};

const NON_TTC_TABLE: usize = 0;

type FontSource = Arc<Cow<'static, [u8]>>;

struct CachedFont {
    source: FontSource,
    parsed: Option<Font>,
}

#[derive(Clone)]
pub struct FontCache {
    inner: Arc<RwLock<HashMap<SmolStr, CachedFont>>>,
}

impl FontCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(hash_map::new())),
        }
    }

    pub fn remove(&self, name: impl AsRef<str>) -> bool {
        let lock = self
            .inner
            .write()
            .map_err(|e| Error::LockError(e.to_string().into()));

        match lock {
            Ok(mut lock) => lock.remove(name.as_ref()).is_some(),
            Err(_) => false,
        }
    }

    pub fn add(&self, name: impl ToSmolStr, source: &'static [u8]) -> Result<(), Error> {
        self.add_cow(name, Cow::Borrowed(source), false)
    }

    pub fn replace(&self, name: impl ToSmolStr, source: &'static [u8]) -> Result<(), Error> {
        self.add_cow(name, Cow::Borrowed(source), true)
    }

    pub fn add_owned(&self, name: impl ToSmolStr, source: Vec<u8>) -> Result<(), Error> {
        self.add_cow(name, Cow::Owned(source), false)
    }

    pub fn replace_owned(&self, name: impl ToSmolStr, source: Vec<u8>) -> Result<(), Error> {
        self.add_cow(name, Cow::Owned(source), true)
    }

    fn add_cow(
        &self,
        name: impl ToSmolStr,
        source: Cow<'static, [u8]>,
        replace: bool,
    ) -> Result<(), Error> {
        match self
            .inner
            .write()
            .map_err(|e| Error::LockError(e.to_string().into()))?
            .entry(name.to_smolstr())
        {
            Entry::Occupied(mut occupied) => {
                if replace {
                    *occupied.get_mut() = CachedFont {
                        source: Arc::new(source),
                        parsed: None,
                    };
                }
            }
            Entry::Vacant(vacant) => {
                vacant.insert(CachedFont {
                    source: Arc::new(source),
                    parsed: None,
                });
            }
        }
        Ok(())
    }

    pub fn get(&self, name: impl AsRef<str>) -> Result<Font, Error> {
        let name = name.as_ref();

        {
            let lock = self
                .inner
                .read()
                .map_err(|e| Error::LockError(e.to_string().into()))?;

            let font = lock
                .get(name)
                .ok_or_else(|| Error::UnknownFont(name.to_smolstr()))?;

            if let Some(font) = font.parsed.clone() {
                return Ok(font.clone());
            }
        }

        let mut lock = self
            .inner
            .write()
            .map_err(|e| Error::LockError(e.to_string().into()))?;

        let font = lock
            .get_mut(name)
            .ok_or_else(|| Error::UnknownFont(name.into()))?;

        let cached_font = CachedAllsortsFont::from_source(name, font.source.clone())?;
        let parsed = Font::new(cached_font);
        font.parsed = Some(parsed.clone());

        Ok(parsed)
    }
}

impl Default for FontCache {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for Font {}
unsafe impl Sync for Font {}

#[derive(Clone)]
pub struct Font {
    cached_font: Arc<Mutex<CachedAllsortsFont>>,
}

impl Font {
    pub fn new(cached_font: CachedAllsortsFont) -> Self {
        Self {
            cached_font: Arc::new(Mutex::new(cached_font)),
        }
    }

    fn with<F, U>(&self, f: F) -> U
    where
        F: Fn(&CachedAllsortsFont) -> U,
    {
        let cached_font = self.cached_font.lock().unwrap();
        f(&cached_font)
    }

    fn with_mut<F, U>(&self, mut f: F) -> U
    where
        F: FnMut(&mut CachedAllsortsFont) -> U,
    {
        let mut cached_font = self.cached_font.lock().unwrap();
        f(&mut cached_font)
    }

    pub fn typeset(
        &self,
        text: impl AsRef<str>,
        features: &Features,
    ) -> Result<TextPosition, Error> {
        self.with_mut(|cached_font| {
            cached_font.with_font_mut(|font| Self::typeset_inner(font, text.as_ref(), features))
        })
    }

    fn typeset_inner(
        font: &mut allsorts::Font<DynamicFontTableProvider<'_>>,
        text: &str,
        features: &Features,
    ) -> Result<TextPosition, Error> {
        let features = features.into();

        let glyphs = font.map_glyphs(text, tag::LATN, MatchingPresentation::NotRequired);

        let shapes = font
            .shape(glyphs, tag::LATN, None, &features, None, true)
            .unwrap_or_else(|(_, shapes)| shapes);

        let positions = glyph_position::GlyphLayout::new(
            font,
            &shapes,
            glyph_position::TextDirection::LeftToRight,
            false,
        )
        .glyph_positions()?;

        let units_per_em = font.head_table().unwrap().unwrap().units_per_em as f64;
        let ascender = font.hhea_table.ascender as f64 / units_per_em;
        let descender = -font.hhea_table.descender as f64 / units_per_em;

        let positions = shapes
            .iter()
            .zip(positions.iter())
            .map(|(info, position)| {
                GlyphPosition::new(
                    info.glyph.unicodes.first().copied(),
                    info.glyph.glyph_index,
                    Em(position.hori_advance as f64 / units_per_em),
                    Em(position.vert_advance as f64 / units_per_em),
                    Em(position.x_offset as f64 / units_per_em),
                    Em(position.y_offset as f64 / units_per_em),
                )
            })
            .collect::<Vec<GlyphPosition>>();

        let width = positions
            .iter()
            .fold(Em(0.0), |sum, position| sum + position.h_advance);

        let depth = Em(descender);
        let height = Em(ascender + descender);

        Ok(TextPosition {
            width,
            height,
            depth,
            positions,
        })
    }

    pub fn typeset_collect(
        &self,
        glyph_collector: &mut IndexSet<u16>,
        text: impl AsRef<str>,
        features: &Features,
    ) -> Result<TextPosition, Error> {
        let mut positions = self.typeset(text, features)?;
        for glyph in positions.positions.iter_mut() {
            glyph.set_glyph_index(glyph_collector.insert_full(glyph.glyph_index).0 as u16);
        }
        Ok(positions)
    }

    pub fn subset(&self, glyph_collector: &IndexSet<u16>) -> Result<Option<Vec<u8>>, Error> {
        self.with(|cached_font| Self::subset_inner(cached_font.borrow_source(), glyph_collector))
    }

    fn subset_inner(
        source: &FontSource,
        glyph_collector: &IndexSet<u16>,
    ) -> Result<Option<Vec<u8>>, Error> {
        if glyph_collector.is_empty() {
            return Ok(None);
        }
        let subsetted_glyphs: Vec<u16> = glyph_collector.iter().copied().collect();

        let scope = ReadScope::new(source);
        let font_data = scope.read::<FontData>()?;
        let provider = font_data.table_provider(NON_TTC_TABLE)?;

        match subset(&provider, &subsetted_glyphs) {
            Ok(subset) => Ok(Some(subset)),
            Err(error) => Err(error.into()),
        }
    }
}

#[self_referencing]
pub struct CachedAllsortsFont {
    source: FontSource,
    #[borrows(source)]
    #[covariant]
    font: allsorts::Font<allsorts::font_data::DynamicFontTableProvider<'this>>,
}

impl CachedAllsortsFont {
    fn from_source(name: &str, source: FontSource) -> Result<Self, Error> {
        Self::try_new(source, |source| {
            let scope = ReadScope::new(source);
            let font_data = scope.read::<FontData>()?;
            let provider = font_data.table_provider(NON_TTC_TABLE)?;
            allsorts::Font::new(provider).map_err(|_| Error::MalformedFont(name.into()))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::BufWriter};

    use layout::Features;
    use printpdf::{Color, Mm, PdfDocument, Point, Polygon, Pt, Rgb, path::PaintMode};
    use rtext::index_set;

    use super::FontCache;

    #[test]
    fn render() {
        let (doc, page1, layer1) =
            PdfDocument::new("PDF_Document_title", Mm(500.0), Mm(300.0), "Layer 1");
        let current_layer = doc.get_page(page1).get_layer(layer1);

        let bin_font = include_bytes!("../../tests/Lato-Regular.ttf").as_ref();

        // text fill color = blue
        let blue = Rgb::new(13.0 / 256.0, 71.0 / 256.0, 161.0 / 256.0, None);
        let orange = Rgb::new(244.0 / 256.0, 67.0 / 256.0, 54.0 / 256.0, None);
        current_layer.set_fill_color(Color::Rgb(blue));
        current_layer.set_outline_color(Color::Rgb(orange));

        // For more complex layout of text, you can use functions
        // defined on the PdfLayerReference
        // Make sure to wrap your commands
        // in a `begin_text_section()` and `end_text_section()` wrapper
        current_layer.begin_text_section();

        // setup the general fonts.
        // see the docs for these functions for details
        current_layer.set_text_cursor(Mm(10.0), Mm(100.0));
        current_layer.set_line_height(33.0);
        // current_layer.set_word_spacing(3000.0);
        // current_layer.set_character_spacing(0.0);

        let sh_fonts = FontCache::new();
        sh_fonts.add("LatoReg", bin_font).unwrap();

        let sh_font = sh_fonts.get("LatoReg").unwrap();

        let mut collector = index_set::new::<u16>();
        collector.insert(0);

        let sh_positions = sh_font
            .typeset_collect(
                &mut collector,
                "Ťg AVA AA ě Tě 012 afa afia",
                &Features::empty(),
            )
            .unwrap();

        let subsetted_font = sh_fonts
            .get("LatoReg")
            .unwrap()
            .subset(&collector)
            .unwrap()
            .unwrap();
        let mut font_reader = std::io::Cursor::new(subsetted_font);
        let sub_font = doc.add_external_font(&mut font_reader).unwrap();
        current_layer.set_font(&sub_font, 33.0);

        for position in sh_positions.positions.iter() {
            if position.h_offset.0 != 0.0 || position.v_offset.0 != 0.0 {
                current_layer.set_text_cursor(
                    Pt((position.h_offset.0 * 33.0) as f32).into(),
                    Pt((position.v_offset.0 * 33.0) as f32).into(),
                );
            }

            current_layer.write_codepoints([position.glyph_index]);

            current_layer.set_text_cursor(
                Pt(((position.h_advance.0 - position.h_offset.0) * 33.0) as f32).into(),
                Pt(if position.v_offset.0.abs() == 0.0 {
                    0.0
                } else {
                    -(position.v_offset.0 * 33.0) as f32
                })
                .into(),
            );
        }

        current_layer.end_text_section();

        current_layer.set_outline_thickness(0.1);

        let mut hofs = 0.0;
        for position in sh_positions.positions.iter() {
            let points = vec![
                (
                    Point::new(Mm(10.0) + Pt(hofs * 33.0).into(), Mm(100.0)),
                    false,
                ),
                (
                    Point::new(Mm(10.0) + Pt(hofs * 33.0).into(), Mm(90.0)),
                    false,
                ),
            ];

            let mut polygon = Polygon::from_iter(points);
            polygon.mode = PaintMode::Stroke;
            current_layer.add_polygon(polygon);

            hofs += position.h_advance.0 as f32;
        }

        let points = vec![
            (
                Point::new(Mm(10.0) + Pt(hofs * 33.0).into(), Mm(100.0)),
                false,
            ),
            (
                Point::new(Mm(10.0) + Pt(hofs * 33.0).into(), Mm(90.0)),
                false,
            ),
        ];

        let mut polygon = Polygon::from_iter(points);
        polygon.mode = PaintMode::Stroke;
        current_layer.add_polygon(polygon);

        let points = vec![
            (
                Point::new(
                    Mm(10.0),
                    Mm(100.0)
                        + Pt(((sh_positions.height.0 - sh_positions.depth.0) * 33.0) as f32).into(),
                ),
                false,
            ),
            (
                Point::new(
                    Mm(50.0),
                    Mm(100.0)
                        + Pt(((sh_positions.height.0 - sh_positions.depth.0) * 33.0) as f32).into(),
                ),
                false,
            ),
        ];

        let mut polygon = Polygon::from_iter(points);
        polygon.mode = PaintMode::Stroke;
        current_layer.add_polygon(polygon);

        let points = vec![
            (
                Point::new(
                    Mm(10.0),
                    Mm(100.0) - Pt(((sh_positions.depth.0) * 33.0) as f32).into(),
                ),
                false,
            ),
            (
                Point::new(
                    Mm(50.0),
                    Mm(100.0) - Pt(((sh_positions.depth.0) * 33.0) as f32).into(),
                ),
                false,
            ),
        ];

        let mut polygon = Polygon::from_iter(points);
        polygon.mode = PaintMode::Stroke;
        current_layer.add_polygon(polygon);

        doc.save(&mut BufWriter::new(
            File::create("test_fonts_a.pdf").unwrap(),
        ))
        .unwrap();
    }
}
