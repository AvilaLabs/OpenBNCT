// SPDX-License-Identifier: MIT

use eframe::egui;

// The OpenBNCT mark from the Avila Labs tool set, rendered from its SVG at
// 256 px (crisp at HiDPI sizes). The tile variant is the window icon.
const LOGO_PNG: &[u8] = include_bytes!("../assets/openbnct-mark.png");
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const ICON_PNG: &[u8] = include_bytes!("../assets/openbnct-icon.png");

pub(crate) fn load_logo_texture(context: &egui::Context) -> Result<egui::TextureHandle, String> {
    let image = decode_logo()?;
    Ok(context.load_texture("openbnct-mark", image, egui::TextureOptions::LINEAR))
}

fn decode_logo() -> Result<egui::ColorImage, String> {
    let decoded = image::load_from_memory_with_format(LOGO_PNG, image::ImageFormat::Png)
        .map_err(|error| format!("embedded OpenBNCT mark is not a valid PNG: {error}"))?
        .into_rgba8();
    let size = [decoded.width() as usize, decoded.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        decoded.as_raw(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_logo_asset_is_embedded_at_hidpi_size() {
        let image = decode_logo().expect("official logo should decode");
        assert_eq!(image.size, [256, 256]);
        assert_eq!(image.pixels.len(), 256 * 256);
    }
}
