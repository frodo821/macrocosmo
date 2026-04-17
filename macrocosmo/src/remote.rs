//! #390-T4: BRP screenshot command for vision-based testing.
//!
//! Provides a `macrocosmo/screenshot` JSON-RPC method via the Bevy Remote Protocol.
//! The method captures the primary window as a base64-encoded PNG.
//!
//! Because screenshot capture is asynchronous (the GPU read-back completes on the
//! next frame), the handler uses a two-phase approach:
//!
//! 1. On the first call (buffer empty) it spawns a `Screenshot` entity and returns
//!    an error asking the client to retry.
//! 2. An entity observer on `ScreenshotCaptured` encodes the image as PNG,
//!    base64-encodes it, and stores it in a `ScreenshotBuffer` resource.
//! 3. On retry the handler drains the buffer and returns the screenshot data.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, error_codes};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use serde_json::{Value, json};
use std::io::Cursor;

/// The method path for the screenshot BRP command.
pub const MACROCOSMO_SCREENSHOT_METHOD: &str = "macrocosmo/screenshot";

/// Holds the latest captured screenshot as a base64-encoded PNG string plus
/// dimensions.  Written by the entity observer, consumed by the BRP handler.
#[derive(Resource, Default)]
pub struct ScreenshotBuffer {
    pub data: Option<ScreenshotData>,
}

/// Payload returned by the `macrocosmo/screenshot` method.
pub struct ScreenshotData {
    pub base64: String,
    pub width: u32,
    pub height: u32,
}

/// BRP handler for `macrocosmo/screenshot`.
///
/// Returns `{ "base64": "...", "width": u32, "height": u32 }` when a screenshot
/// is available.  On the first call (nothing buffered) it requests a capture and
/// returns an error telling the client to retry after one frame.
pub fn screenshot_handler(In(_params): In<Option<Value>>, world: &mut World) -> BrpResult {
    // Try to consume a previously-captured screenshot.
    let has_data = world
        .get_resource::<ScreenshotBuffer>()
        .and_then(|buf| buf.data.as_ref())
        .is_some();

    if has_data {
        let data = world
            .resource_mut::<ScreenshotBuffer>()
            .data
            .take()
            .unwrap();
        return Ok(json!({
            "base64": data.base64,
            "width": data.width,
            "height": data.height,
        }));
    }

    // No screenshot buffered yet — spawn a capture request.
    // The entity observer will encode the result into ScreenshotBuffer.
    world
        .spawn(Screenshot::primary_window())
        .observe(on_screenshot_captured);

    Err(BrpError {
        code: error_codes::INTERNAL_ERROR,
        message: "Screenshot requested — retry after one frame".into(),
        data: None,
    })
}

/// Entity observer callback: encodes the captured image as PNG → base64 and
/// stores it in [`ScreenshotBuffer`].
fn on_screenshot_captured(trigger: On<ScreenshotCaptured>, mut buffer: ResMut<ScreenshotBuffer>) {
    let captured = &*trigger;
    let image = &captured.image;

    let width = image.width();
    let height = image.height();

    // Convert Bevy Image → DynamicImage → RGB8 → PNG bytes → base64.
    let dyn_img = match image.clone().try_into_dynamic() {
        Ok(img) => img,
        Err(e) => {
            error!("Screenshot: failed to convert to DynamicImage: {e:?}");
            return;
        }
    };

    let rgb = dyn_img.to_rgb8();
    let mut png_bytes = Cursor::new(Vec::new());
    if let Err(e) = rgb.write_to(&mut png_bytes, image::ImageFormat::Png) {
        error!("Screenshot: failed to encode PNG: {e}");
        return;
    }

    let encoded = BASE64.encode(png_bytes.into_inner());

    buffer.data = Some(ScreenshotData {
        base64: encoded,
        width,
        height,
    });

    info!("Screenshot captured and buffered: {width}x{height}");
}
