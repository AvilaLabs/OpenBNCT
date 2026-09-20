// SPDX-License-Identifier: Apache-2.0

//! Browser entry point. The app code is target-agnostic — this module
//! only hosts it on a canvas. Filesystems and child processes do not
//! exist here; artifacts arrive as dropped bytes (`io` module).

#![cfg(target_arch = "wasm32")]

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::OpenBnctApp;

const CANVAS_ID: &str = "openbnct_canvas";

/// Trigger a browser download for in-memory bytes — the web counterpart
/// of a save dialog (screenshots, exports).
pub(crate) fn download_bytes(name: &str, bytes: &[u8]) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Some(document) = window.document() else {
        return;
    };
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array);
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence(&parts) else {
        return;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    let Ok(anchor) = document.create_element("a") else {
        return;
    };
    let _ = anchor.set_attribute("href", &url);
    let _ = anchor.set_attribute("download", name);
    if let Ok(element) = anchor.dyn_into::<web_sys::HtmlElement>() {
        element.click();
    }
    let _ = web_sys::Url::revoke_object_url(&url);
}

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

    let message = "This browser did not allow a WebGL/WebGPU context.<br><br>\
                   <b>Firefox</b>: should work by default — check that hardware \
                   acceleration is enabled.<br>\
                   <b>LibreWolf</b>: disables WebGL by default. Reload, then allow \
                   WebGL from the address-bar permission icon, or set \
                   <code>webgl.disabled = false</code> in <code>about:config</code>.<br>\
                   <b>Other browsers</b>: enable WebGL / hardware acceleration in settings.";
    show_boot_error(message);
    Err(JsValue::from_str(
        "no usable GPU backend (WebGPU and WebGL both unavailable)",
    ))
}
