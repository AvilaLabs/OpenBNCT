# openbnct-gui

egui desktop workbench for
[OpenBNCT](https://github.com/AvilaLabs/OpenBNCT): workspace navigation, dose
overlay on registered grids, positioning panels, and artifact inspection built
on the shared Rust contracts.

Research software.

## Desktop workflow

The workbench opens in light mode; View → Dark mode switches the full palette.
The case toolbar verifies input before images appear. Overview links to the
geometry, transport, plan, dose, and evidence workspaces. File owns template
export; Help and F1 open the existing guided tours.

Geometry presents linked axial, coronal, and sagittal images alongside a
scrollable inspector. Images preserve their physical aspect ratio, and pointer
selection and crosshairs use the painted image bounds. Each workspace keeps
its own scroll position. Plan files can be selected with Browse.

Run with a generated synthetic case:

```sh
cargo run --bin openbnct-gui -- /tmp/nf-bnct-001
```

For visual review, debug builds can save their own rendered window and close:

```sh
OPENBNCT_CAPTURE=/tmp/openbnct.png cargo run --bin openbnct-gui -- /tmp/nf-bnct-001
```

With capture enabled, `OPENBNCT_CAPTURE_WORKSPACE=01` through `06` select the
six workspaces and `OPENBNCT_CAPTURE_DARK=1` selects dark mode. These review
settings do not affect normal launches or release builds.
