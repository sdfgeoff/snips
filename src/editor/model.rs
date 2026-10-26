//! Annotation geometry and transactional undo, independent of the X11 event loop.
use x11rb::protocol::xproto::Point;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Tool {
    Select,
    Pen,
    Arrow,
    Box,
    Text,
    Highlight,
}

#[derive(Clone, Debug)]
pub(super) struct Mark {
    pub tool: Tool,
    pub color: u32,
    pub width: u16,
    pub points: Vec<Point>,
    pub text: Vec<u8>,
}

impl PartialEq for Mark {
    fn eq(&self, other: &Self) -> bool {
        self.tool == other.tool
            && self.color == other.color
            && self.width == other.width
            && self.text == other.text
            && self.points.len() == other.points.len()
            && self
                .points
                .iter()
                .zip(&other.points)
                .all(|(a, b)| a.x == b.x && a.y == b.y)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    pub fn contains(self, p: Point, tolerance: f32) -> bool {
        f32::from(p.x) >= self.left - tolerance
            && f32::from(p.x) <= self.right + tolerance
            && f32::from(p.y) >= self.top - tolerance
            && f32::from(p.y) <= self.bottom + tolerance
    }
}

impl Mark {
    pub fn arrow_head(&self) -> Option<[Point; 2]> {
        let (a, b) = (*self.points.first()?, *self.points.last()?);
        let angle = f32::from(b.y - a.y).atan2(f32::from(b.x - a.x));
        let length = (f32::from(self.width) * 4. + 8.)
            .min(f32::from(b.x - a.x).hypot(f32::from(b.y - a.y)) * 0.5);
        Some([0.5f32, -0.5].map(|offset| Point {
            x: b.x - (length * (angle + offset).cos()) as i16,
            y: b.y - (length * (angle + offset).sin()) as i16,
        }))
    }

    pub fn bounds(&self, text: Option<Bounds>) -> Option<Bounds> {
        if self.tool == Tool::Text {
            return text;
        }
        let a = self.points.first()?;
        let mut b = Bounds {
            left: f32::from(a.x),
            top: f32::from(a.y),
            right: f32::from(a.x),
            bottom: f32::from(a.y),
        };
        let mut points = self.points.clone();
        if self.tool == Tool::Arrow {
            points.extend(self.arrow_head()?);
        }
        for p in points {
            b.left = b.left.min(f32::from(p.x));
            b.top = b.top.min(f32::from(p.y));
            b.right = b.right.max(f32::from(p.x));
            b.bottom = b.bottom.max(f32::from(p.y));
        }
        let pad = if self.tool == Tool::Highlight {
            f32::from(self.width) * 3.
        } else {
            f32::from(self.width) / 2.
        };
        Some(Bounds {
            left: b.left - pad,
            top: b.top - pad,
            right: b.right + pad,
            bottom: b.bottom + pad,
        })
    }

    pub fn hit(&self, p: Point, text: Option<Bounds>) -> bool {
        if self.tool == Tool::Text {
            return text.is_some_and(|b| b.contains(p, 4.));
        }
        let Some(&a) = self.points.first() else {
            return false;
        };
        let b = *self.points.last().unwrap();
        let radius = if self.tool == Tool::Highlight {
            f32::from(self.width) * 3. + 4.
        } else {
            f32::from(self.width) / 2. + 5.
        };
        let mut points = self.points.clone();
        if self.tool == Tool::Box {
            points = vec![a, Point { x: b.x, y: a.y }, b, Point { x: a.x, y: b.y }, a];
        }
        if points.len() == 1 && distance(p, a, a) <= radius {
            return true;
        }
        if points
            .windows(2)
            .any(|line| distance(p, line[0], line[1]) <= radius)
        {
            return true;
        }
        if self.tool == Tool::Arrow {
            return self
                .arrow_head()
                .is_some_and(|head| head.iter().any(|&h| distance(p, h, b) <= radius));
        }
        false
    }

    pub fn translated(&self, dx: i16, dy: i16, w: u16, h: u16) -> Self {
        let mut mark = self.clone();
        if self.points.is_empty() {
            return mark;
        }
        let min_x = self.points.iter().map(|p| p.x).min().unwrap();
        let max_x = self.points.iter().map(|p| p.x).max().unwrap();
        let min_y = self.points.iter().map(|p| p.y).min().unwrap();
        let max_y = self.points.iter().map(|p| p.y).max().unwrap();
        let dx = i32::from(dx).clamp(-i32::from(min_x), i32::from(w) - 1 - i32::from(max_x));
        let dy = i32::from(dy).clamp(-i32::from(min_y), i32::from(h) - 1 - i32::from(max_y));
        for p in &mut mark.points {
            p.x = (i32::from(p.x) + dx) as i16;
            p.y = (i32::from(p.y) + dy) as i16;
        }
        mark
    }
}

fn distance(p: Point, a: Point, b: Point) -> f32 {
    let (dx, dy) = (f32::from(b.x - a.x), f32::from(b.y - a.y));
    let (px, py) = (f32::from(p.x - a.x), f32::from(p.y - a.y));
    let length = dx * dx + dy * dy;
    let t = if length > 0. {
        ((px * dx + py * dy) / length).clamp(0., 1.)
    } else {
        0.
    };
    (px - t * dx).hypot(py - t * dy)
}

#[derive(Default)]
pub(super) struct Document {
    pub marks: Vec<Mark>,
    undo: Vec<Vec<Mark>>,
    redo: Vec<Vec<Mark>>,
}

impl Document {
    pub fn record(&mut self, before: Vec<Mark>) {
        if self.marks != before {
            self.undo.push(before);
            self.redo.clear();
        }
    }
    pub fn push(&mut self, mark: Mark) {
        let before = self.marks.clone();
        self.marks.push(mark);
        self.record(before);
    }
    pub fn replace(&mut self, index: usize, mark: Mark) {
        let before = self.marks.clone();
        self.marks[index] = mark;
        self.record(before);
    }
    pub fn remove(&mut self, index: usize) {
        let before = self.marks.clone();
        self.marks.remove(index);
        self.record(before);
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn undo(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.marks, previous));
        }
    }
    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.marks, next));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn arrow() -> Mark {
        Mark {
            tool: Tool::Arrow,
            color: 0xff4545,
            width: 3,
            points: vec![Point { x: 10, y: 10 }, Point { x: 110, y: 110 }],
            text: Vec::new(),
        }
    }
    #[test]
    fn hit_testing_follows_strokes_and_arrowhead_not_the_whole_bounding_box() {
        let m = arrow();
        assert!(m.hit(Point { x: 60, y: 60 }, None));
        assert!(!m.hit(Point { x: 10, y: 100 }, None));
        assert!(m.hit(m.arrow_head().unwrap()[0], None));
        let mut rect = m;
        rect.tool = Tool::Box;
        assert!(rect.hit(Point { x: 60, y: 10 }, None));
        assert!(!rect.hit(Point { x: 60, y: 60 }, None));
    }
    #[test]
    fn moves_keep_shape_and_stay_inside_the_crop() {
        let moved = arrow().translated(-50, 250, 200, 200);
        assert_eq!((moved.points[0].x, moved.points[0].y), (0, 99));
        assert_eq!((moved.points[1].x, moved.points[1].y), (100, 199));
    }
    #[test]
    fn moves_edits_and_deletes_are_independent_undo_steps() {
        let mut doc = Document::default();
        let original = arrow();
        doc.push(original.clone());
        let moved = original.translated(20, 30, 200, 200);
        doc.replace(0, moved.clone());
        let mut edited = moved.clone();
        edited.color = 0xffffff;
        doc.replace(0, edited.clone());
        doc.remove(0);
        doc.undo();
        assert_eq!(doc.marks, vec![edited]);
        doc.undo();
        assert_eq!(doc.marks, vec![moved]);
        doc.undo();
        assert_eq!(doc.marks, vec![original]);
        doc.redo();
        assert_eq!(doc.marks[0].points[0].x, 30);
        let before = doc.marks.clone();
        doc.record(before);
        doc.redo();
        assert_eq!(doc.marks[0].color, 0xffffff);
    }
    #[test]
    fn committing_a_new_edit_discards_the_redo_branch() {
        let mut doc = Document::default();
        doc.push(arrow());
        doc.replace(0, arrow().translated(10, 10, 200, 200));
        doc.undo();
        doc.remove(0);
        doc.redo();
        assert!(doc.marks.is_empty());
    }
}
