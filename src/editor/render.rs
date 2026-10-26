use super::*;

impl Editor<'_> {
    pub(super) fn pixel(&self, color: u32) -> u32 {
        let mut pixel = 0;
        for (component, mask) in [
            (color >> 16, self.visual.red_mask),
            (color >> 8, self.visual.green_mask),
            (color, self.visual.blue_mask),
        ] {
            let shift = mask.trailing_zeros();
            pixel |= (((component & 255) * (mask >> shift) / 255) << shift) & mask;
        }
        pixel
    }

    pub(super) fn paint(&self, color: u32, width: u16) -> Result<()> {
        self.conn.change_gc(
            self.gc,
            &ChangeGCAux::new()
                .foreground(self.pixel(color))
                .line_width(u32::from(width))
                .cap_style(CapStyle::ROUND)
                .join_style(JoinStyle::ROUND)
                .fill_style(FillStyle::SOLID)
                .function(GX::COPY),
        )?;
        Ok(())
    }

    pub(super) fn rect(
        &self,
        drawable: Drawable,
        x: i16,
        y: i16,
        w: u16,
        h: u16,
        color: u32,
    ) -> Result<()> {
        self.paint(color, 1)?;
        self.conn.poly_fill_rectangle(
            drawable,
            self.gc,
            &[Rectangle {
                x,
                y,
                width: w,
                height: h,
            }],
        )?;
        Ok(())
    }

    pub(super) fn text(
        &self,
        drawable: Drawable,
        x: i16,
        y: i16,
        text: &[u8],
        color: u32,
        size: u16,
    ) -> Result<()> {
        let (width, height) = if drawable == self.canvas {
            (self.r.w, self.r.h)
        } else {
            (self.screen.width_in_pixels, self.screen.height_in_pixels)
        };
        let mut glyphs = self.glyphs.borrow_mut();
        let mut placements = Vec::new();
        let mut pen = f32::from(x);
        let (mut left, mut top, mut right, mut bottom) =
            (i32::from(width), i32::from(height), 0, 0);
        for ch in String::from_utf8_lossy(text).chars() {
            let (metrics, _) = glyphs
                .entry((ch, size))
                .or_insert_with(|| self.typeface.rasterize(ch, f32::from(size)));
            let gx = pen.round() as i32 + metrics.xmin;
            let gy = i32::from(y) - metrics.height as i32 - metrics.ymin;
            if metrics.width > 0 && metrics.height > 0 {
                left = left.min(gx);
                top = top.min(gy);
                right = right.max(gx + metrics.width as i32);
                bottom = bottom.max(gy + metrics.height as i32);
                placements.push((ch, gx, gy));
            }
            pen += metrics.advance_width;
        }
        left = left.max(0);
        top = top.max(0);
        right = right.min(i32::from(width));
        bottom = bottom.min(i32::from(height));
        if right <= left || bottom <= top {
            return Ok(());
        }
        let w = (right - left) as u16;
        let h = (bottom - top) as u16;
        let mut image = Raster::new(u32::from(w), u32::from(h)).ok_or("text too large")?;
        for (ch, gx, gy) in placements {
            let (metrics, coverage) = &glyphs[&(ch, size)];
            let mut glyph =
                Raster::new(metrics.width as u32, metrics.height as u32).ok_or("invalid glyph")?;
            for (pixel, &alpha) in glyph.data_mut().chunks_exact_mut(4).zip(coverage) {
                for (channel, shift) in [16, 8, 0].iter().enumerate() {
                    pixel[channel] =
                        ((((color >> shift) & 255) * u32::from(alpha) + 127) / 255) as u8;
                }
                pixel[3] = alpha;
            }
            image.draw_pixmap(
                gx - left,
                gy - top,
                glyph.as_ref(),
                &tiny_skia::PixmapPaint::default(),
                Transform::identity(),
                None,
            );
        }
        self.composite(drawable, left as i16, top as i16, &image)
    }

    // Composite premultiplied RGBA pixels onto an X11 drawable. Reading its
    // actual background keeps edge coverage correct over screenshots and marks.
    pub(super) fn composite(
        &self,
        drawable: Drawable,
        x: i16,
        y: i16,
        overlay: &Raster,
    ) -> Result<()> {
        let (limit_w, limit_h) = if drawable == self.canvas {
            (self.r.w, self.r.h)
        } else {
            (self.screen.width_in_pixels, self.screen.height_in_pixels)
        };
        let left = i32::from(x).max(0);
        let top = i32::from(y).max(0);
        let right = (i32::from(x) + overlay.width() as i32).min(i32::from(limit_w));
        let bottom = (i32::from(y) + overlay.height() as i32).min(i32::from(limit_h));
        if right <= left || bottom <= top {
            return Ok(());
        }
        let (w, h) = ((right - left) as u16, (bottom - top) as u16);
        let mut image = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                drawable,
                left as i16,
                top as i16,
                w,
                h,
                u32::MAX,
            )?
            .reply()?;
        let format = self
            .conn
            .setup()
            .pixmap_formats
            .iter()
            .find(|f| f.depth == image.depth)
            .ok_or("unsupported image format")?;
        let bytes = usize::from(format.bits_per_pixel).div_ceil(8);
        let stride = (usize::from(w) * usize::from(format.bits_per_pixel))
            .div_ceil(usize::from(format.scanline_pad))
            * usize::from(format.scanline_pad)
            / 8;
        let little = self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        for dy in 0..usize::from(h) {
            for dx in 0..usize::from(w) {
                let p = overlay
                    .pixel(
                        (left - i32::from(x)) as u32 + dx as u32,
                        (top - i32::from(y)) as u32 + dy as u32,
                    )
                    .unwrap();
                let alpha = u32::from(p.alpha());
                if alpha == 0 {
                    continue;
                }
                let offset = dy * stride + dx * bytes;
                let pixel = &mut image.data[offset..offset + bytes];
                let mut value = 0u32;
                for (i, &b) in pixel.iter().enumerate() {
                    value |= u32::from(b) << (if little { i } else { bytes - 1 - i } * 8);
                }
                for (src, mask) in [
                    (p.red(), self.visual.red_mask),
                    (p.green(), self.visual.green_mask),
                    (p.blue(), self.visual.blue_mask),
                ] {
                    let shift = mask.trailing_zeros();
                    let max = mask >> shift;
                    let dst = (value & mask) >> shift;
                    let blended =
                        (u32::from(src) * max / 255 + (dst * (255 - alpha) + 127) / 255).min(max);
                    value = (value & !mask) | (blended << shift);
                }
                for (i, b) in pixel.iter_mut().enumerate() {
                    *b = (value >> (if little { i } else { bytes - 1 - i } * 8)) as u8;
                }
            }
        }
        self.paint(0xffffff, 1)?;
        let rows = (240_000 / stride).max(1);
        for (i, data) in image.data.chunks(rows * stride).enumerate() {
            self.conn.put_image(
                ImageFormat::Z_PIXMAP,
                drawable,
                self.gc,
                w,
                (data.len() / stride) as u16,
                left as i16,
                (top + (i * rows) as i32) as i16,
                0,
                image.depth,
                data,
            )?;
        }
        Ok(())
    }

    pub(super) fn line(
        &self,
        drawable: Drawable,
        points: &[Point],
        color: u32,
        width: u16,
    ) -> Result<()> {
        let mut p = PathBuilder::new();
        if let Some(a) = points.first() {
            p.move_to(f32::from(a.x), f32::from(a.y));
            for b in &points[1..] {
                p.line_to(f32::from(b.x), f32::from(b.y));
            }
            // Tiny-skia needs a nonzero segment to render a round dot.
            if points.iter().all(|b| b.x == a.x && b.y == a.y) {
                p.line_to(f32::from(a.x) + 0.01, f32::from(a.y));
            }
            if let Some(path) = p.finish() {
                self.path(drawable, &path, color, width)?;
            }
        }
        Ok(())
    }

    pub(super) fn path(
        &self,
        drawable: Drawable,
        path: &tiny_skia::Path,
        color: u32,
        width: u16,
    ) -> Result<()> {
        let pad = f32::from(width) / 2.0 + 2.0;
        let bounds = path.bounds();
        let (w, h) = if drawable == self.canvas {
            (self.r.w, self.r.h)
        } else {
            (self.screen.width_in_pixels, self.screen.height_in_pixels)
        };
        let x = (bounds.left() - pad).floor().max(0.) as i16;
        let y = (bounds.top() - pad).floor().max(0.) as i16;
        let right = (bounds.right() + pad).ceil().min(f32::from(w));
        let bottom = (bounds.bottom() + pad).ceil().min(f32::from(h));
        if right <= f32::from(x) || bottom <= f32::from(y) {
            return Ok(());
        }
        let mut image = Raster::new(
            (right - f32::from(x)) as u32,
            (bottom - f32::from(y)) as u32,
        )
        .ok_or("stroke too large")?;
        let local = path
            .clone()
            .transform(Transform::from_translate(-f32::from(x), -f32::from(y)))
            .ok_or("invalid stroke")?;
        graphics::stroke(&mut image, &local, color, f32::from(width));
        self.composite(drawable, x, y, &image)
    }

    pub(super) fn mark(&self, m: &Mark) -> Result<()> {
        let Some(&a) = m.points.first() else {
            return Ok(());
        };
        let b = *m.points.last().unwrap();
        match m.tool {
            Tool::Select => {}
            Tool::Pen => self.line(self.canvas, &m.points, m.color, m.width)?,
            Tool::Highlight => {
                // AND gives a transparent marker-like tint without alpha support.
                self.paint(m.color, m.width * 6)?;
                self.conn
                    .change_gc(self.gc, &ChangeGCAux::new().function(GX::AND))?;
                for start in (0..m.points.len().saturating_sub(1)).step_by(16000) {
                    let chunk = &m.points[start..(start + 16001).min(m.points.len())];
                    self.conn
                        .poly_line(CoordMode::ORIGIN, self.canvas, self.gc, chunk)?;
                }
                self.conn
                    .change_gc(self.gc, &ChangeGCAux::new().function(GX::COPY))?;
            }
            Tool::Arrow => {
                self.line(self.canvas, &[a, b], m.color, m.width)?;
                if let Some(head) = m.arrow_head() {
                    self.line(self.canvas, &[head[0], b, head[1]], m.color, m.width)?;
                }
            }
            Tool::Box => {
                let r = crate::region((a.x, a.y), (b.x, b.y));
                if let Some(rect) = tiny_skia::Rect::from_xywh(
                    f32::from(r.x),
                    f32::from(r.y),
                    f32::from(r.w),
                    f32::from(r.h),
                ) {
                    self.path(self.canvas, &PathBuilder::from_rect(rect), m.color, m.width)?;
                }
            }
            Tool::Text => self.text(self.canvas, a.x, a.y, &m.text, m.color, 24)?,
        }
        Ok(())
    }

    pub(super) fn mark_bounds(&self, mark: &Mark) -> Option<Bounds> {
        let text = if mark.tool == Tool::Text {
            let p = *mark.points.first()?;
            let mut pen = f32::from(p.x);
            let mut bounds = Bounds {
                left: pen,
                right: pen + 2.,
                top: f32::from(p.y) - 24.,
                bottom: f32::from(p.y) + 4.,
            };
            for ch in String::from_utf8_lossy(&mark.text).chars() {
                let m = self.typeface.metrics(ch, 24.);
                bounds.left = bounds.left.min(pen + m.xmin as f32);
                bounds.top = bounds
                    .top
                    .min(f32::from(p.y) - m.height as f32 - m.ymin as f32);
                bounds.bottom = bounds.bottom.max(f32::from(p.y) - m.ymin as f32);
                pen += m.advance_width;
            }
            bounds.right = pen.max(bounds.right);
            Some(bounds)
        } else {
            None
        };
        mark.bounds(text)
    }

    fn selection(&self) -> Result<()> {
        let Some(index) = self.selected else {
            return Ok(());
        };
        let mark = self
            .draft
            .as_ref()
            .filter(|_| self.editing == Some(index))
            .unwrap_or(&self.document.marks[index]);
        let Some(b) = self.mark_bounds(mark) else {
            return Ok(());
        };
        let left = (b.left - 4.).max(2.);
        let top = (b.top - 4.).max(2.);
        let right = (b.right + 4.).min(f32::from(self.r.w) - 2.);
        let bottom = (b.bottom + 4.).min(f32::from(self.r.h) - 2.);
        if let Some(rect) = tiny_skia::Rect::from_xywh(
            left + f32::from(self.r.x),
            top + f32::from(self.r.y),
            right - left,
            bottom - top,
        ) {
            self.path(self.surface, &PathBuilder::from_rect(rect), 0x70d6ed, 1)?;
            if self.text_all {
                let mut tint =
                    Raster::new((right - left).ceil() as u32, (bottom - top).ceil() as u32)
                        .ok_or("selection too large")?;
                tint.fill(tiny_skia::Color::from_rgba8(70, 150, 255, 55));
                self.composite(
                    self.surface,
                    self.r.x + left as i16,
                    self.r.y + top as i16,
                    &tint,
                )?;
            }
        }
        if mark.tool == Tool::Arrow && self.r.w >= 10 && self.r.h >= 10 {
            for p in [mark.points[0], *mark.points.last().unwrap()] {
                let mut handle = Raster::new(10, 10).unwrap();
                graphics::circle(&mut handle, 5., 5., 4., 0xffffff);
                graphics::circle(&mut handle, 5., 5., 2.5, 0x227b94);
                let x = (i32::from(p.x) - 5).clamp(0, i32::from(self.r.w) - 10) as i16;
                let y = (i32::from(p.y) - 5).clamp(0, i32::from(self.r.h) - 10) as i16;
                self.composite(self.surface, self.r.x + x, self.r.y + y, &handle)?;
            }
        }
        Ok(())
    }

    pub(super) fn render_canvas(&self) -> Result<()> {
        self.conn.copy_area(
            self.original,
            self.canvas,
            self.gc,
            self.r.x,
            self.r.y,
            0,
            0,
            self.r.w,
            self.r.h,
        )?;
        for (index, mark) in self.document.marks.iter().enumerate() {
            if Some(index) == self.editing {
                if let Some(draft) = &self.draft {
                    self.mark(draft)?;
                }
            } else {
                self.mark(mark)?;
            }
        }
        if self.editing.is_none() {
            if let Some(mark) = &self.draft {
                self.mark(mark)?;
            }
        }
        Ok(())
    }

    pub(super) fn repaint(&self, full: bool) -> Result<()> {
        if full {
            self.conn.copy_area(
                self.background,
                self.surface,
                self.gc,
                0,
                0,
                0,
                0,
                self.screen.width_in_pixels,
                self.screen.height_in_pixels,
            )?;
        }
        self.render_canvas()?;
        self.conn.copy_area(
            self.canvas,
            self.surface,
            self.gc,
            0,
            0,
            self.r.x,
            self.r.y,
            self.r.w,
            self.r.h,
        )?;
        self.selection()?;
        if let Some(mark) = self.draft.as_ref().filter(|m| m.tool == Tool::Text) {
            let p = mark.points[0];
            let advance: f32 = String::from_utf8_lossy(&mark.text)
                .chars()
                .map(|ch| self.typeface.metrics(ch, 24.0).advance_width)
                .sum();
            let x = (f32::from(self.r.x + p.x) + advance).round() as i16;
            self.line(
                self.surface,
                &[
                    Point {
                        x,
                        y: self.r.y + p.y - 16,
                    },
                    Point {
                        x,
                        y: self.r.y + p.y + 3,
                    },
                ],
                mark.color,
                1,
            )?;
        }
        self.paint(0x57d7f0, 1)?;
        self.conn.poly_rectangle(
            self.surface,
            self.gc,
            &[Rectangle {
                x: self.r.x,
                y: self.r.y,
                width: self.r.w.saturating_sub(1),
                height: self.r.h.saturating_sub(1),
            }],
        )?;
        for (x, y) in [
            (self.r.x, self.r.y),
            (self.r.x + self.r.w as i16 - 6, self.r.y),
            (self.r.x, self.r.y + self.r.h as i16 - 6),
            (
                self.r.x + self.r.w as i16 - 6,
                self.r.y + self.r.h as i16 - 6,
            ),
        ] {
            if self.r.w >= 6 && self.r.h >= 6 {
                self.rect(self.surface, x, y, 6, 6, 0x57d7f0)?;
            }
        }
        self.toolbar()?;
        // Present a completed frame in one operation to avoid toolbar flicker.
        if full {
            self.conn.copy_area(
                self.surface,
                self.window,
                self.gc,
                0,
                0,
                0,
                0,
                self.screen.width_in_pixels,
                self.screen.height_in_pixels,
            )?;
        } else {
            self.conn.copy_area(
                self.surface,
                self.window,
                self.gc,
                self.r.x,
                self.r.y,
                self.r.x,
                self.r.y,
                self.r.w,
                self.r.h,
            )?;
            self.conn.copy_area(
                self.surface,
                self.window,
                self.gc,
                self.bar.x,
                self.bar.y,
                self.bar.x,
                self.bar.y,
                BAR_W,
                BAR_H + 20,
            )?;
        }
        self.conn.flush()?;
        Ok(())
    }
}
