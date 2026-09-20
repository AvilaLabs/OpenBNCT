// SPDX-License-Identifier: Apache-2.0

//! Browser entry point. The app code is target-agnostic — this module
//! only hosts it on a canvas. Filesystems and child processes do not
//! exist here; artifacts arrive as dropped bytes (`io` module).

#![cfg(target_arch = "wasm32")]

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::OpenBnctApp;

const CANVAS_ID: &str = "openbnct_canvas";

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

    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|creation_context| {
                crate::configure_style(&creation_context.egui_ctx);
                Ok(Box::new(OpenBnctApp::new(None, &creation_context.egui_ctx)))
            }),
        )
        .await
        .map(|_| ())
        .map_err(|error| JsValue::from_str(&format!("eframe start failed: {error:?}")))
}
