# snips

Small Rust region screenshot command for this computer's X11/i3 desktop.

Run `~/bin/snips` (or press Print Screen), then hold the left mouse button
and drag over the area you want. Releasing opens an annotation overlay directly
on the screen, with the selected region bright and the surroundings dimmed.

Use the icon toolbar beneath the selection to select existing annotations,
or choose a pen, arrow, rectangle, text, or highlighter. Hover over a control
to see its name and shortcut. Pen, arrow, and rectangle edges are antialiased in both the preview and saved PNG.
Pick a color, click the line-width control to cycle through
3, 6, and 10 pixels, and use Undo to remove the last annotation. The toolbar
stays on the selection's monitor and moves above the region when needed. For
a selection that fills the screen, it sits inside the frame.

Press **Enter** or click **Save** to save the annotated PNG in `~/tmp` and copy
the image to the clipboard. To save without annotations, press Enter immediately
after selecting. **Escape**, right-click, or the toolbar's **Esc** button cancels
without saving or changing the clipboard. A zero-sized selection also cancels.

Keyboard shortcuts:

1. `V`: select and move; `P`: pen; `A`: arrow; `R`: rectangle; `T`: text; `H`: highlighter.
2. `Ctrl+Z`: undo; `Ctrl+Shift+Z`: redo.
3. Text: click inside the selection, type, and press Enter to finish the text.
   Press Enter again to save. Backspace edits the current text; `Ctrl+A` selects
   all of the current text for replacement.

To change an annotation, choose the pointer icon or press `V`, then click the
mark. Drag it to move it. An arrow has draggable endpoint handles for changing
its direction or length. Double-click a text label, or select it and press
`F2`, to edit its wording. The Text tool can also edit a label by clicking it.
Click a color or line width while a mark is selected to restyle it. `Delete`
or Backspace removes the selected mark. Undo and redo cover drawing, movement,
endpoint changes, text edits, styling, and deletion. Selection outlines and
handles are excluded from the saved image.

Filenames use ISO 8601 local timestamps with a timezone offset, for example
`screenshot-2026-10-19T14:30:00.123456789+13:00.png`.

The image is frozen when the command starts. The pointer, frame, toolbar,
and dimming are excluded from the saved image. Drawing is clipped to the
selected region. No separate editor window opens.
An `xclip` background process serves the clipboard until another application
replaces it. Errors are printed to stderr and sent through `notify-send`.

Add this to your i3 config and reload i3 to use the Print Screen key:

```i3
bindsym --release Print exec --no-startup-id /home/geoffrey/bin/snips
```

The `--release` option lets i3 release the shortcut before snips grabs input.
The command can also be used directly by an i3bar button.

To choose another destination:

```sh
snips --directory /tmp
```

Build and check:

```sh
cargo build --release --offline
cargo test --offline
cargo clippy --offline -- -D warnings
```

Runtime dependencies are the X11 server and `xclip`;
`notify-send` is optional for error notifications. Text uses the installed
DejaVu Sans font at `/usr/share/fonts/TTF/DejaVuSans.ttf`. The overlay uses X11 with
Rust vector and font rasterization; it needs no GUI toolkit.

The opt-in desktop regression check uses a temporary synthetic test surface.
It moves the pointer and replaces the clipboard, then closes its windows and
restores the pointer. With tkinter, Pillow, xrandr, and xdotool installed, run:

```sh
python3 tests/desktop_smoke.py
```

Editor code is split into `model` (geometry and undo history), `input`,
`toolbar` (shared layout and hit targets), and `render`. Text and vector paths
use the same premultiplied RGBA compositor.
