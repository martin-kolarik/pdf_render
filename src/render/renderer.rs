use layout::{
    Error, Layout,
    position::{Offset, Quad, Size},
};
use printpdf::PdfDocument;

use crate::{RenderContext, font::Fonts};

use super::from_unit;

pub struct Renderer {
    context: RenderContext,
    content_size: Size,
}

impl Renderer {
    pub fn new(document_title: &str, page_margin: Quad, page_size: Size, fonts: Fonts) -> Self {
        let (document, page, layer) = PdfDocument::new(
            document_title,
            from_unit(page_size.width.base_size()),
            from_unit(page_size.height.base_size()),
            "default",
        );

        let mut content_size = page_size.clone();
        page_margin.narrow(None, Some(&mut content_size));

        let context = RenderContext::new(document, page, layer, page_margin, page_size, fonts);

        Self {
            context,
            content_size,
        }
    }

    pub fn with_debug_frame(mut self, debug_frame: bool) -> Self {
        self.context = self.context.with_debug_frame(debug_frame);
        self
    }

    pub fn with_debug_page_breaks(mut self, debug_page_breaks: bool) -> Self {
        self.context = self.context.with_debug_page_breaks(debug_page_breaks);
        self
    }

    pub fn render(
        mut self,
        mut layout: Box<dyn Layout>,
        debug_input: bool,
        debug_measured: bool,
        debug_laid_out: bool,
    ) -> Result<Vec<u8>, Error> {
        if debug_input {
            tracing::debug!("INPUT\n{:#?}", layout);
        }

        layout.measure(&mut self.context, self.content_size.clone())?;

        if debug_measured {
            tracing::debug!("MEASURED\n{:#?}", layout);
        }

        layout.lay_out(&mut self.context, Offset::zero(), self.content_size.clone())?;

        if debug_laid_out {
            tracing::debug!("LAID OUT\n{:#?}", layout);
        }

        self.context.complete_fonts()?;
        layout.render(&mut self.context)?;

        let pdf = self.context.save_to_bytes()?;

        Ok(pdf)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::{BufWriter, Write},
    };

    use layout::{
        Axis, Border, Features, Font, LayoutBox, Rgba, Stroke, StyleBuilder, Text, hbox, hfill,
        position::{Quad, Size},
        unit::{Mm, Pt},
        vbox, vfill,
    };

    use crate::{Renderer, new_font_sources, new_fonts};

    #[test]
    fn h_center() {
        let mut sources = new_font_sources();

        let font_bin = include_bytes!("../../tests/Lato-Regular.ttf").as_ref();
        sources.add("LatoReg", font_bin).unwrap();

        let fonts = new_fonts(sources);

        let renderer = Renderer::new(
            "Text",
            Quad::square(Mm(10.0)),
            Size::fixed(Mm(210.0), Mm(297.0)),
            fonts,
        )
        .with_debug_frame(true);

        let style = StyleBuilder::default().with_font(Font::new(
            "LatoReg",
            Pt(10.0),
            Some(Features::default()),
        ));

        let outer = hbox()
            .axis_size(Mm(190.0))
            .child(hfill(2))
            .child(Text::new("Žáňa Nováková jr.").style(style))
            .child(hfill(1));

        let pdf = renderer
            .render(Box::new(outer), false, false, false)
            .unwrap();

        BufWriter::new(File::create("test_fonts_th.pdf").unwrap())
            .write_all(&pdf)
            .unwrap();
    }

    #[test]
    fn v_center() {
        let mut sources = new_font_sources();

        let font_bin = include_bytes!("../../tests/Lato-Regular.ttf").as_ref();
        sources.add("LatoReg", font_bin).unwrap();

        let fonts = new_fonts(sources);

        let renderer = Renderer::new(
            "Text",
            Quad::square(Mm(10.0)),
            Size::fixed(Mm(210.0), Mm(297.0)),
            fonts,
        )
        .with_debug_frame(true);

        let style = StyleBuilder::default().with_font(Font::new(
            "LatoReg",
            Pt(10.0),
            Some(Features::default()),
        ));

        let outer = hbox()
            .mark("1")
            .axis_size(Mm(190.0))
            .child(hfill(2).mark("2"))
            .child(
                vbox()
                    .mark("3")
                    .axis_size(Mm(277.0))
                    .child(vfill(1).mark("4"))
                    .child(Text::new("Žáňa Nováková jr.").mark("5").style(style))
                    .child(vfill(1).mark("6")),
            )
            .child(hfill(1).mark("7"));

        let pdf = renderer
            .render(Box::new(outer), false, false, false)
            .unwrap();

        BufWriter::new(File::create("test_fonts_tv.pdf").unwrap())
            .write_all(&pdf)
            .unwrap();
    }

    #[test]
    fn padding() {
        let mut sources = new_font_sources();

        let font_bin = include_bytes!("../../tests/Lato-Regular.ttf").as_ref();
        sources.add("LatoReg", font_bin).unwrap();

        let fonts = new_fonts(sources);

        let renderer = Renderer::new(
            "Text",
            Quad::square(Mm(10.0)),
            Size::fixed(Mm(210.0), Mm(297.0)),
            fonts,
        )
        .with_debug_frame(true);

        let style = StyleBuilder::default().with_font(Font::new(
            "LatoReg",
            Pt(10.0),
            Some(Features::default()),
        ));

        let outer = hbox()
            .mark("outer")
            .axis_size(Mm(190.0))
            .child(hfill(2))
            .child(
                vbox()
                    .mark("inner")
                    .axis_size(Mm(277.0))
                    .child(vfill(1))
                    .child(
                        LayoutBox::new(Axis::Vertical)
                            .mark("deco")
                            .style(StyleBuilder::new().with_padding(Quad::square(Mm(4.0))))
                            .child(Text::new("Žáňa Nováková jr.").style(style).mark("TT")),
                    )
                    .child(vfill(1)),
            )
            .child(hfill(1));

        let pdf = renderer
            .render(Box::new(outer), false, false, false)
            .unwrap();

        BufWriter::new(File::create("test_padding.pdf").unwrap())
            .write_all(&pdf)
            .unwrap();
    }

    #[test]
    fn border() {
        let mut sources = new_font_sources();

        let font_bin = include_bytes!("../../tests/Lato-Regular.ttf").as_ref();
        sources.add("LatoReg", font_bin).unwrap();

        let fonts = new_fonts(sources);

        let renderer = Renderer::new(
            "Text",
            Quad::square(Mm(10.0)),
            Size::fixed(Mm(210.0), Mm(297.0)),
            fonts,
        )
        .with_debug_frame(true);

        let style = StyleBuilder::default().with_font(Font::new(
            "LatoReg",
            Pt(10.0),
            Some(Features::default()),
        ));

        let outer = hbox()
            .mark("outer")
            .axis_size(Mm(190.0))
            .child(hfill(2))
            .child(
                vbox()
                    .mark("inner")
                    .axis_size(Mm(277.0))
                    .child(vfill(1))
                    .child(
                        LayoutBox::new(Axis::Vertical)
                            .mark("deco")
                            .style(
                                StyleBuilder::new()
                                    .with_border(Border::h(Stroke::new(
                                        Rgba::from((135, 235, 64, 0.0)),
                                        Pt(1.0),
                                    )))
                                    .with_padding(Quad::square(Mm(4.0))),
                            )
                            .child(Text::new("Žáňa Nováková jr.").style(style).mark("TT")),
                    )
                    .child(vfill(1)),
            )
            .child(hfill(1));

        let pdf = renderer
            .render(Box::new(outer), false, false, false)
            .unwrap();

        BufWriter::new(File::create("test_border.pdf").unwrap())
            .write_all(&pdf)
            .unwrap();
    }
}
