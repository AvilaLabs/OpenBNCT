// SPDX-License-Identifier: MIT

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let initial_case = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    openbnct_gui::run_native(initial_case)
}

// The web build enters through the cdylib's `start_web`; this target is
// never launched as a binary on wasm.
#[cfg(target_arch = "wasm32")]
fn main() {}
