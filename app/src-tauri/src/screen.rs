//! Screenshots of the monitor that holds the foreground window (the game).

use anyhow::{anyhow, Result};
use base64::Engine;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, RgbImage};
use xcap::Monitor;

#[derive(Clone)]
pub struct Shot {
    pub jpeg: Vec<u8>,
    pub thumbnail: String,
    pub width: u32,
    pub height: u32,
}

fn foreground_rect() -> Option<(i32, i32, i32, i32)> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
        let window = GetForegroundWindow();
        if window.0.is_null() {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(window, &mut rect).ok()?;
        return Some((rect.left, rect.top, rect.right, rect.bottom));
    }
    #[allow(unreachable_code)]
    None
}

fn pick_monitor() -> Result<Monitor> {
    let monitors = Monitor::all().map_err(|error| anyhow!("écrans introuvables: {error}"))?;
    if monitors.is_empty() {
        return Err(anyhow!("aucun écran détecté"));
    }
    let Some((left, top, right, bottom)) = foreground_rect() else {
        return monitors.into_iter().find(|monitor| monitor.is_primary().unwrap_or(false)).ok_or_else(|| anyhow!("aucun écran principal"));
    };
    let overlap = |monitor: &Monitor| -> i64 {
        let (Ok(x), Ok(y), Ok(width), Ok(height)) = (monitor.x(), monitor.y(), monitor.width(), monitor.height()) else {
            return 0;
        };
        let width = (right.min(x + width as i32) - left.max(x)).max(0) as i64;
        let height = (bottom.min(y + height as i32) - top.max(y)).max(0) as i64;
        width * height
    };
    monitors.into_iter().max_by_key(overlap).ok_or_else(|| anyhow!("aucun écran"))
}

fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    JpegEncoder::new_with_quality(&mut buffer, quality).encode_image(image)?;
    Ok(buffer)
}

pub fn capture(max_width: u32) -> Result<Shot> {
    let monitor = pick_monitor()?;
    let captured = monitor.capture_image().map_err(|error| anyhow!("capture impossible: {error}"))?;
    let mut image = DynamicImage::ImageRgba8(captured);
    if image.width() > max_width {
        let height = (u64::from(image.height()) * u64::from(max_width) / u64::from(image.width())) as u32;
        image = image.resize_exact(max_width, height.max(1), FilterType::Triangle);
    }
    let rgb = image.to_rgb8();
    let jpeg = encode_jpeg(&rgb, 80)?;
    let thumbnail_image = image.thumbnail(480, 480).to_rgb8();
    let thumbnail = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(encode_jpeg(&thumbnail_image, 70)?)
    );
    Ok(Shot { jpeg, thumbnail, width: rgb.width(), height: rgb.height() })
}

pub fn is_mostly_black(shot: &Shot) -> bool {
    image::load_from_memory(&shot.jpeg)
        .map(|image| image.to_luma8().pixels().all(|pixel| pixel.0[0] < 16))
        .unwrap_or(false)
}
