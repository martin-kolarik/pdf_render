use std::{borrow::Borrow, sync::Arc};

use layout::{
    Error, Features, NewPageOptions, Rgba, Stroke, Style, TextPosition,
    position::{Offset, Quad, Size},
    unit::{FillPerMille, Unit},
};
use printpdf::{
    Color, IndirectFontRef, PdfDocumentReference, PdfLayerIndex, PdfLayerReference, PdfPageIndex,
    PdfPageReference, Point, Polygon, Rgb, path::PaintMode,
};
use rtext::index_set::{self, IndexSet};

use crate::font::Fonts;

use super::{from_pt, from_rgba, from_unit};

struct RenderFont {
    name: String,
    glyph_collector: IndexSet<u16>,
    font_ref: Option<IndirectFontRef>,
}

impl RenderFont {
    fn new(name: impl Into<String>) -> Self {
        let mut collector = index_set::new();
        collector.insert(0);

        Self {
            name: name.into(),
            glyph_collector: collector,
            font_ref: None,
        }
    }
}

pub struct RenderFonts {
    fonts: Fonts,
    render_fonts: Vec<RenderFont>,
}

impl RenderFonts {
    pub fn new(fonts: Fonts) -> Self {
        Self {
            fonts,
            render_fonts: vec![],
        }
    }

    pub fn typeset<F, B>(
        &mut self,
        font_name: &F,
        text: &B,
        features: &Features,
    ) -> Result<TextPosition, Error>
    where
        F: Borrow<str> + ?Sized,
        B: Borrow<str> + ?Sized,
    {
        let font_name = font_name.borrow();
        let glyph_collector = match self
            .render_fonts
            .iter_mut()
            .find(|render_font| render_font.name == font_name)
        {
            Some(font) => &mut font.glyph_collector,
            None => {
                self.render_fonts.push(RenderFont::new(font_name));
                &mut self.render_fonts.last_mut().unwrap().glyph_collector
            }
        };

        self.fonts
            .get(font_name)?
            .typeset_collect(glyph_collector, text, features)
    }

    pub fn complete_and_write(&mut self, document: &PdfDocumentReference) -> Result<(), Error> {
        for render_font in self.render_fonts.iter_mut() {
            let subsetted_font = self
                .fonts
                .get(&render_font.name)?
                .subset(&render_font.glyph_collector)?;
            let subsetted_font = match subsetted_font {
                Some(subsetted_font) => subsetted_font,
                None => continue,
            };
            let reader = std::io::Cursor::new(subsetted_font);
            render_font.font_ref = Some(
                document
                    .add_external_font(reader)
                    .map_err(|error| Error::PdfWrite(error.to_string()))?,
            );
        }

        Ok(())
    }

    pub fn get_font_ref<B>(&self, name: &B) -> Option<&IndirectFontRef>
    where
        B: Borrow<str> + ?Sized,
    {
        if let Some(render_font) = self
            .render_fonts
            .iter()
            .find(|render_font| render_font.name == name.borrow())
        {
            render_font.font_ref.as_ref()
        } else {
            None
        }
    }
}

pub struct RenderContext {
    fonts: RenderFonts,

    document: PdfDocumentReference,
    page: PdfPageReference,
    layer: PdfLayerReference,

    page_margin: Quad,
    page_size: Size,
    page_start: Option<Offset>,
    page_end: Option<Offset>,

    style: Arc<Style>,
    debug_frame: bool,
    debug_page_breaks: bool,
}

impl RenderContext {
    pub fn new(
        document: PdfDocumentReference,
        page: PdfPageIndex,
        layer: PdfLayerIndex,
        margin: Quad,
        size: Size,
        fonts: Fonts,
    ) -> Self {
        let page = document.get_page(page);
        let layer = page.get_layer(layer);

        let mut render_context = Self {
            fonts: RenderFonts::new(fonts),
            document,
            page,
            layer,
            page_margin: margin,
            page_size: size,
            page_start: None,
            page_end: None,
            style: Style::new_default(),
            debug_frame: false,
            debug_page_breaks: false,
        };
        render_context.set_page_offsets(Unit::from(0));

        render_context
    }

    pub fn with_debug_frame(mut self, debug_frame: bool) -> Self {
        self.debug_frame = debug_frame;
        self
    }

    pub fn with_debug_page_breaks(mut self, debug_page_breaks: bool) -> Self {
        self.debug_page_breaks = debug_page_breaks;
        self
    }

    pub fn complete_fonts(&mut self) -> Result<(), Error> {
        self.fonts.complete_and_write(&self.document)
    }

    pub fn save_to_bytes(self) -> Result<Vec<u8>, Error> {
        self.document
            .save_to_bytes()
            .map_err(|error| Error::PdfWrite(error.to_string()))
    }

    fn page_content_offset(&self, content_offset: &Offset) -> Offset {
        match &self.page_start {
            Some(page_start) => content_offset - page_start,
            None => content_offset.clone(),
        }
    }

    fn swap_y(&self, page_position: &Offset) -> Offset {
        Offset::new(page_position.x, self.page_size.height() - page_position.y)
    }

    fn new_page_internal(&mut self, margin: Option<&Quad>, size: Option<&Size>) {
        if let Some(margin) = margin {
            self.page_margin = margin.clone();
        }
        if let Some(size) = size {
            self.page_size = size.clone();
        }

        self.page_start = None;
        self.page_end = None;

        let (page, layer) = self.document.add_page(
            from_unit(self.page_size.width()),
            from_unit(self.page_size.height()),
            "default",
        );

        self.page = self.document.get_page(page);
        self.layer = self.page.get_layer(layer);
    }

    fn check_page_break(
        &mut self,
        content_offset: impl Into<Unit>,
        content_height: impl Into<Unit>,
    ) -> bool {
        let content_offset = content_offset.into();
        let content_height = content_height.into();

        let mut new_page = false;
        if let Some(page_end) = &self.page_end {
            if content_offset + content_height > page_end.y {
                if self.debug_page_breaks {
                    tracing::debug!(
                        "Page BREAK at offset {content_offset:?}, content height {content_height:?}, page end {:?}",
                        page_end.y
                    );
                }

                self.new_page_internal(None, None);
                new_page = true;
            } else if self.debug_page_breaks {
                tracing::debug!(
                    "Page fill CHECK at offset {content_offset:?}, content height {content_height:?}, page end {:?}",
                    page_end.y
                );
            }
        }

        if self.page_start.is_none() {
            self.set_page_offsets(content_offset);
        }

        new_page
    }

    fn set_page_offsets(&mut self, content_offset: Unit) {
        let page_start = Offset::new(Unit::zero(), content_offset);

        let mut page_end = page_start.clone();
        page_end.x_advance(self.page_size.width() - self.page_margin.width());
        page_end.y_advance(self.page_size.height() - self.page_margin.height());

        self.page_start = Some(page_start);
        self.page_end = Some(page_end);
    }

    fn line_inner(&self, content_points: &[&Offset]) {
        let line_points = content_points.iter().map(|point| {
            let position = self.swap_y(point);
            (
                Point::new(from_unit(position.x), from_unit(position.y)),
                false,
            )
        });

        let mut polygon = Polygon::from_iter(line_points);
        polygon.mode = PaintMode::Stroke;

        self.layer.add_polygon(polygon);
    }
}

impl layout::MeasureContext for RenderContext {
    fn style(&self) -> &Style {
        self.style.as_ref()
    }

    fn typeset(&mut self, style: &Style, text: &str) -> Result<TextPosition, Error> {
        let font = style.font().merge(self.style.font());
        if font.name().is_none() || font.size().is_none() {
            Err(Error::UnknownFont("Font name or size is undefined".into()))
        } else {
            self.fonts.typeset(
                font.name().unwrap(),
                text,
                &font.features().cloned().unwrap_or_default(),
            )
        }
    }
}

impl layout::RenderContext for RenderContext {
    fn new_page(&mut self, options: Option<NewPageOptions>) -> bool {
        if let Some(must_be_in_page) = options
            .as_ref()
            .and_then(|options| options.must_be_in_page())
        {
            self.check_page_break(must_be_in_page.0, must_be_in_page.1)
        } else {
            self.new_page_internal(
                options.as_ref().and_then(|options| options.margin()),
                options.as_ref().and_then(|options| options.size()),
            );
            true
        }
    }

    fn debug_frame(&mut self, content_position: &Offset, size: &Size) {
        if self.debug_frame {
            let content_position = self.page_content_offset(content_position);
            let top_left = self.page_margin.offset(&content_position);
            let bottom_right = &top_left + size;

            let points = [
                &top_left,
                &Offset::new(bottom_right.x, top_left.y),
                &bottom_right,
                &Offset::new(top_left.x, bottom_right.y),
                &top_left,
            ];

            self.layer
                .set_outline_color(from_rgba(&Rgba::from((240, 240, 240, 1.0))));
            self.layer.set_outline_thickness(0.25);

            self.line_inner(&points);
        }
    }

    fn line(&mut self, from: &Offset, to: &Offset, stroke: &Stroke) {
        self.check_page_break(from.y, 0);

        let from = self.page_content_offset(from);
        let from = self.page_margin.offset(&from);

        let to = self.page_content_offset(to);
        let to = self.page_margin.offset(&to);

        self.layer.set_outline_color(from_rgba(stroke.color()));
        self.layer
            .set_outline_thickness(stroke.thickness().0 as f32);

        self.line_inner(&[&from, &to]);
    }

    fn text(
        &mut self,
        content_position: &Offset,
        style: &Style,
        text: &TextPosition,
        position_is_baseline: bool,
    ) {
        if text.positions.is_empty() {
            return;
        }

        let font = style.font().merge(self.style.font());
        if font.name().is_none() || font.size().is_none() {
            tracing::warn!("Try to typeset text without defined font");
            return;
        }
        let font_size = font.size().unwrap();
        let font_scaling = font
            .scaling()
            .as_ref()
            .map(FillPerMille::scaling)
            .unwrap_or(1.0);

        self.check_page_break(content_position.y, text.height * font_size);

        let content_position = self.page_content_offset(content_position);
        let mut page_position = self.page_margin.offset(&content_position);
        if !position_is_baseline {
            page_position.y_advance(text.ascent() * font.size().unwrap());
        }
        let page_position = self.swap_y(&page_position);

        let font_ref = self.fonts.get_font_ref(font.name().unwrap()).unwrap();

        let layer = &self.layer;
        layer.begin_text_section();
        if let Some(color) = style.color()
            && *color != Rgba::black()
        {
            let color = color.into_rgba();
            layer.set_fill_color(Color::Rgb(Rgb::new(color.0, color.1, color.2, None)));
        }
        layer.set_font(font_ref, *font.size().unwrap() as f32);
        layer.set_text_cursor(from_unit(page_position.x), from_unit(page_position.y));
        layer.set_text_scaling(100.0 * font_scaling as f32);

        for position in text.positions.iter() {
            let h_offset = position.h_offset;
            let v_offset = position.v_offset;
            if !h_offset.is_zero() || !v_offset.is_zero() {
                let h_offset = h_offset * font_size * font_scaling;
                let v_offset = v_offset * font_size;
                layer.set_text_cursor(from_pt(h_offset), from_pt(v_offset));
            }

            layer.write_codepoints([position.glyph_index]);

            let h_advance = position.h_advance_rest() * font_size * font_scaling;
            let v_advance = position.v_advance_rest() * font_size;

            layer.set_text_cursor(from_pt(h_advance), from_pt(v_advance));
        }

        layer.set_text_scaling(100.0);
        if let Some(color) = style.color()
            && *color != Rgba::black()
        {
            layer.set_fill_color(Color::Rgb(Rgb::new(0.0, 0.0, 0.0, None)));
        }
        layer.end_text_section();
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::BufWriter};

    use layout::{
        Features, Font, MeasureContext, RenderContext as _, StyleBuilder,
        position::{Offset, Quad, Size},
        unit::{Mm, Pt},
    };
    use printpdf::PdfDocument;

    use crate::{new_font_sources, new_fonts};

    use super::RenderContext;

    #[test]
    fn render_context() {
        let mut sources = new_font_sources();

        let font_bin = include_bytes!("../../tests/Lato-Regular.ttf").as_ref();
        sources.add("LatoReg", font_bin).unwrap();

        let fonts = new_fonts(sources);

        let (document, page, layer) =
            PdfDocument::new("Test", printpdf::Mm(210.0), printpdf::Mm(297.0), "default");

        let mut rctx = RenderContext::new(
            document,
            page,
            layer,
            Quad::empty(),
            Size::fixed(Mm(210.0), Mm(297.0)),
            fonts,
        )
        .with_debug_frame(true);

        let style = StyleBuilder::default()
            .with_font(Font::new(
                "LatoReg",
                Pt(36.0),
                Some(Features::empty().pnum()),
            ))
            .build();

        let text1 = rctx
            .typeset(&style, "Fimfifárumík 12115 jgenealogie")
            .unwrap();

        let style = StyleBuilder::default()
            .with_font(Font::new(
                "LatoReg",
                Pt(36.0),
                Some(Features::empty().tnum().smcp()),
            ))
            .build();

        let text2 = rctx
            .typeset(&style, "Fimfifárumík 12115 jgenealogie")
            .unwrap();

        rctx.complete_fonts().unwrap();

        rctx.text(&Offset::new(Mm(20.0), Mm(20.0)), &style, &text1, true);

        rctx.text(&Offset::new(Mm(20.0), Mm(40.0)), &style, &text2, true);

        let style = StyleBuilder::default()
            .with_font(Font::new(
                "LatoReg",
                Pt(18.0),
                Some(Features::empty().pnum()),
            ))
            .build();
        rctx.text(&Offset::new(Mm(20.0), Mm(60.0)), &style, &text2, true);
        let style = StyleBuilder::default()
            .with_font(Font::new(
                "LatoReg",
                Pt(12.0),
                Some(Features::empty().pnum()),
            ))
            .build();
        rctx.text(
            &Offset::new(Mm(20.0) + text2.width * Pt(18.0), Mm(60.0)),
            &style,
            &text2,
            true,
        );

        rctx.document
            .save(&mut BufWriter::new(
                File::create("test_fonts_r.pdf").unwrap(),
            ))
            .unwrap();
    }
}
