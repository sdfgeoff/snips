//! Antialiased paths shared by annotation strokes and toolbar icons.
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform,
};

pub fn paint(color: u32, opacity: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
        (opacity * 255.0).round() as u8,
    ));
    p.anti_alias = true;
    p
}

pub fn stroke(image: &mut Pixmap, path: &Path, color: u32, width: f32) {
    image.stroke_path(
        path,
        &paint(color, 1.0),
        &Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        },
        Transform::identity(),
        None,
    );
}

pub fn round_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let mut p = PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.quad_to(x + w, y, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.quad_to(x + w, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.quad_to(x, y + h, x, y + h - r);
    p.line_to(x, y + r);
    p.quad_to(x, y, x + r, y);
    p.close();
    p.finish().unwrap()
}

pub fn fill(image: &mut Pixmap, path: &Path, color: u32) {
    image.fill_path(
        path,
        &paint(color, 1.0),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

pub fn circle(image: &mut Pixmap, x: f32, y: f32, r: f32, color: u32) {
    fill(image, &PathBuilder::from_circle(x, y, r).unwrap(), color);
}

// Every icon uses the same 24px grid and 1.8px rounded stroke.
// IDs match toolbar actions, so the visible controls and their hit targets agree.
pub fn icon(image: &mut Pixmap, id: usize, x: f32, y: f32, color: u32) {
    let mut p = PathBuilder::new();
    match id {
        0 => {
            // pencil
            p.move_to(5., 15.);
            p.line_to(15., 5.);
            p.quad_to(16., 4., 17., 5.);
            p.line_to(19., 7.);
            p.quad_to(20., 8., 19., 9.);
            p.line_to(9., 19.);
            p.line_to(4., 20.);
            p.close();
            p.move_to(13., 7.);
            p.line_to(17., 11.);
        }
        1 => {
            p.move_to(5., 19.);
            p.line_to(19., 5.);
            p.move_to(9., 5.);
            p.line_to(19., 5.);
            p.line_to(19., 15.);
        }
        2 => p.push_rect(Rect::from_xywh(4., 5., 16., 14.).unwrap()),
        3 => {
            p.move_to(5., 6.);
            p.line_to(19., 6.);
            p.move_to(12., 6.);
            p.line_to(12., 20.);
            p.move_to(8., 20.);
            p.line_to(16., 20.);
        }
        4 => {
            // highlighter
            p.move_to(7., 15.);
            p.line_to(15., 5.);
            p.line_to(20., 9.);
            p.line_to(12., 19.);
            p.close();
            p.move_to(7., 15.);
            p.line_to(5., 19.);
            p.line_to(10., 19.);
            p.move_to(4., 22.);
            p.line_to(18., 22.);
        }
        12 => {
            p.move_to(5., 9.);
            p.line_to(15., 9.);
            p.cubic_to(23., 9., 23., 20., 15., 20.);
            p.line_to(9., 20.);
            p.move_to(9., 4.);
            p.line_to(4., 9.);
            p.line_to(9., 14.);
        }
        13 => {
            p.move_to(6., 6.);
            p.line_to(18., 18.);
            p.move_to(18., 6.);
            p.line_to(6., 18.);
        }
        14 => {
            p.move_to(5., 12.);
            p.line_to(10., 17.);
            p.line_to(20., 7.);
        }
        _ => return,
    }
    let path = p
        .finish()
        .unwrap()
        .transform(Transform::from_translate(x, y))
        .unwrap();
    stroke(image, &path, color, 1.8);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagonal_has_partial_coverage_and_solid_center() {
        let mut image = Pixmap::new(40, 40).unwrap();
        let mut p = PathBuilder::new();
        p.move_to(4., 5.);
        p.line_to(35., 24.);
        stroke(&mut image, &p.finish().unwrap(), 0xff4545, 3.);
        assert!(image
            .pixels()
            .iter()
            .any(|p| p.alpha() > 0 && p.alpha() < 255));
        assert!(image.pixels().iter().any(|p| p.alpha() == 255));
        assert_eq!(image.pixel(0, 0).unwrap().alpha(), 0);
    }
}
