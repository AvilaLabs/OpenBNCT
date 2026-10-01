// SPDX-License-Identifier: MIT

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    match openbnct_gui::parse_launch_args(std::env::args_os().skip(1)) {
        Ok(options) => openbnct_gui::run_native_with(options),
        Err(message) => {
            eprintln!("openbnct-gui: {message}");
            eprintln!(
                "usage: openbnct-gui [CASE] [--workspace W] [--project-dir DIR] [--load-dose]\n\
                 \x20      [--prefill-volume DIR] [--scroll-results] [--advanced] [--openbnct PATH]\n\
                 \x20      [--screenshot OUT.png] [--wait-seconds N]"
            );
            std::process::exit(2);
        }
    }
}

// The web build enters through the cdylib's `start_web`; this target is
// never launched as a binary on wasm.
#[cfg(target_arch = "wasm32")]
fn main() {}
