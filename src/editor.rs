//! In-place annotation. The canvas pixmap contains only exportable pixels;
//! the frame and toolbar are drawn separately on the overlay window.
use crate::{graphics, Region, Result};
use std::{cell::RefCell, collections::HashMap};
use tiny_skia::{PathBuilder, Pixmap as Raster, Transform};
type GlyphCache = RefCell<HashMap<(char, u16), (fontdue::Metrics, Vec<u8>)>>;
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    rust_connection::RustConnection,
};

const COLORS: [u32; 6] = [0xff4545, 0xffde45, 0x36cf82, 0x46a4ff, 0xffffff, 0x16191f];
const BAR_W: u16 = 746;
const BAR_H: u16 = 54;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tool {
    Pen,
    Arrow,
    Box,
    Text,
    Highlight,
}

#[derive(Clone, Debug)]
struct Mark {
    tool: Tool,
    color: u32,
    width: u16,
    points: Vec<Point>,
    text: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
struct Bar {
    x: i16,
    y: i16,
}

impl Bar {
    fn new(r: Region, screen_w: u16, screen_h: u16) -> Self {
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

    fn hit(self, x: i16, y: i16) -> Option<usize> {
        let (x, y) = (
            i32::from(x) - i32::from(self.x),
            i32::from(y) - i32::from(self.y),
        );
        if !(0..i32::from(BAR_H)).contains(&y) {
            return None;
        }
        match x {
            8..=217 => Some(((x - 8) / 42) as usize),
            230..=397 => Some(5 + ((x - 230) / 28) as usize),
            410..=471 => Some(11),
            480..=537 => Some(12),
            548..=599 => Some(13),
            610..=737 => Some(14),
            _ => None,
        }
    }

    fn contains(self, x: i16, y: i16) -> bool {
        x >= self.x
            && i32::from(x) < i32::from(self.x) + i32::from(BAR_W)
            && y >= self.y
            && i32::from(y) < i32::from(self.y) + i32::from(BAR_H)
    }
}

struct Editor<'a> {
    conn: &'a RustConnection,
    screen: &'a Screen,
    window: Window,
    surface: Pixmap,
    original: Pixmap,
    background: Pixmap,
    canvas: Pixmap,
    gc: Gcontext,
    typeface: fontdue::Font,
    glyphs: GlyphCache,
    r: Region,
    bar: Bar,
    marks: Vec<Mark>,
    redo: Vec<Mark>,
    draft: Option<Mark>,
    tool: Tool,
    color: u32,
    width: u16,
    visual: Visualtype,
    hover: Option<usize>,
}

pub(super) fn annotate(
    conn: &RustConnection,
    screen: &Screen,
    snapshot: &GetImageReply,
    r: Region,
    mapping: &GetKeyboardMappingReply,
) -> Result<Option<GetImageReply>> {
    let visual = *screen
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == screen.root_visual)
        .ok_or("missing root visual")?;
    if visual.class != VisualClass::TRUE_COLOR {
        return Err("only TrueColor displays are supported".into());
    }
    let gc = conn.generate_id()?;
    conn.create_gc(gc, screen.root, &CreateGCAux::new().graphics_exposures(0))?
        .check()?;
    let original = conn.generate_id()?;
    let background = conn.generate_id()?;
    let canvas = conn.generate_id()?;
    let surface = conn.generate_id()?;
    for (id, w, h) in [
        (original, screen.width_in_pixels, screen.height_in_pixels),
        (surface, screen.width_in_pixels, screen.height_in_pixels),
        (background, screen.width_in_pixels, screen.height_in_pixels),
        (canvas, r.w, r.h),
    ] {
        conn.create_pixmap(screen.root_depth, id, screen.root, w, h)?
            .check()?;
    }
    let format = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|f| f.depth == snapshot.depth)
        .ok_or("unsupported pixel format")?;
    let stride = (usize::from(screen.width_in_pixels) * usize::from(format.bits_per_pixel))
        .div_ceil(usize::from(format.scanline_pad))
        * usize::from(format.scanline_pad)
        / 8;
    upload(conn, original, gc, screen, stride, &snapshot.data)?;
    let mut dim = snapshot.data.clone();
    let bytes = usize::from(format.bits_per_pixel).div_ceil(8);
    if bytes > 4 {
        return Err("unsupported pixel size".into());
    }
    for row in dim.chunks_mut(stride) {
        for pixel in row[..usize::from(screen.width_in_pixels) * bytes].chunks_mut(bytes) {
            let mut value = 0u32;
            for (i, b) in pixel.iter().enumerate() {
                let shift = if conn.setup().image_byte_order == ImageOrder::LSB_FIRST {
                    i
                } else {
                    bytes - 1 - i
                };
                value |= u32::from(*b) << (shift * 8);
            }
            let mut dark = value & !(visual.red_mask | visual.green_mask | visual.blue_mask);
            for mask in [visual.red_mask, visual.green_mask, visual.blue_mask] {
                let shift = mask.trailing_zeros();
                dark |= (((value & mask) >> shift) * 2 / 5) << shift;
            }
            for (i, b) in pixel.iter_mut().enumerate() {
                let shift = if conn.setup().image_byte_order == ImageOrder::LSB_FIRST {
                    i
                } else {
                    bytes - 1 - i
                };
                *b = (dark >> (shift * 8)) as u8;
            }
        }
    }
    upload(conn, background, gc, screen, stride, &dim)?;
    let window = conn.generate_id()?;
    conn.create_window(
        screen.root_depth,
        window,
        screen.root,
        0,
        0,
        screen.width_in_pixels,
        screen.height_in_pixels,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .override_redirect(1)
            .background_pixmap(background)
            .event_mask(EventMask::EXPOSURE),
    )?
    .check()?;
    let name = b"snips annotation";
    conn.change_property(
        PropMode::REPLACE,
        window,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        8,
        name.len() as u32,
        name,
    )?;
    let typeface = fontdue::Font::from_bytes(
        std::fs::read("/usr/share/fonts/TTF/DejaVuSans.ttf")?,
        fontdue::FontSettings::default(),
    )
    .map_err(|e| format!("cannot load annotation font: {e}"))?;
    conn.map_window(window)?.check()?;
    conn.configure_window(
        window,
        &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
    )?;
    let mut editor = Editor {
        conn,
        screen,
        window,
        surface,
        original,
        background,
        canvas,
        gc,
        typeface,
        glyphs: RefCell::new(HashMap::new()),
        r,
        bar: monitor_bar(conn, screen, r),
        marks: Vec::new(),
        redo: Vec::new(),
        draft: None,
        tool: Tool::Pen,
        color: COLORS[0],
        width: 3,
        visual,
        hover: None,
    };
    editor.repaint(true)?;
    let accepted = editor.events(mapping)?;
    let result = if accepted {
        editor.render_canvas()?;
        Some(
            conn.get_image(ImageFormat::Z_PIXMAP, canvas, 0, 0, r.w, r.h, u32::MAX)?
                .reply()?,
        )
    } else {
        None
    };
    conn.destroy_window(window)?;
    for pixmap in [original, background, canvas, surface] {
        conn.free_pixmap(pixmap)?;
    }
    conn.free_gc(gc)?;
    conn.flush()?;
    Ok(result)
}

fn monitor_bar(conn: &RustConnection, screen: &Screen, r: Region) -> Bar {
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

fn upload(
    conn: &RustConnection,
    drawable: Drawable,
    gc: Gcontext,
    screen: &Screen,
    stride: usize,
    data: &[u8],
) -> Result<()> {
    // Stay below the core protocol's 256 KiB limit (also works without BIG-REQUESTS).
    let rows = (240_000 / stride).max(1);
    for (i, chunk) in data.chunks(rows * stride).enumerate() {
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            drawable,
            gc,
            screen.width_in_pixels,
            (chunk.len() / stride) as u16,
            0,
            (i * rows) as i16,
            0,
            screen.root_depth,
            chunk,
        )?
        .check()?;
    }
    Ok(())
}

impl Editor<'_> {
    fn pixel(&self, color: u32) -> u32 {
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

    fn paint(&self, color: u32, width: u16) -> Result<()> {
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

    fn rect(&self, drawable: Drawable, x: i16, y: i16, w: u16, h: u16, color: u32) -> Result<()> {
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

    fn text(
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
        // Blend glyph coverage with the actual pixels beneath it, including annotations.
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
            .ok_or("unsupported text pixel format")?;
        let bytes = usize::from(format.bits_per_pixel).div_ceil(8);
        let stride = (usize::from(w) * usize::from(format.bits_per_pixel))
            .div_ceil(usize::from(format.scanline_pad))
            * usize::from(format.scanline_pad)
            / 8;
        let foreground = self.pixel(color);
        let little = self.conn.setup().image_byte_order == ImageOrder::LSB_FIRST;
        for (ch, gx, gy) in placements {
            let (metrics, coverage) = &glyphs[&(ch, size)];
            for py in 0..metrics.height {
                for px in 0..metrics.width {
                    let (dx, dy) = (gx + px as i32, gy + py as i32);
                    if dx < left || dx >= right || dy < top || dy >= bottom {
                        continue;
                    }
                    let alpha = u32::from(coverage[py * metrics.width + px]);
                    if alpha == 0 {
                        continue;
                    }
                    let offset = (dy - top) as usize * stride + (dx - left) as usize * bytes;
                    let pixel = &mut image.data[offset..offset + bytes];
                    let mut value = 0u32;
                    for (i, &b) in pixel.iter().enumerate() {
                        value |= u32::from(b) << (if little { i } else { bytes - 1 - i } * 8);
                    }
                    for mask in [
                        self.visual.red_mask,
                        self.visual.green_mask,
                        self.visual.blue_mask,
                    ] {
                        let shift = mask.trailing_zeros();
                        let a = (value & mask) >> shift;
                        let b = (foreground & mask) >> shift;
                        value = (value & !mask)
                            | (((a * (255 - alpha) + b * alpha + 127) / 255) << shift);
                    }
                    for (i, b) in pixel.iter_mut().enumerate() {
                        *b = (value >> (if little { i } else { bytes - 1 - i } * 8)) as u8;
                    }
                }
            }
        }
        self.paint(color, 1)?;
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

    // Composite premultiplied RGBA pixels onto an X11 drawable. Reading its
    // actual background keeps edge coverage correct over screenshots and marks.
    fn composite(&self, drawable: Drawable, x: i16, y: i16, overlay: &Raster) -> Result<()> {
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

    fn line(&self, drawable: Drawable, points: &[Point], color: u32, width: u16) -> Result<()> {
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

    fn path(
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

    fn mark(&self, m: &Mark) -> Result<()> {
        let Some(&a) = m.points.first() else {
            return Ok(());
        };
        let b = *m.points.last().unwrap();
        match m.tool {
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
                let angle = f64::from(b.y - a.y).atan2(f64::from(b.x - a.x));
                let length = (f64::from(m.width) * 4.0 + 8.0)
                    .min(f64::from(b.x - a.x).hypot(f64::from(b.y - a.y)) * 0.5);
                let head = |offset: f64| Point {
                    x: b.x - (length * (angle + offset).cos()) as i16,
                    y: b.y - (length * (angle + offset).sin()) as i16,
                };
                self.line(self.canvas, &[head(0.5), b, head(-0.5)], m.color, m.width)?;
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

    fn render_canvas(&self) -> Result<()> {
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
        for mark in &self.marks {
            self.mark(mark)?;
        }
        if let Some(mark) = &self.draft {
            self.mark(mark)?;
        }
        Ok(())
    }

    fn repaint(&self, full: bool) -> Result<()> {
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

    fn toolbar(&self) -> Result<()> {
        let (x, y) = (self.bar.x, self.bar.y);
        // Restore pixels under rounded corners, including an overlapping crop.
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
        for i in [0, 1, 2, 3, 4, 11, 12, 13, 14] {
            let (bx, w) = match i {
                0..=4 => (8. + i as f32 * 42., 40.),
                11 => (410., 62.),
                12 => (480., 58.),
                13 => (548., 52.),
                _ => (610., 128.),
            };
            let selected = i < 5
                && self.tool
                    == [
                        Tool::Pen,
                        Tool::Arrow,
                        Tool::Box,
                        Tool::Text,
                        Tool::Highlight,
                    ][i];
            let hovered = self.hover == Some(i);
            let button = graphics::round_rect(bx, 8., w, 38., 8.);
            if i == 14 {
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
            let color = if i == 12 && self.marks.is_empty() {
                0x657183
            } else if selected {
                0x88e8f4
            } else {
                0xe3e9f0
            };
            if i < 5 {
                graphics::icon(&mut image, i, bx + 8., 15., color);
            } else if i == 12 || i == 13 {
                graphics::icon(&mut image, i, bx + (w - 24.) / 2., 14., color);
            } else if i == 14 {
                graphics::icon(&mut image, i, bx + 5., 15., 0xffffff);
            }
        }
        for bx in [224., 404., 476., 542., 604.] {
            let mut p = PathBuilder::new();
            p.move_to(bx, 17.);
            p.line_to(bx, 37.);
            graphics::stroke(&mut image, &p.finish().unwrap(), 0x414b59, 1.);
        }
        for (i, &color) in COLORS.iter().enumerate() {
            let cx = 243. + i as f32 * 28.;
            if self.color == color {
                let ring = PathBuilder::from_circle(cx, 27., 12.).unwrap();
                graphics::stroke(&mut image, &ring, 0xeaf1f8, 1.5);
            } else if self.hover == Some(i + 5) {
                graphics::circle(&mut image, cx, 27., 12., 0x4c596a);
            }
            graphics::circle(&mut image, cx, 27., 9., color);
            if i == 5 {
                graphics::stroke(
                    &mut image,
                    &PathBuilder::from_circle(cx, 27., 9.).unwrap(),
                    0x8b96a5,
                    1.,
                );
            }
        }
        // A line sample makes the thickness control recognizable without a label.
        let mut sample = PathBuilder::new();
        sample.move_to(419., 27.);
        sample.line_to(435., 27.);
        graphics::stroke(
            &mut image,
            &sample.finish().unwrap(),
            0xdce5ef,
            f32::from(self.width).min(6.),
        );
        graphics::fill(
            &mut image,
            &graphics::round_rect(686., 16., 44., 23., 5.),
            if self.hover == Some(14) {
                0x17804f
            } else {
                0x116d43
            },
        );
        self.composite(self.surface, x, y, &image)?;
        self.text(
            self.surface,
            x + 443,
            y + 32,
            format!("{}", self.width).as_bytes(),
            0xe3e9f0,
            14,
        )?;
        self.text(self.surface, x + 646, y + 32, b"Save", 0xffffff, 14)?;
        self.text(self.surface, x + 692, y + 31, b"Enter", 0xe2f9ed, 11)?;
        let hint = match self.hover {
            Some(0) => "Pen · P",
            Some(1) => "Arrow · A",
            Some(2) => "Rectangle · R",
            Some(3) => "Text · T",
            Some(4) => "Highlighter · H",
            Some(5..=10) => "Choose an annotation color",
            Some(11) => "Line width · click to cycle 3 / 6 / 10 px",
            Some(12) => "Undo · Ctrl+Z     Redo · Ctrl+Shift+Z",
            Some(13) => "Cancel · Escape",
            Some(14) => "Save PNG and copy image to clipboard · Enter",
            _ if self.tool == Tool::Text => {
                "Click to place text · Type · Enter to finish · Enter again to save"
            }
            _ => "Draw to annotate     Enter to save & copy     Esc to cancel",
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

    fn finish(&mut self) {
        if let Some(mark) = self.draft.take() {
            if mark.tool != Tool::Text || !mark.text.is_empty() {
                self.marks.push(mark);
                self.redo.clear();
            }
        }
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

    fn events(&mut self, mapping: &GetKeyboardMappingReply) -> Result<bool> {
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
                    let shifted = e.state.contains(KeyButMask::SHIFT);
                    let mut sym = keys[usize::from(shifted).min(keys.len() - 1)];
                    if sym == 0 {
                        sym = keys[0];
                    }
                    if sym == 0xff1b {
                        return Ok(false);
                    }
                    let text_active = self.draft.as_ref().is_some_and(|m| m.tool == Tool::Text);
                    if sym == 0xff0d || sym == 0xff8d {
                        if text_active {
                            self.finish();
                        } else {
                            self.finish();
                            return Ok(true);
                        }
                    } else if text_active {
                        let mark = self.draft.as_mut().unwrap();
                        if sym == 0xff08 {
                            mark.text.pop();
                        } else if (32..=126).contains(&sym)
                            && mark.text.len() < 240
                            && !e.state.contains(KeyButMask::CONTROL)
                        {
                            let mut ch = sym as u8;
                            if ch.is_ascii_alphabetic() && e.state.contains(KeyButMask::LOCK) {
                                ch ^= 32;
                            }
                            mark.text.push(ch);
                        }
                    } else if e.state.contains(KeyButMask::CONTROL)
                        && (sym == u32::from(b'z') || sym == u32::from(b'Z'))
                    {
                        if shifted {
                            if let Some(mark) = self.redo.pop() {
                                self.marks.push(mark);
                            }
                        } else if let Some(mark) = self.marks.pop() {
                            self.redo.push(mark);
                        }
                    } else {
                        match sym {
                            0x70 => self.tool = Tool::Pen,
                            0x61 => self.tool = Tool::Arrow,
                            0x72 => self.tool = Tool::Box,
                            0x74 => self.tool = Tool::Text,
                            0x68 => {
                                self.tool = Tool::Highlight;
                                self.color = COLORS[1];
                            }
                            _ => {}
                        }
                    }
                    dirty = true;
                }
                Event::ButtonPress(e) if e.detail == 3 => return Ok(false),
                Event::ButtonPress(e) if e.detail == 1 => {
                    if self.bar.contains(e.root_x, e.root_y) {
                        self.finish();
                        match self.bar.hit(e.root_x, e.root_y) {
                            Some(i @ 0..=4) => {
                                self.tool = [
                                    Tool::Pen,
                                    Tool::Arrow,
                                    Tool::Box,
                                    Tool::Text,
                                    Tool::Highlight,
                                ][i];
                                if self.tool == Tool::Highlight {
                                    self.color = COLORS[1];
                                }
                            }
                            Some(i @ 5..=10) => self.color = COLORS[i - 5],
                            Some(11) => {
                                self.width = match self.width {
                                    3 => 6,
                                    6 => 10,
                                    _ => 3,
                                }
                            }
                            Some(12) => {
                                if let Some(mark) = self.marks.pop() {
                                    self.redo.push(mark);
                                }
                            }
                            Some(13) => return Ok(false),
                            Some(14) => return Ok(true),
                            _ => {}
                        }
                    } else if self.inside(e.root_x, e.root_y) {
                        self.finish();
                        self.draft = Some(Mark {
                            tool: self.tool,
                            color: self.color,
                            width: self.width,
                            points: vec![self.point(e.root_x, e.root_y)],
                            text: Vec::new(),
                        });
                    }
                    dirty = true;
                }
                Event::MotionNotify(e) if e.state.contains(KeyButMask::BUTTON1) => {
                    let point = self.point(e.root_x, e.root_y);
                    if let Some(mark) = &mut self.draft {
                        if mark.tool != Tool::Text {
                            if matches!(mark.tool, Tool::Arrow | Tool::Box) {
                                mark.points.truncate(1);
                            }
                            if !mark
                                .points
                                .last()
                                .is_some_and(|p| p.x == point.x && p.y == point.y)
                            {
                                mark.points.push(point);
                            }
                            dirty = true;
                        }
                    }
                }
                Event::MotionNotify(e) => {
                    let hover = self.bar.hit(e.root_x, e.root_y);
                    if hover != self.hover {
                        self.hover = hover;
                        dirty = true;
                    }
                }
                Event::ButtonRelease(e) if e.detail == 1 => {
                    if self.draft.as_ref().is_some_and(|m| m.tool != Tool::Text) {
                        let point = self.point(e.root_x, e.root_y);
                        let mark = self.draft.as_mut().unwrap();
                        if matches!(mark.tool, Tool::Arrow | Tool::Box) {
                            mark.points.truncate(1);
                        }
                        mark.points.push(point);
                        self.finish();
                        dirty = true;
                    }
                }
                _ => {}
            }
            // Process queued input before painting, retaining every pen point and
            // keystroke while avoiding a render for every individual event.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_fits_screen_and_avoids_selection_when_possible() {
        for r in [
            Region {
                x: 10,
                y: 20,
                w: 500,
                h: 300,
            },
            Region {
                x: 1400,
                y: 800,
                w: 500,
                h: 270,
            },
        ] {
            let b = Bar::new(r, 1920, 1080);
            assert!(b.x >= 0 && i32::from(b.x) + i32::from(BAR_W) <= 1920);
            assert!(
                i32::from(b.y) >= i32::from(r.y) + i32::from(r.h)
                    || i32::from(b.y) + i32::from(BAR_H) <= i32::from(r.y)
            );
            assert_eq!(b.hit(b.x + 620, b.y + 20), Some(14));
            assert_eq!(b.hit(b.x + 245, b.y + 20), Some(5));
            assert_eq!(b.hit(b.x + 20, b.y - 1), None);
        }
    }
}
