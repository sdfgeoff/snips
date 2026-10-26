use std::{
    error::Error,
    fs,
    io::BufWriter,
    path::PathBuf,
    process::{Command, Stdio},
};
use x11rb::{
    connection::Connection,
    protocol::{xproto::*, Event},
    CURRENT_TIME,
};

mod editor;
mod graphics;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Region {
    x: i16,
    y: i16,
    w: u16,
    h: u16,
}

fn region(a: (i16, i16), b: (i16, i16)) -> Region {
    Region {
        x: a.0.min(b.0),
        y: a.1.min(b.1),
        w: a.0.abs_diff(b.0),
        h: a.1.abs_diff(b.1),
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("snips: {e}");
        let _ = Command::new("notify-send")
            .args(["snips failed", &e.to_string()])
            .status();
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut output = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unset")?
        .join("tmp");
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            println!("snips [--directory PATH]\nDrag to select, then annotate. Enter saves; Escape or right-click cancels.\nSaves a PNG in ~/tmp and copies the image to the X11 clipboard.");
            return Ok(());
        } else if arg == "--directory" {
            output = args.next().ok_or("--directory needs a path")?.into();
        } else {
            return Err(format!("unknown argument: {}", arg.to_string_lossy()).into());
        }
    }
    Command::new("xclip")
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "xclip is required to copy images")?;
    fs::create_dir_all(&output)?;
    let (conn, screen_num) = x11rb::connect(None)?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let font = conn.generate_id()?;
    conn.open_font(font, b"cursor")?.check()?;
    let cursor = conn.generate_id()?;
    conn.create_glyph_cursor(cursor, font, font, 34, 35, 65535, 65535, 65535, 0, 0, 0)?
        .check()?;
    let pointer = conn
        .grab_pointer(
            false,
            root,
            EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
            GrabMode::ASYNC,
            GrabMode::ASYNC,
            root,
            cursor,
            CURRENT_TIME,
        )?
        .reply()?;
    if pointer.status != GrabStatus::SUCCESS {
        return Err("pointer is busy; try again after releasing the shortcut".into());
    }
    let keyboard = conn
        .grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?
        .reply()?;
    if keyboard.status != GrabStatus::SUCCESS {
        return Err("keyboard is busy".into());
    }
    let mapping = conn
        .get_keyboard_mapping(
            conn.setup().min_keycode,
            conn.setup().max_keycode - conn.setup().min_keycode + 1,
        )?
        .reply()?;
    let escape: Vec<u8> = mapping
        .keysyms
        .chunks(mapping.keysyms_per_keycode as usize)
        .enumerate()
        .filter(|(_, keys)| keys.contains(&0xff1b))
        .map(|(i, _)| conn.setup().min_keycode + i as u8)
        .collect();
    // Snapshot before drawing the outline so no selector pixels enter the image.
    let snapshot = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            root,
            0,
            0,
            screen.width_in_pixels,
            screen.height_in_pixels,
            u32::MAX,
        )?
        .reply()?;
    let mut borders = Vec::new();
    for _ in 0..4 {
        let win = conn.generate_id()?;
        conn.create_window(
            screen.root_depth,
            win,
            root,
            0,
            0,
            1,
            1,
            1,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(screen.white_pixel)
                .border_pixel(screen.black_pixel),
        )?
        .check()?;
        borders.push(win);
    }
    conn.flush()?;
    let mut start = None;
    let selected = loop {
        match conn.wait_for_event()? {
            Event::KeyPress(e) if escape.contains(&e.detail) => break None,
            Event::ButtonPress(e) if e.detail == 3 => break None,
            Event::ButtonPress(e) if e.detail == 1 => start = Some((e.root_x, e.root_y)),
            Event::MotionNotify(e) if start.is_some() => {
                let r = region(start.unwrap(), (e.root_x, e.root_y));
                if r.w > 0 && r.h > 0 {
                    let edges = [
                        (r.x, r.y, r.w, 1),
                        (r.x, r.y + r.h as i16 - 1, r.w, 1),
                        (r.x, r.y, 1, r.h),
                        (r.x + r.w as i16 - 1, r.y, 1, r.h),
                    ];
                    for (&win, (x, y, w, h)) in borders.iter().zip(edges) {
                        conn.configure_window(
                            win,
                            &ConfigureWindowAux::new()
                                .x(i32::from(x))
                                .y(i32::from(y))
                                .width(u32::from(w))
                                .height(u32::from(h))
                                .stack_mode(StackMode::ABOVE),
                        )?;
                        conn.map_window(win)?;
                    }
                    conn.flush()?;
                }
            }
            Event::ButtonRelease(e) if e.detail == 1 && start.is_some() => {
                let r = region(start.unwrap(), (e.root_x, e.root_y));
                break (r.w > 0 && r.h > 0).then_some(r);
            }
            _ => {}
        }
    };
    for win in borders {
        conn.destroy_window(win)?;
    }
    conn.flush()?;
    let annotated = match selected {
        Some(r) => editor::annotate(&conn, screen, &snapshot, r, &mapping)?,
        None => None,
    };
    conn.ungrab_pointer(CURRENT_TIME)?;
    conn.ungrab_keyboard(CURRENT_TIME)?;
    conn.free_cursor(cursor)?;
    conn.close_font(font)?;
    conn.flush()?;
    let (Some(r), Some(snapshot)) = (selected, annotated) else {
        return Ok(());
    };
    let format = conn
        .setup()
        .pixmap_formats
        .iter()
        .find(|f| f.depth == snapshot.depth)
        .ok_or("unsupported pixel depth")?;
    let visual = screen
        .allowed_depths
        .iter()
        .flat_map(|d| &d.visuals)
        .find(|v| v.visual_id == screen.root_visual)
        .ok_or("missing root visual")?;
    if visual.class != VisualClass::TRUE_COLOR {
        return Err("only TrueColor displays are supported".into());
    }
    let bytes = usize::from(format.bits_per_pixel).div_ceil(8);
    let pad = usize::from(format.scanline_pad);
    let stride = (usize::from(r.w) * usize::from(format.bits_per_pixel)).div_ceil(pad) * pad / 8;
    let mut rgb = Vec::with_capacity(usize::from(r.w) * usize::from(r.h) * 3);
    for y in 0..usize::from(r.h) {
        for x in 0..usize::from(r.w) {
            let offset = y * stride + x * bytes;
            let mut pixel = 0u32;
            for (i, &b) in snapshot.data[offset..offset + bytes].iter().enumerate() {
                let shift = if conn.setup().image_byte_order == ImageOrder::LSB_FIRST {
                    i
                } else {
                    bytes - 1 - i
                };
                pixel |= u32::from(b) << (shift * 8);
            }
            for mask in [visual.red_mask, visual.green_mask, visual.blue_mask] {
                let value = (pixel & mask) >> mask.trailing_zeros();
                rgb.push((u64::from(value) * 255 / u64::from(mask >> mask.trailing_zeros())) as u8);
            }
        }
    }
    let file = tempfile::Builder::new()
        .prefix("screenshot-")
        .suffix(".png")
        .tempfile_in(&output)?;
    {
        let mut encoder = png::Encoder::new(
            BufWriter::new(file.as_file()),
            u32::from(r.w),
            u32::from(r.h),
        );
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&rgb)?;
    }
    let path = output.join(format!(
        "screenshot-{}.png",
        chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, false)
    ));
    // Publish without overwriting an existing screenshot, even on a name collision.
    file.persist_noclobber(&path)?;
    // xclip forks into the background and owns the clipboard until replaced.
    let status = Command::new("xclip")
        .args(["-selection", "clipboard", "-t", "image/png", "-i"])
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(format!("saved {}, but copying failed", path.display()).into());
    }
    println!("{}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_works_in_every_direction() {
        let expected = Region {
            x: 10,
            y: 20,
            w: 90,
            h: 60,
        };
        for (a, b) in [
            ((10, 20), (100, 80)),
            ((100, 80), (10, 20)),
            ((10, 80), (100, 20)),
            ((100, 20), (10, 80)),
        ] {
            assert_eq!(region(a, b), expected);
        }
    }
}
