#!/usr/bin/env python3
"""Opt-in live X11 regression checks on a temporary, synthetic test surface.

Build snips first, then run this script. Requires tkinter, Pillow, xrandr,
xdotool and xclip. It briefly moves the pointer and replaces the clipboard.
The pointer is restored and the temporary windows/files are removed on exit.
"""
import argparse
import io
import pathlib
import re
import subprocess as sp
import tempfile
import time
import tkinter as tk

from PIL import Image

BACKGROUND = (230, 232, 235)
RED = (255, 69, 69)
PROJECT = pathlib.Path(__file__).resolve().parents[1]


class Desktop:
    def __init__(self, binary, folder):
        self.binary, self.folder, self.process = str(binary), pathlib.Path(folder), None
        monitor = sp.check_output(['xrandr', '--listmonitors'], text=True)
        match = re.search(r'(\d+)/\d+x(\d+)/\d+\+(-?\d+)\+(-?\d+)', monitor)
        if not match:
            raise RuntimeError('No active monitor found')
        w, h, mx, my = map(int, match.groups())
        if w < 1400 or h < 1000:
            raise RuntimeError('Smoke test needs a monitor of at least 1400x1000')
        mouse = sp.check_output(['xdotool', 'getmouselocation', '--shell'], text=True)
        self.mouse = {k: int(v) for k, v in re.findall(r'^(X|Y)=(\d+)$', mouse, re.M)}
        self.fx, self.fy = mx + 120, my + 100
        self.x, self.y = self.fx + 100, self.fy + 150
        self.bar_x, self.bar_y = self.x + 56, self.y + 412
        self.root = tk.Tk()
        self.root.overrideredirect(True)
        self.root.attributes('-topmost', True)
        self.root.geometry(f'1200x850+{self.fx}+{self.fy}')
        canvas = tk.Canvas(self.root, bg='#e6e8eb', highlightthickness=0)
        canvas.pack(fill='both', expand=True)
        canvas.create_text(40, 40, anchor='nw', text='snips regression test', font=('sans', 20))
        canvas.create_text(40, 85, anchor='nw', text='Temporary synthetic surface; closes automatically.', font=('sans', 12))
        self.settle()

    def settle(self):
        # Let the test surface handle Expose events after the overlay closes.
        for _ in range(5):
            self.root.update()
            time.sleep(.1)

    def xd(self, *args):
        sp.run(['xdotool', *map(str, args)], check=True, stdout=sp.DEVNULL)
        self.root.update()
        time.sleep(.08)

    def key(self, *args):
        self.xd('key', *args)

    def click(self, x, y):
        self.xd('mousemove', self.x + x, self.y + y, 'click', 1)

    def drag(self, x, y, u, v):
        self.xd('mousemove', self.x + x, self.y + y, 'mousedown', 1)
        self.xd('mousemove', self.x + u, self.y + v)
        self.xd('mouseup', 1)

    def start(self):
        self.settle()
        self.process = sp.Popen([self.binary, '--directory', str(self.folder)], stdout=sp.PIPE, stderr=sp.PIPE, text=True)
        time.sleep(.6)
        self.drag(0, 0, 900, 400)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise AssertionError(self.process.communicate())
            if sp.run(['xdotool', 'search', '--onlyvisible', '--name', '^snips annotation$'], capture_output=True).returncode == 0:
                time.sleep(.4)
                return
            time.sleep(.1)
        raise AssertionError('Annotation overlay did not appear')

    def text(self, x, y, value):
        self.key('t')
        self.click(x, y)
        self.xd('type', '--clearmodifiers', value)
        self.key('Return')

    def save(self, button=False):
        if button:
            self.xd('mousemove', self.bar_x + 675, self.bar_y + 27, 'click', 1)
        else:
            self.key('Return')
        out, err = self.process.communicate(timeout=15)
        assert self.process.returncode == 0, (out, err)
        path = pathlib.Path(out.strip())
        assert re.match(r'screenshot-\d{4}-\d\d-\d\dT', path.name), path
        data = path.read_bytes()
        clip = sp.check_output(['xclip', '-selection', 'clipboard', '-t', 'image/png', '-o'], timeout=5)
        assert clip == data, 'Clipboard differs from the saved PNG'
        image = Image.open(io.BytesIO(data)).convert('RGB')
        assert image.size == (900, 400), image.size
        assert image.getpixel((0, 0)) == BACKGROUND, 'Selection frame leaked into export'
        return image

    def close(self):
        if self.process and self.process.poll() is None:
            self.process.terminate()
            self.process.wait(timeout=5)
        self.root.destroy()
        self.xd_restore()

    def xd_restore(self):
        sp.run(['xdotool', 'mousemove', str(self.mouse['X']), str(self.mouse['Y'])], check=False)


def check(d):
    d.start()
    blank = d.save()
    assert blank.tobytes() == bytes(BACKGROUND) * (900 * 400)

    d.start()
    d.drag(50, 50, 300, 100)
    d.key('ctrl+z')
    assert d.save().tobytes() == blank.tobytes(), 'Undo did not restore the original'

    d.start()
    d.drag(50, 50, 300, 100)
    d.key('ctrl+z')
    d.key('ctrl+shift+z')
    d.key('a')
    d.drag(100, 170, 400, 170)
    d.key('r')
    d.drag(450, 50, 750, 250)
    d.key('h')
    d.drag(50, 280, 350, 280)
    d.text(450, 320, 'Looks good')
    image = d.save(button=True)
    assert image.getpixel((150, 70)) == RED
    assert image.getpixel((200, 280)) == (230, 200, 65)
    assert any(230 < r < 255 and 69 < g < 232 for r, g, b in image.crop((40, 40, 310, 110)).get_flattened_data()), 'No antialiased edge pixels'

    # Moving an arrow and reshaping an endpoint are separate undoable edits.
    d.start()
    d.key('a')
    d.drag(100, 100, 350, 150)
    d.key('v')
    d.drag(225, 125, 325, 185)
    d.drag(450, 210, 500, 290)
    d.key('Delete')
    d.key('ctrl+z')
    d.key('ctrl+shift+z')
    d.key('ctrl+z')
    image = d.save()
    assert image.getpixel((225, 125)) == BACKGROUND, 'Old arrow position remains'
    assert image.getpixel((350, 225)) == RED, 'Arrow endpoint was not reshaped/restored'
    assert image.getpixel((230, 173)) == RED, 'Arrow translation was lost'

    # Reference glyph pixels for an unchanged label.
    d.start()
    d.text(450, 320, 'Draft')
    original_text = d.save()
    d.start()
    d.text(450, 320, 'Draft')
    d.key('v')
    d.drag(460, 310, 540, 270)
    d.key('F2')
    d.key('ctrl+a')
    d.xd('type', '--clearmodifiers', 'Edited')
    d.key('Return')
    d.key('ctrl+z')
    moved_text = d.save()
    assert moved_text.crop((520, 250, 610, 290)).tobytes() == original_text.crop((440, 290, 530, 330)).tobytes(), 'Text move or edit undo changed the original label'
    assert moved_text.crop((440, 290, 520, 330)).tobytes() == blank.crop((440, 290, 520, 330)).tobytes(), 'Text was not removed from its old position'

    # Double-click editing preserves position and redo restores the edited text.
    d.start()
    d.text(530, 280, 'Edited')
    expected = d.save()
    d.start()
    d.text(450, 320, 'Draft')
    d.key('v')
    d.drag(460, 310, 540, 270)
    d.xd('mousemove', d.x + 540, d.y + 270, 'click', '--repeat', 2, '--delay', 90, 1)
    d.key('ctrl+a')
    d.xd('type', '--clearmodifiers', 'Edited')
    d.key('Return')
    d.key('ctrl+z')
    d.key('ctrl+shift+z')
    assert d.save().tobytes() == expected.tobytes(), 'Text edit/redo differs from directly placing the same text'

    # Editing lower text must not bring it above later annotations.
    d.start()
    d.text(530, 280, 'Edited')
    d.key('p')
    d.xd('mousemove', d.bar_x + 369, d.bar_y + 27, 'click', 1)
    d.drag(520, 270, 650, 270)
    layered_reference = d.save()
    d.start()
    d.text(530, 280, 'Draft')
    d.key('p')
    d.xd('mousemove', d.bar_x + 369, d.bar_y + 27, 'click', 1)
    d.drag(520, 270, 650, 270)
    d.key('t')
    d.click(540, 277)
    d.key('ctrl+a')
    d.xd('type', '--clearmodifiers', 'Edited')
    d.key('Return')
    assert d.save().tobytes() == layered_reference.tobytes(), 'Editing text changed annotation order'

    before = set(d.folder.glob('screenshot-*.png'))
    clipboard = sp.check_output(['xclip', '-selection', 'clipboard', '-t', 'image/png', '-o'])
    d.start()
    d.key('Escape')
    out, err = d.process.communicate(timeout=5)
    assert d.process.returncode == 0, (out, err)
    assert before == set(d.folder.glob('screenshot-*.png'))
    assert clipboard == sp.check_output(['xclip', '-selection', 'clipboard', '-t', 'image/png', '-o'])
    print('PASS: drawing, undo/redo, annotation movement, arrow handles, deletion, F2/double-click text editing, PNG export, clipboard, antialiasing and cancellation.')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, default=PROJECT / 'target/release/snips')
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='snips-smoke-') as folder:
        desktop = Desktop(args.binary, folder)
        try:
            check(desktop)
        finally:
            desktop.close()
