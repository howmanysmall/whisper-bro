use std::ffi::c_void;

use anyhow::{Result, anyhow, ensure};
use windows::Win32::{
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
        DeleteObject, HBITMAP,
    },
    UI::WindowsAndMessaging::{
        CreateIconIndirect, DestroyIcon, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON,
    },
};

use crate::platform::Status;

const STATUSES: [Status; 4] = [
    Status::Idle,
    Status::Recording,
    Status::Processing,
    Status::Error,
];

/// Tray icons indexed by `Status as usize`.
pub fn status_icons() -> Result<[HICON; 4]> {
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let mut icons = Vec::with_capacity(STATUSES.len());
    for status in STATUSES {
        match icon(size, status) {
            Ok(icon) => icons.push(icon),
            Err(error) => {
                for icon in icons {
                    unsafe {
                        let _ = DestroyIcon(icon);
                    }
                }
                return Err(error);
            }
        }
    }
    icons
        .try_into()
        .map_err(|_| anyhow!("Could not create the tray icons"))
}

fn icon(size: i32, status: Status) -> Result<HICON> {
    let side = size as usize;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut c_void = std::ptr::null_mut();
    unsafe {
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let result = (|| -> Result<HICON> {
            ensure!(!bits.is_null(), "Could not draw the tray icon");
            let pixels = std::slice::from_raw_parts_mut(bits.cast::<u32>(), side * side);
            paint(pixels, side, status);
            // A zeroed AND mask keeps every pixel; the alpha channel shapes the icon.
            let mask_bits = vec![0u8; side.div_ceil(16) * 2 * side];
            let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
            ensure!(!mask.is_invalid(), "Could not draw the tray icon");
            let icon = CreateIconIndirect(&ICONINFO {
                fIcon: true.into(),
                hbmMask: mask,
                hbmColor: color,
                ..Default::default()
            });
            delete(mask);
            Ok(icon?)
        })();
        delete(color);
        result
    }
}

fn delete(bitmap: HBITMAP) {
    unsafe {
        let _ = DeleteObject(bitmap.into());
    }
}

fn paint(pixels: &mut [u32], side: usize, status: Status) {
    let (red, green, blue) = match status {
        Status::Idle => (0x9E, 0x9E, 0x9E),
        Status::Recording | Status::Error => (0xE5, 0x39, 0x35),
        Status::Processing => (0xFF, 0xB3, 0x00),
    };
    let size = side as f32;
    let center = size / 2.0;
    let radius = center - 1.0;
    for (index, pixel) in pixels.iter_mut().enumerate() {
        let x = (index % side) as f32 + 0.5;
        let y = (index / side) as f32 + 0.5;
        let alpha = coverage((x - center).hypot(y - center) - radius);
        let white = if status == Status::Error {
            mark(x - center, y, size)
        } else {
            0.0
        };
        let channel = |value: f32| ((value * (1.0 - white) + 255.0 * white) * alpha).round() as u32;
        *pixel = (((alpha * 255.0).round() as u32) << 24)
            | (channel(red as f32) << 16)
            | (channel(green as f32) << 8)
            | channel(blue as f32);
    }
}

/// Coverage of the white "!" (a vertical bar and a dot) at an offset from the centre column.
fn mark(dx: f32, y: f32, size: f32) -> f32 {
    let half = size * 0.07;
    let top = size * 0.25;
    let bottom = size * 0.56;
    let bar = dx.hypot(y - y.clamp(top, bottom)) - half;
    let dot = dx.hypot(y - size * 0.73) - half * 1.15;
    coverage(bar.min(dot))
}

/// One-pixel anti-aliased edge for a signed distance.
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}
