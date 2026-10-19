use std::ffi::c_void;

use anyhow::{Result, ensure};
use windows::{
    Win32::{Foundation::*, Graphics::Gdi::*, UI::WindowsAndMessaging::*},
    core::w,
};

use super::Shared;

const CLASS: windows::core::PCWSTR = w!("YapOverlay");
const MAX_WIDTH: i32 = 420;
const PADDING: i32 = 12;
const MARGIN: i32 = 16;
const POINT_SIZE: i32 = 11;
const ALPHA: u8 = 235;
/// COLORREF is 0x00BBGGRR.
const BACKGROUND: COLORREF = COLORREF(0x0020_2020);
const FOREGROUND: COLORREF = COLORREF(0x00FF_FFFF);
/// Per-window extra bytes holding the font and brush for the paint handler.
const FONT_SLOT: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(0);
const BRUSH_SLOT: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX(size_of::<isize>() as i32);
const TEXT_FORMAT: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_WORDBREAK.0 | DT_NOPREFIX.0);

/// A small dark panel at the bottom-right of the work area that shows the
/// recording status and the live preview. It never takes focus and lets
/// clicks through, so the app being dictated into keeps the keyboard.
pub struct Overlay {
    window: HWND,
    font: HFONT,
    brush: HBRUSH,
    instance: HINSTANCE,
}

impl Overlay {
    /// The paint handler reads `shared.overlay`, so `shared` must outlive
    /// the overlay at a fixed address.
    pub fn new(instance: HINSTANCE, shared: &Shared) -> Result<Self> {
        unsafe {
            let class = WNDCLASSW {
                lpfnWndProc: Some(overlay_proc),
                hInstance: instance,
                lpszClassName: CLASS,
                cbWndExtra: 2 * size_of::<isize>() as i32,
                ..Default::default()
            };
            ensure!(
                RegisterClassW(&class) != 0,
                "Could not register the preview overlay"
            );
            let screen = GetDC(None);
            let dpi = GetDeviceCaps(Some(screen), LOGPIXELSY);
            ReleaseDC(None, screen);
            let font = CreateFontW(
                -(POINT_SIZE * dpi / 72),
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            );
            let brush = CreateSolidBrush(BACKGROUND);
            let mut overlay = Self {
                window: HWND::default(),
                font,
                brush,
                instance,
            };
            overlay.window = CreateWindowExW(
                WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_LAYERED
                    | WS_EX_TRANSPARENT,
                CLASS,
                w!("Yap"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(instance),
                None,
            )?;
            let window = overlay.window;
            SetLayeredWindowAttributes(window, COLORREF(0), ALPHA, LWA_ALPHA)?;
            SetWindowLongPtrW(window, FONT_SLOT, font.0 as isize);
            SetWindowLongPtrW(window, BRUSH_SLOT, brush.0 as isize);
            SetWindowLongPtrW(window, GWLP_USERDATA, (shared as *const Shared) as isize);
            Ok(overlay)
        }
    }

    /// Shows `text`, sized to fit it up to 420 px wide, and repaints.
    pub fn show(&self, shared: &Shared, text: &str) {
        text.clone_into(&mut shared.overlay.lock().unwrap_or_else(|e| e.into_inner()));
        let mut units: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let mut bounds = RECT {
                right: MAX_WIDTH - 2 * PADDING,
                ..Default::default()
            };
            let context = GetDC(Some(self.window));
            let previous = SelectObject(context, self.font.into());
            DrawTextW(context, &mut units, &mut bounds, TEXT_FORMAT | DT_CALCRECT);
            SelectObject(context, previous);
            ReleaseDC(Some(self.window), context);
            let width = bounds.right + 2 * PADDING;
            let height = bounds.bottom + 2 * PADDING;
            let mut area = RECT::default();
            if SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut area as *mut RECT as *mut c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .is_err()
            {
                area = RECT {
                    right: GetSystemMetrics(SM_CXSCREEN),
                    bottom: GetSystemMetrics(SM_CYSCREEN),
                    ..Default::default()
                };
            }
            let x = (area.right - MARGIN - width).max(area.left);
            let y = (area.bottom - MARGIN - height).max(area.top + MARGIN);
            let _ = SetWindowPos(
                self.window,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE,
            );
            let _ = ShowWindow(self.window, SW_SHOWNOACTIVATE);
            let _ = InvalidateRect(Some(self.window), None, false);
        }
    }

    pub fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.window, SW_HIDE);
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.window, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.window);
            let _ = UnregisterClassW(CLASS, Some(self.instance));
            let _ = DeleteObject(self.font.into());
            let _ = DeleteObject(self.brush.into());
        }
    }
}

unsafe extern "system" fn overlay_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        // WM_PAINT fills the whole window, so skipping the erase avoids flicker.
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            unsafe { paint(window) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// # Safety
///
/// `window` must be an overlay window whose extra slots and user data
/// [`Overlay::new`] filled in.
unsafe fn paint(window: HWND) {
    unsafe {
        let mut paint = PAINTSTRUCT::default();
        let context = BeginPaint(window, &mut paint);
        let font = HFONT(GetWindowLongPtrW(window, FONT_SLOT) as *mut c_void);
        let brush = HBRUSH(GetWindowLongPtrW(window, BRUSH_SLOT) as *mut c_void);
        let shared = GetWindowLongPtrW(window, GWLP_USERDATA) as *const Shared;
        let mut client = RECT::default();
        if GetClientRect(window, &mut client).is_ok() {
            FillRect(context, &client, brush);
            if !shared.is_null() {
                let mut units: Vec<u16> = (*shared)
                    .overlay
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .encode_utf16()
                    .collect();
                SetBkMode(context, TRANSPARENT);
                SetTextColor(context, FOREGROUND);
                let previous = SelectObject(context, font.into());
                let mut bounds = RECT {
                    left: client.left + PADDING,
                    top: client.top + PADDING,
                    right: client.right - PADDING,
                    bottom: client.bottom - PADDING,
                };
                DrawTextW(context, &mut units, &mut bounds, TEXT_FORMAT);
                SelectObject(context, previous);
            }
        }
        let _ = EndPaint(window, &paint);
    }
}
