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

mod input;
mod model;
mod render;
mod toolbar;
use model::{Bounds, Document, Mark, Tool};
use toolbar::{monitor_bar, Action, Bar, BAR_H, BAR_W, COLORS};

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
    document: Document,
    selected: Option<usize>,
    editing: Option<usize>,
    drag: Option<input::Drag>,
    last_click: Option<(usize, u32, Point)>,
    text_all: bool,
    draft: Option<Mark>,
    tool: Tool,
    color: u32,
    width: u16,
    visual: Visualtype,
    hover: Option<Action>,
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
        document: Document::default(),
        selected: None,
        editing: None,
        drag: None,
        last_click: None,
        text_all: false,
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
