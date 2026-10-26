//! User input changes annotation state; drawing lives in render.rs.
use super::*;

pub(super) struct Drag {
    index: usize,
    start: Point,
    original: Mark,
    before: Vec<Mark>,
    endpoint: Option<usize>,
}

impl Editor<'_> {
    pub(super) fn finish(&mut self) {
        self.finish_drag();
        if let Some(mut mark) = self.draft.take() {
            if let Some(index) = self.editing.take() {
                if mark.text.is_empty() {
                    self.document.remove(index);
                    self.selected = None;
                } else {
                    self.document.replace(index, mark);
                    self.selected = Some(index);
                }
            } else if mark.tool != Tool::Text || !mark.text.is_empty() {
                if mark.tool != Tool::Text && mark.points.len() == 1 {
                    mark.points.push(mark.points[0]);
                }
                self.document.push(mark);
            }
        }
        self.text_all = false;
    }

    fn finish_drag(&mut self) {
        if let Some(drag) = self.drag.take() {
            self.document.record(drag.before);
        }
    }

    fn switch_tool(&mut self, tool: Tool) {
        self.finish();
        self.tool = tool;
        self.selected = None;
        self.last_click = None;
        if tool == Tool::Highlight {
            self.color = COLORS[1];
        }
    }

    fn edit_text(&mut self, index: usize) {
        self.finish();
        if self.document.marks[index].tool == Tool::Text {
            self.color = self.document.marks[index].color;
            self.width = self.document.marks[index].width;
            self.draft = Some(self.document.marks[index].clone());
            self.editing = Some(index);
            self.selected = Some(index);
            self.tool = Tool::Select;
            self.text_all = false;
        }
    }

    fn hit(&self, p: Point) -> Option<usize> {
        self.document
            .marks
            .iter()
            .enumerate()
            .rev()
            .find(|(_, m)| m.hit(p, self.mark_bounds(m)))
            .map(|(i, _)| i)
    }

    fn select(&mut self, p: Point, time: u32) {
        self.finish();
        let endpoint = self.selected.and_then(|i| {
            let m = &self.document.marks[i];
            if m.tool != Tool::Arrow {
                return None;
            }
            [0, m.points.len() - 1]
                .into_iter()
                .find(|&j| {
                    f32::from(p.x - m.points[j].x).hypot(f32::from(p.y - m.points[j].y)) <= 8.
                })
                .map(|j| (i, j))
        });
        let hit = endpoint.map(|(i, _)| i).or_else(|| self.hit(p));
        self.selected = hit;
        if let Some(index) = hit {
            let m = &self.document.marks[index];
            self.color = m.color;
            self.width = m.width;
            let double = self.last_click.is_some_and(|(last, when, at)| {
                last == index
                    && time.wrapping_sub(when) < 400
                    && (i32::from(p.x) - i32::from(at.x)).abs() < 5
                    && (i32::from(p.y) - i32::from(at.y)).abs() < 5
            });
            self.last_click = Some((index, time, p));
            if double && m.tool == Tool::Text {
                self.edit_text(index);
                return;
            }
            self.drag = Some(Drag {
                index,
                start: p,
                original: m.clone(),
                before: self.document.marks.clone(),
                endpoint: endpoint.map(|(_, j)| j),
            });
        } else {
            self.last_click = None;
        }
    }

    fn motion(&mut self, p: Point) {
        if let Some(drag) = &self.drag {
            let mut moved = drag.original.clone();
            if let Some(j) = drag.endpoint {
                moved.points[j] = p;
            } else {
                moved =
                    moved.translated(p.x - drag.start.x, p.y - drag.start.y, self.r.w, self.r.h);
            }
            if moved != drag.original {
                self.last_click = None;
            }
            self.document.marks[drag.index] = moved;
        } else if let Some(mark) = &mut self.draft {
            if mark.tool != Tool::Text {
                if matches!(mark.tool, Tool::Arrow | Tool::Box) {
                    mark.points.truncate(1);
                }
                if !mark.points.last().is_some_and(|a| a.x == p.x && a.y == p.y) {
                    mark.points.push(p);
                }
            }
        }
    }

    fn style(&mut self, color: Option<u32>) {
        if let Some(color) = color {
            self.color = color;
        } else {
            self.width = match self.width {
                3 => 6,
                6 => 10,
                _ => 3,
            };
        }
        if let Some(draft) = &mut self.draft {
            if color.is_some() {
                draft.color = self.color;
            } else {
                draft.width = self.width;
            }
        } else if let Some(index) = self.selected {
            let mut mark = self.document.marks[index].clone();
            if color.is_some() {
                mark.color = self.color;
            } else {
                mark.width = self.width;
            }
            self.document.replace(index, mark);
        }
    }

    fn action(&mut self, action: Action) -> Option<bool> {
        match action {
            Action::Color(i) => self.style(Some(COLORS[i])),
            Action::Width => self.style(None),
            Action::Tool(tool) => self.switch_tool(tool),
            Action::Undo => {
                self.finish();
                self.document.undo();
                self.selected = None;
                self.last_click = None;
            }
            Action::Cancel => return Some(false),
            Action::Save => {
                self.finish();
                return Some(true);
            }
        }
        None
    }

    fn key(&mut self, sym: u32, state: KeyButMask) -> Option<bool> {
        if sym == 0xff1b {
            return Some(false);
        }
        let ctrl = state.contains(KeyButMask::CONTROL);
        let shift = state.contains(KeyButMask::SHIFT);
        if ctrl && (sym == u32::from(b'z') || sym == u32::from(b'Z')) {
            self.finish();
            self.selected = None;
            self.last_click = None;
            if shift {
                self.document.redo();
            } else {
                self.document.undo();
            }
        } else if sym == 0xff0d || sym == 0xff8d {
            let text_active = self.draft.as_ref().is_some_and(|m| m.tool == Tool::Text);
            self.finish();
            if !text_active {
                return Some(true);
            }
        } else if self.draft.as_ref().is_some_and(|m| m.tool == Tool::Text) {
            let mark = self.draft.as_mut().unwrap();
            if ctrl && (sym == u32::from(b'a') || sym == u32::from(b'A')) {
                self.text_all = true;
            } else if sym == 0xff08 || sym == 0xffff {
                if self.text_all {
                    mark.text.clear();
                    self.text_all = false;
                } else if sym == 0xff08 {
                    mark.text.pop();
                }
            } else if (32..=126).contains(&sym) && !ctrl {
                if self.text_all {
                    mark.text.clear();
                    self.text_all = false;
                }
                if mark.text.len() < 240 {
                    let mut ch = sym as u8;
                    if ch.is_ascii_alphabetic() && state.contains(KeyButMask::LOCK) {
                        ch ^= 32;
                    }
                    mark.text.push(ch);
                }
            }
        } else if sym == 0xffbf {
            // F2 edits the selected text.
            if let Some(index) = self.selected {
                self.edit_text(index);
            }
        } else if sym == 0xffff || sym == 0xff08 {
            self.finish_drag();
            if let Some(index) = self.selected.take() {
                self.document.remove(index);
                self.last_click = None;
            }
        } else if !ctrl {
            match sym {
                0x76 => self.switch_tool(Tool::Select),
                0x70 => self.switch_tool(Tool::Pen),
                0x61 => self.switch_tool(Tool::Arrow),
                0x72 => self.switch_tool(Tool::Box),
                0x74 => self.switch_tool(Tool::Text),
                0x68 => self.switch_tool(Tool::Highlight),
                _ => {}
            }
        }
        None
    }

    fn press(&mut self, p: Point, time: u32) {
        if self.tool == Tool::Select {
            self.select(p, time);
            return;
        }
        self.finish();
        self.selected = None;
        if self.tool == Tool::Text {
            if let Some(index) = self
                .document
                .marks
                .iter()
                .enumerate()
                .rev()
                .find(|(_, m)| m.tool == Tool::Text && m.hit(p, self.mark_bounds(m)))
                .map(|(i, _)| i)
            {
                self.edit_text(index);
                return;
            }
        }
        self.draft = Some(Mark {
            tool: self.tool,
            color: self.color,
            width: self.width,
            points: vec![p],
            text: Vec::new(),
        });
    }

    fn point(&self, x: i16, y: i16) -> Point {
        Point {
            x: (i32::from(x) - i32::from(self.r.x)).clamp(0, i32::from(self.r.w) - 1) as i16,
            y: (i32::from(y) - i32::from(self.r.y)).clamp(0, i32::from(self.r.h) - 1) as i16,
        }
    }
    fn inside(&self, x: i16, y: i16) -> bool {
        i32::from(x) >= i32::from(self.r.x)
            && i32::from(x) < i32::from(self.r.x) + i32::from(self.r.w)
            && i32::from(y) >= i32::from(self.r.y)
            && i32::from(y) < i32::from(self.r.y) + i32::from(self.r.h)
    }

    pub(super) fn events(&mut self, mapping: &GetKeyboardMappingReply) -> Result<bool> {
        let mut pending = None;
        let mut dirty = false;
        let mut full = false;
        let mut last_frame = std::time::Instant::now();
        loop {
            let event = match pending.take() {
                Some(event) => event,
                None => self.conn.wait_for_event()?,
            };
            match event {
                Event::Expose(e) if e.count == 0 => {
                    full = true;
                    dirty = true;
                }
                Event::KeyPress(e) => {
                    let index = usize::from(e.detail - self.conn.setup().min_keycode)
                        * usize::from(mapping.keysyms_per_keycode);
                    let keys =
                        &mapping.keysyms[index..index + usize::from(mapping.keysyms_per_keycode)];
                    let mut sym =
                        keys[usize::from(e.state.contains(KeyButMask::SHIFT)).min(keys.len() - 1)];
                    if sym == 0 {
                        sym = keys[0];
                    }
                    if let Some(accepted) = self.key(sym, e.state) {
                        return Ok(accepted);
                    }
                    dirty = true;
                }
                Event::ButtonPress(e) if e.detail == 3 => return Ok(false),
                Event::ButtonPress(e) if e.detail == 1 => {
                    if self.bar.contains(e.root_x, e.root_y) {
                        if let Some(action) = self.bar.hit(e.root_x, e.root_y) {
                            if let Some(accepted) = self.action(action) {
                                return Ok(accepted);
                            }
                        }
                    } else if self.inside(e.root_x, e.root_y) {
                        self.press(self.point(e.root_x, e.root_y), e.time);
                    } else {
                        self.finish();
                        self.selected = None;
                    }
                    dirty = true;
                }
                Event::MotionNotify(e) if e.state.contains(KeyButMask::BUTTON1) => {
                    self.motion(self.point(e.root_x, e.root_y));
                    dirty = true;
                }
                Event::MotionNotify(e) => {
                    let hover = self.bar.hit(e.root_x, e.root_y);
                    if hover != self.hover {
                        self.hover = hover;
                        dirty = true;
                    }
                }
                Event::ButtonRelease(e) if e.detail == 1 => {
                    if self.drag.is_some() {
                        self.motion(self.point(e.root_x, e.root_y));
                        self.finish_drag();
                        dirty = true;
                    } else if self.draft.as_ref().is_some_and(|m| m.tool != Tool::Text) {
                        self.motion(self.point(e.root_x, e.root_y));
                        self.finish();
                        dirty = true;
                    }
                }
                _ => {}
            }
            pending = self.conn.poll_for_event()?;
            if dirty && (pending.is_none() || last_frame.elapsed().as_millis() >= 16) {
                self.repaint(full)?;
                dirty = false;
                full = false;
                last_frame = std::time::Instant::now();
            }
        }
    }
}
