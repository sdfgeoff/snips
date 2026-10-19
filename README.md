# snips

Small Rust region screenshot command for this computer's X11/i3 desktop.

Run `~/bin/snips`, then hold the left mouse button and drag over the area
you want. Release to save a uniquely named PNG in `~/tmp` and copy the PNG
image to the clipboard. Filenames use ISO 8601 local timestamps with a timezone
offset, for example `screenshot-2026-10-19T14:30:00.123456789+13:00.png`.
Escape, right-click, or a zero-sized selection cancels
without changing the clipboard or saving a file.

The image is taken when the command starts, before the selection outline is
drawn. The pointer is excluded. No editor or persistent application window opens.
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
`notify-send` is optional for error notifications. The build uses cached Rust
crates and needs no additional system packages.
