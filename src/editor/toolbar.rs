use super::*;

pub(super) const COLORS: [u32; 6] = [0xff4545, 0xffde45, 0x36cf82, 0x46a4ff, 0xffffff, 0x16191f];
pub(super) const BAR_W: u16 = 788;
pub(super) const BAR_H: u16 = 54;

#[derive(Clone, Copy, Debug)]
pub(super) struct Bar {
    pub(super) x: i16,
    pub(super) y: i16,
}

impl Bar {
    pub(super) fn new(r: Region, screen_w: u16, screen_h: u16) -> Self {
        let x = (i32::from(r.x) + i32::from(r.w) / 2 - i32::from(BAR_W) / 2)
            .clamp(0, (i32::from(screen_w) - i32::from(BAR_W)).max(0)) as i16;
        let below = i32::from(r.y) + i32::from(r.h) + 12;
        let y = if below + i32::from(BAR_H) + 24 <= i32::from(screen_h) {
            below
        } else if i32::from(r.y) >= i32::from(BAR_H) + 36 {
            i32::from(r.y) - i32::from(BAR_H) - 32
        } else {
            (i32::from(screen_h) - i32::from(BAR_H) - 24).max(0)
        } as i16;
        Self { x, y }
    }

    pub(super) fn hit(self, x: i16, y: i16) -> Option<Action> {
        let (x, y) = (
            i32::from(x) - i32::from(self.x),
            i32::from(y) - i32::from(self.y),
        );
        if !(8..46).contains(&y) {
            return None;
        }
        BUTTONS
            .iter()
            .find(|b| x >= i32::from(b.x) && x < i32::from(b.x + b.width))
            .map(|b| b.action)
    }

    pub(super) fn contains(self, x: i16, y: i16) -> bool {
        x >= self.x
            && i32::from(x) < i32::from(self.x) + i32::from(BAR_W)
            && y >= self.y
            && i32::from(y) < i32::from(self.y) + i32::from(BAR_H)
    }
}

pub(super) fn monitor_bar(conn: &RustConnection, screen: &Screen, r: Region) -> Bar {
    let monitors = x11rb::protocol::randr::get_monitors(conn, screen.root, true)
        .ok()
        .and_then(|cookie| cookie.reply().ok());
    let monitor = monitors.as_ref().and_then(|reply| {
        reply.monitors.iter().max_by_key(|m| {
            let w = (i32::from(r.x) + i32::from(r.w)).min(i32::from(m.x) + i32::from(m.width))
                - i32::from(r.x).max(i32::from(m.x));
            let h = (i32::from(r.y) + i32::from(r.h)).min(i32::from(m.y) + i32::from(m.height))
                - i32::from(r.y).max(i32::from(m.y));
            w.max(0) * h.max(0)
        })
    });
    if let Some(m) = monitor {
        let local = Region {
            x: r.x - m.x,
            y: r.y - m.y,
            ..r
        };
        let bar = Bar::new(local, m.width, m.height);
        Bar {
            x: bar.x + m.x,
            y: bar.y + m.y,
        }
    } else {
        Bar::new(r, screen.width_in_pixels, screen.height_in_pixels)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Action {
    Tool(Tool),
    Color(usize),
    Width,
    Undo,
    Cancel,
    Save,
}
struct Button {
    action: Action,
    x: i16,
    width: i16,
}
const BUTTONS: [Button; 16] = [
    Button {
        action: Action::Tool(Tool::Select),
        x: 8,
        width: 40,
    },
    Button {
        action: Action::Tool(Tool::Pen),
        x: 50,
        width: 40,
    },
    Button {
        action: Action::Tool(Tool::Arrow),
        x: 92,
        width: 40,
    },
    Button {
        action: Action::Tool(Tool::Box),
        x: 134,
        width: 40,
    },
    Button {
        action: Action::Tool(Tool::Text),
        x: 176,
        width: 40,
    },
    Button {
        action: Action::Tool(Tool::Highlight),
        x: 218,
        width: 40,
    },
    Button {
        action: Action::Color(0),
        x: 272,
        width: 28,
    },
    Button {
        action: Action::Color(1),
        x: 300,
        width: 28,
    },
    Button {
        action: Action::Color(2),
        x: 328,
        width: 28,
    },
    Button {
        action: Action::Color(3),
        x: 356,
        width: 28,
    },
    Button {
        action: Action::Color(4),
        x: 384,
        width: 28,
    },
    Button {
        action: Action::Color(5),
        x: 412,
        width: 28,
    },
    Button {
        action: Action::Width,
        x: 452,
        width: 62,
    },
    Button {
        action: Action::Undo,
        x: 522,
        width: 58,
    },
    Button {
        action: Action::Cancel,
        x: 590,
        width: 52,
    },
    Button {
        action: Action::Save,
        x: 652,
        width: 128,
    },
];

impl Editor<'_> {
    pub(super) fn toolbar(&self) -> Result<()> {
        let (x, y) = (self.bar.x, self.bar.y);
        self.conn.copy_area(
            self.background,
            self.surface,
            self.gc,
            x,
            y,
            x,
            y,
            BAR_W,
            BAR_H,
        )?;
        let left = i32::from(x).max(i32::from(self.r.x));
        let top = i32::from(y).max(i32::from(self.r.y));
        let right =
            (i32::from(x) + i32::from(BAR_W)).min(i32::from(self.r.x) + i32::from(self.r.w));
        let bottom =
            (i32::from(y) + i32::from(BAR_H)).min(i32::from(self.r.y) + i32::from(self.r.h));
        if right > left && bottom > top {
            self.conn.copy_area(
                self.canvas,
                self.surface,
                self.gc,
                (left - i32::from(self.r.x)) as i16,
                (top - i32::from(self.r.y)) as i16,
                left as i16,
                top as i16,
                (right - left) as u16,
                (bottom - top) as u16,
            )?;
        }
        let mut image = Raster::new(u32::from(BAR_W), u32::from(BAR_H)).unwrap();
        let outer =
            graphics::round_rect(0.5, 0.5, f32::from(BAR_W) - 1., f32::from(BAR_H) - 1., 13.);
        graphics::fill(&mut image, &outer, 0x202630);
        graphics::stroke(&mut image, &outer, 0x46505e, 1.);
        for b in &BUTTONS {
            let bx = f32::from(b.x);
            let width = f32::from(b.width);
            if let Action::Color(index) = b.action {
                let cx = bx + 13.;
                if self.color == COLORS[index] {
                    graphics::stroke(
                        &mut image,
                        &PathBuilder::from_circle(cx, 27., 12.).unwrap(),
                        0xeaf1f8,
                        1.5,
                    );
                } else if self.hover == Some(b.action) {
                    graphics::circle(&mut image, cx, 27., 12., 0x4c596a);
                }
                graphics::circle(&mut image, cx, 27., 9., COLORS[index]);
                if index == 5 {
                    graphics::stroke(
                        &mut image,
                        &PathBuilder::from_circle(cx, 27., 9.).unwrap(),
                        0x8b96a5,
                        1.,
                    );
                }
                continue;
            }
            let selected = b.action == Action::Tool(self.tool);
            let hovered = self.hover == Some(b.action);
            let button = graphics::round_rect(bx, 8., width, 38., 8.);
            if b.action == Action::Save {
                graphics::fill(
                    &mut image,
                    &button,
                    if hovered { 0x20a96c } else { 0x168653 },
                );
            } else if selected || hovered {
                graphics::fill(
                    &mut image,
                    &button,
                    if selected { 0x284c59 } else { 0x343e4b },
                );
                if selected {
                    graphics::stroke(&mut image, &button, 0x53c9de, 1.);
                }
            }
            let color = if b.action == Action::Undo && !self.document.can_undo() {
                0x657183
            } else if selected {
                0x88e8f4
            } else {
                0xe3e9f0
            };
            let icon = match b.action {
                Action::Tool(t) => Some(match t {
                    Tool::Select => 15,
                    Tool::Pen => 0,
                    Tool::Arrow => 1,
                    Tool::Box => 2,
                    Tool::Text => 3,
                    Tool::Highlight => 4,
                }),
                Action::Undo => Some(12),
                Action::Cancel => Some(13),
                Action::Save => Some(14),
                _ => None,
            };
            if let Some(icon) = icon {
                graphics::icon(
                    &mut image,
                    icon,
                    bx + if b.action == Action::Save {
                        5.
                    } else {
                        (width - 24.) / 2.
                    },
                    14.,
                    color,
                );
            }
            if b.action == Action::Width {
                let mut path = PathBuilder::new();
                path.move_to(bx + 9., 27.);
                path.line_to(bx + 25., 27.);
                graphics::stroke(
                    &mut image,
                    &path.finish().unwrap(),
                    color,
                    f32::from(self.width).min(6.),
                );
            }
        }
        for bx in [266., 446., 518., 584., 646.] {
            let mut p = PathBuilder::new();
            p.move_to(bx, 17.);
            p.line_to(bx, 37.);
            graphics::stroke(&mut image, &p.finish().unwrap(), 0x414b59, 1.);
        }
        graphics::fill(
            &mut image,
            &graphics::round_rect(728., 16., 44., 23., 5.),
            if self.hover == Some(Action::Save) {
                0x17804f
            } else {
                0x116d43
            },
        );
        self.composite(self.surface, x, y, &image)?;
        self.text(
            self.surface,
            x + 485,
            y + 32,
            self.width.to_string().as_bytes(),
            0xe3e9f0,
            14,
        )?;
        self.text(self.surface, x + 688, y + 32, b"Save", 0xffffff, 14)?;
        self.text(self.surface, x + 734, y + 31, b"Enter", 0xe2f9ed, 11)?;
        let hint = match self.hover {
            Some(Action::Tool(Tool::Select))=>"Select & move · V",
            Some(Action::Tool(Tool::Pen))=>"Pen · P",
            Some(Action::Tool(Tool::Arrow))=>"Arrow · A",
            Some(Action::Tool(Tool::Box))=>"Rectangle · R",
            Some(Action::Tool(Tool::Text))=>"Text · T",
            Some(Action::Tool(Tool::Highlight))=>"Highlighter · H",
            Some(Action::Color(_))=>"Choose an annotation color",
            Some(Action::Width)=>"Line width · click to cycle 3 / 6 / 10 px",
            Some(Action::Undo)=>"Undo · Ctrl+Z     Redo · Ctrl+Shift+Z",
            Some(Action::Cancel)=>"Cancel · Escape",
            Some(Action::Save)=>"Save PNG and copy image to clipboard · Enter",
            _ if self.editing.is_some()=>"Edit text · Ctrl+A to replace · Enter to finish",
            _ if self.selected.is_some()=>"Drag to move · Arrow handles reshape · Double-click text or F2 to edit · Delete to remove",
            _ if self.tool==Tool::Select=>"Click an annotation to select it · Drag to move · Double-click text to edit",
            _ if self.tool==Tool::Text=>"Click to place or edit text · Enter to finish · Enter again to save",
            _=>"Draw to annotate     Enter to save & copy     Esc to cancel",
        };
        if i32::from(y) + i32::from(BAR_H) + 20 <= i32::from(self.screen.height_in_pixels) {
            self.conn.copy_area(
                self.background,
                self.surface,
                self.gc,
                x,
                y + BAR_H as i16,
                x,
                y + BAR_H as i16,
                BAR_W,
                20,
            )?;
            let advance: f32 = hint
                .chars()
                .map(|ch| self.typeface.metrics(ch, 12.).advance_width)
                .sum();
            self.text(
                self.surface,
                x + ((f32::from(BAR_W) - advance) / 2.).round() as i16,
                y + BAR_H as i16 + 16,
                hint.as_bytes(),
                0xd0d8e2,
                12,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn controls_have_distinct_hit_targets_on_their_visible_buttons() {
        let bar = Bar::new(
            Region {
                x: 10,
                y: 20,
                w: 500,
                h: 300,
            },
            1920,
            1080,
        );
        for button in &BUTTONS {
            assert_eq!(
                bar.hit(bar.x + button.x + button.width / 2, bar.y + 27),
                Some(button.action)
            );
        }
        assert_eq!(bar.hit(bar.x + 2, bar.y + 2), None);
        assert!(bar.x >= 0 && i32::from(bar.x) + i32::from(BAR_W) <= 1920);
    }
}
