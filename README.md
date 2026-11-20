# Roomcheck

A reference plugin for checking how a mix translates to small speakers and real rooms.
Built in pure Rust with `nice-plug` + `nice-plug-egui`; ships as **VST3** and **CLAP**
for macOS. The GUI is a hand-painted 1160×740 recreation of `Design/Main.dc.html`.

## What is included

Three real, factory-captured IRs are embedded in the binary:

| Preset | IR | Channels |
| --- | --- | --- |
| Behritone | `Presets/Behritone.wav` | mono |
| iPad Speaker | `Presets/iPad.wav` | mono |
| iPhone Speaker | `Presets/iPhone.wav` | mono |

There are **no car, bus, club or concert-hall IRs in this repo**. Roomcheck does not
invent them — use **Import your own IR (.wav)** to add those. Imported files are copied
to `~/Documents/Roomcheck/IRs` and scanned on startup. Filenames
containing `car`, `bus`, `club`, `hall`, `room`, etc. are grouped under **SPACES**.
The listener dot in the position sketch follows the **Distance** knob.

## Custom names and categories for imports

Drop an optional `manifest.json` next to your imported WAVs in
`~/Documents/Roomcheck/IRs`. Only `file` is required; every
other field overrides the filename-based guess:

```json
[
  {
    "file": "my_basement.wav",
    "name": "Basement",
    "subtitle": "Concrete room",
    "category": "spaces",
    "diagram": "car",
    "tag": "0.42 s"
  }
]
```

`category` is `speakers` or `spaces`; `diagram` is `phone`, `car` or `room`.
Files not listed in the manifest are still detected and categorised by filename.

## Install from source

```bash
cargo xtask bundle roomcheck --release
cp -R target/bundled/Roomcheck.vst3 ~/Library/Audio/Plug-Ins/VST3/
cp -R target/bundled/Roomcheck.clap ~/Library/Audio/Plug-Ins/CLAP/
codesign --force --deep --sign - ~/Library/Audio/Plug-Ins/VST3/Roomcheck.vst3
codesign --force --deep --sign - ~/Library/Audio/Plug-Ins/CLAP/Roomcheck.clap
```

Restart your DAW and rescan plugins. Logic Pro needs Audio Unit; this build does not
include AU. Use a CLAP-to-AU wrapper (`clap-wrapper`) or a framework with AU support
if you need Logic specifically.

## Using it

- Insert on the master bus, then pick a reference in the left sidebar or with the
  ◀ / ▶ buttons in the header.
- Hold **Space**, or hold the **Original** button, to A/B the unprocessed signal.
- **Intensity** blends the IR toward a direct impulse; **Distance** adds air
  absorption (cm for speakers, % for spaces); **Tail** re-decays it; **Width** is
  mid/side stereo width; **Mix** is dry/wet; **Trim** is output gain.
- **Match loudness** applies the computed per-IR compensation so switching
  references does not change perceived level. **Mono sum** collapses the input.
- Double-click a knob to reset it; hold Shift while dragging for fine control.
- The plots are real analysis of the current IR: 1/6-octave frequency response,
  Schroeder decay and RT60, channel count and IR length. The LUFS-M meters and the
  spectrum bars are live.

## Real-time design

- 128-sample framing gives a fixed, reported latency and click-free 30 ms
  equal-power crossfades between IRs.
- A background thread decodes, resamples (with padding for very short IRs),
  shapes, analyses and builds the FFT engines. Results reach the audio thread
  through `rtrb` and `triple_buffer` — no locks, file I/O or allocation in
  `process()`.
- Parameter changes and preset selection are host-automatable; the worker polls
  them and coalesces rapid changes.

## Development

```bash
cargo check
cargo test                 # DSP, analysis, IR decoding and offscreen GUI render
cargo xtask bundle roomcheck --release
```

The GUI render test writes a snapshot to `/tmp/roomcheck-editor.png` so layout
changes can be inspected without opening a DAW.
