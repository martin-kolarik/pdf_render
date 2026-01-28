mod context;
pub(crate) use context::*;

mod renderer;
use layout::{
    Rgba,
    unit::{Mm, Pt, Unit},
};
use printpdf::Color;
pub use renderer::*;

fn from_unit(unit: Unit) -> printpdf::Mm {
    printpdf::Mm(Mm::from(unit).0 as f32)
}

fn from_pt(pt: Pt) -> printpdf::Mm {
    printpdf::Mm(Mm::from(pt).0 as f32)
}

fn from_rgba(color: &Rgba) -> Color {
    let color = color.into_rgba();
    Color::Rgb(printpdf::Rgb::new(color.0, color.1, color.2, None))
}
