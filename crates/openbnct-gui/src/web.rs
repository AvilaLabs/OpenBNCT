// SPDX-License-Identifier: Apache-2.0

//! Browser entry point. The app code is target-agnostic — this module
//! only hosts it on a canvas. Filesystems and child processes do not
//! exist here; artifacts arrive as dropped bytes (`io` module).

#![cfg(target_arch = "wasm32")]

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::OpenBnctApp;

const CANVAS_ID: &str = "openbnct_canvas";

fn show_boot_error(message: &str) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    if let Some(veil) = document.get_element_by_id("openbnct-loading") {
        veil.set_inner_html(&format!("openbnct failed to start:<br><br>{message}"));
    }
}

#[wasm_bindgen(start)]
pub async fn start_web() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| JsValue::from_str("no window/document"))?;
    let canvas = document
        .get_element_by_id(CANVAS_ID)
        .and_then(|element| element.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .ok_or_else(|| JsValue::from_str("canvas element not found"))?;

    // Prefer wgpu (WebGPU where supported, WebGL2 as its fallback backend).
    // Browsers that hard-block WebGPU adapters — Firefox-family with
    // resistFingerprinting, older releases — retry on the direct WebGL
    // renderer before giving up.
    for renderer in [eframe::Renderer::Wgpu, eframe::Renderer::Glow] {
        let mut options = eframe::WebOptions::default();
        options.renderer = renderer;
        match eframe::WebRunner::new()
            .start(
                canvas.clone(),
                options,
                Box::new(|creation_context| {
                    crate::configure_style(&creation_context.egui_ctx);
                    Ok(Box::new(OpenBnctApp::new(None, &creation_context.egui_ctx)))
                }),
            )
            .await
        {
            Ok(_) => return Ok(()),
            Err(error) => {
                web_sys::console::warn_1(
                    &format!("renderer {renderer:?} failed to start: {error:?}").into(),
                );
            }
        }
    }

    let message = "no usable GPU backend (WebGPU and WebGL both unavailable — \
                   check that hardware acceleration and WebGL are enabled)";
    show_boot_error(message);
    Err(JsValue::from_str(message))
}
