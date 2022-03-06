use allsorts::{
    binary::read::ReadScope, font::MatchingPresentation, font_data::FontData, glyph_position,
    subset::subset, tag,
};
use indexmap::IndexSet;
use layout::{unit::Em, Error, Features, GlyphPosition, TextPosition};
use std::{
    borrow::{Borrow, Cow},
    sync::{Arc, Mutex},
};

const NON_TTC_TABLE: usize = 0;

#[derive(Clone)]
pub struct FontSources {
    data: Arc<Mutex<Vec<Arc<FontSource>>>>,
}

impl FontSources {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(vec![])),
        }
    }

    pub fn add(&mut self, name: impl Into<String>, source: &'static [u8]) -> Result<(), Error> {
        self.add_cow(name, Cow::Borrowed(source))
    }

    pub fn add_owned(&mut self, name: impl Into<String>, source: Vec<u8>) -> Result<(), Error> {
        self.add_cow(name, Cow::Owned(source))
    }

    fn add_cow(
        &mut self,
        name: impl Into<String>,
        source: Cow<'static, [u8]>,
    ) -> Result<(), Error> {
        let name = name.into();
        let mut lock = self
            .data
            .lock()
            .map_err(|l| Error::LockError(l.to_string()))?;
        if !lock.iter().any(|source| source.name == name) {
            lock.push(Arc::new(FontSource::new(name, source)?));
        }
        Ok(())
    }

    pub fn get<B>(&self, name: &B) -> Result<Arc<Font>, Error>
    where
        B: Borrow<str> + ?Sized,
    {
        let name = name.borrow();

        let source = self
            .data
            .lock()
            .map_err(|l| Error::LockError(l.to_string()))?
            .iter()
            .find(|&source| source.name == name)
            .ok_or_else(|| Error::UnknownFont(name.to_owned()))?
            .clone();

        Ok(Arc::new(Font::from_source(source)))
    }
}

impl Default for FontSources {
    fn default() -> Self {
        Self::new()
    }
}

struct FontSource {
    name: String,
    source: Arc<Cow<'static, [u8]>>,
}

impl FontSource {
    fn new(name: String, source: Cow<'static, [u8]>) -> Result<Self, Error> {
        Ok(Self {
            name,
            source: Arc::new(source),
        })
    }
}

pub struct Font {
    source: Arc<FontSource>,
}

impl Font {
    fn from_source(source: Arc<FontSource>) -> Self {
        Self { source }
    }

    pub fn typeset<B>(&self, text: &B, features: &Features) -> Result<TextPosition, Error>
    where
        B: Borrow<str> + ?Sized,
    {
        let scope = ReadScope::new(&self.source.source);
        let font_data = scope.read::<FontData>()?;
        let provider = font_data.table_provider(NON_TTC_TABLE)?;
        let mut font = allsorts::Font::new(provider)?
            .ok_or_else(|| Error::MalformedFont(self.source.name.clone()))?;

        let features = features.into();

        let glyphs = font.map_glyphs(text.borrow(), tag::LATN, MatchingPresentation::NotRequired);
        let shapes = font
            .shape(glyphs, tag::LATN, None, &features, true)
            .map_or_else(|(_, shapes)| shapes, |shapes| shapes);
        let positions = glyph_position::GlyphLayout::new(
            &mut font,
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
            .fold(Em(0.0), |sum, position| sum + position.h_advance());

        let depth = Em(descender);
        let height = Em(ascender + descender);

        Ok(TextPosition {
            width,
            height,
            depth,
            positions,
        })
    }

    pub fn typeset_collect<B>(
        &self,
        glyph_collector: &mut IndexSet<u16>,
        text: &B,
        features: &Features,
    ) -> Result<TextPosition, Error>
    where
        B: Borrow<str> + ?Sized,
    {
        let mut positions = self.typeset(text, features)?;
        for glyph in positions.positions.iter_mut() {
            glyph.set_glyph_index(glyph_collector.insert_full(glyph.glyph_index()).0 as u16);
        }
        Ok(positions)
    }

    pub fn subset(&self, glyph_collector: &IndexSet<u16>) -> Result<Option<Vec<u8>>, Error> {
        if glyph_collector.is_empty() {
            return Ok(None);
        }
        let subsetted_glyphs: Vec<u16> = glyph_collector.iter().copied().collect();

        let scope = ReadScope::new(&self.source.source);
        let font_data = scope.read::<FontData>()?;
        let provider = font_data.table_provider(NON_TTC_TABLE)?;

        match subset(&provider, &subsetted_glyphs, None) {
            Ok(subset) => Ok(Some(subset)),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::BufWriter};

    use indexmap::IndexSet;
    use layout::Features;
    use printpdf::{Color, Line, Mm, PdfDocument, Point, Pt, Rgb};

    use crate::FontSources;

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

        let mut sh_sources = FontSources::new();
        sh_sources.add("LatoReg", bin_font).unwrap();

        let sh_font = sh_sources.get("LatoReg").unwrap();

        let mut collector = IndexSet::<u16>::new();
        collector.insert(0);

        let sh_positions = sh_font
            .typeset_collect(
                &mut collector,
                "Ťg AVA AA ě Tě 012 afa afia",
                &Features::empty(),
            )
            .unwrap();

        let subsetted_font = sh_sources
            .get("LatoReg")
            .unwrap()
            .subset(&collector)
            .unwrap()
            .unwrap();
        let mut font_reader = std::io::Cursor::new(subsetted_font);
        let sub_font = doc.add_external_font(&mut font_reader).unwrap();
        current_layer.set_font(&sub_font, 33.0);

        for position in sh_positions.positions.iter() {
            if position.h_offset().0 != 0.0 || position.v_offset().0 != 0.0 {
                current_layer.set_text_cursor(
                    Pt(position.h_offset().0 * 33.0).into(),
                    Pt(position.v_offset().0 * 33.0).into(),
                );
            }

            current_layer.write_codepoints([position.glyph_index()]);

            current_layer.set_text_cursor(
                Pt((position.h_advance().0 - position.h_offset().0) * 33.0).into(),
                Pt(if position.v_offset().0.abs() == 0.0 {
                    0.0
                } else {
                    -position.v_offset().0 * 33.0
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
                    Point::new(Mm(10.0) + Pt(hofs as f64 * 33.0).into(), Mm(100.0)),
                    false,
                ),
                (
                    Point::new(Mm(10.0) + Pt(hofs as f64 * 33.0).into(), Mm(90.0)),
                    false,
                ),
            ];

            let line = Line {
                points,
                is_closed: false,
                has_fill: false,
                has_stroke: true,
                is_clipping_path: false,
            };
            current_layer.add_shape(line);

            hofs += position.h_advance().0;
        }

        let points = vec![
            (
                Point::new(Mm(10.0) + Pt(hofs as f64 * 33.0).into(), Mm(100.0)),
                false,
            ),
            (
                Point::new(Mm(10.0) + Pt(hofs as f64 * 33.0).into(), Mm(90.0)),
                false,
            ),
        ];

        let line = Line {
            points,
            is_closed: false,
            has_fill: false,
            has_stroke: true,
            is_clipping_path: false,
        };
        current_layer.add_shape(line);

        let points = vec![
            (
                Point::new(
                    Mm(10.0),
                    Mm(100.0) + Pt((sh_positions.height.0 - sh_positions.depth.0) * 33.0).into(),
                ),
                false,
            ),
            (
                Point::new(
                    Mm(50.0),
                    Mm(100.0) + Pt((sh_positions.height.0 - sh_positions.depth.0) * 33.0).into(),
                ),
                false,
            ),
        ];

        let line = Line {
            points,
            is_closed: false,
            has_fill: false,
            has_stroke: true,
            is_clipping_path: false,
        };
        current_layer.add_shape(line);

        let points = vec![
            (
                Point::new(
                    Mm(10.0),
                    Mm(100.0) - Pt((sh_positions.depth.0) * 33.0).into(),
                ),
                false,
            ),
            (
                Point::new(
                    Mm(50.0),
                    Mm(100.0) - Pt((sh_positions.depth.0) * 33.0).into(),
                ),
                false,
            ),
        ];

        let line = Line {
            points,
            is_closed: false,
            has_fill: false,
            has_stroke: true,
            is_clipping_path: false,
        };
        current_layer.add_shape(line);

        doc.save(&mut BufWriter::new(
            File::create("test_fonts_a.pdf").unwrap(),
        ))
        .unwrap();
    }
}
