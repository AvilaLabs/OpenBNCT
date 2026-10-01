# Install and choose a workflow

## Browser workbench

Open [openbnct.avilalabs.org](https://openbnct.avilalabs.org). Load the bundled example or drop a supported dose bundle, plan, NIfTI volume or uncertainty budget into the page. Files are processed locally in the browser.

The web build supports viewing and supported in-browser analysis. Case folders and external process execution require desktop or CLI. Use a browser with WebGL2 or WebGPU enabled. The interface supports English, Japanese, Italian, Chinese and Spanish.

## Published packages

```bash
cargo install openbnct-cli
python -m pip install openbnct
```

The CLI executable is `openbnct`. Desktop downloads are on [GitHub Releases](https://github.com/AvilaLabs/OpenBNCT/releases/latest). Check the downloaded release's version and notes; current-source features can be newer than packaged releases.

## Build current source

Install [Rust](https://rustup.rs/), then clone the repository. The toolchain is pinned to Rust 1.95.

```bash
git clone https://github.com/AvilaLabs/OpenBNCT.git
cd OpenBNCT
cargo build --release -p openbnct-cli
```

Use `target/release/openbnct` in the handbook's commands, or add `target/release` to your executable path. Launch the desktop with `cargo run --release --bin openbnct-gui`.

For current Python bindings, use an activated virtual environment:

```bash
python -m pip install maturin
maturin develop --release --manifest-path bindings/python/Cargo.toml
```

For a local browser build, install Trunk 0.21.14 and the `wasm32-unknown-unknown` target, then run `trunk serve` in `crates/openbnct-gui`.

## Optional external tools

The deterministic solver needs no OpenMC installation. Independent `project verify` comparisons require OpenMC 0.16.0 and the declared processed nuclear-data library, including thermal-scattering tables. NJOY, MCNP and PHITS workflows have separate installation and data requirements. Follow their applicable licenses and the [usage reference](https://github.com/AvilaLabs/OpenBNCT/blob/main/docs/USAGE.md).

Keep private data and generated studies in ignored `cases/`, `runs/` or `outputs/` directories.
