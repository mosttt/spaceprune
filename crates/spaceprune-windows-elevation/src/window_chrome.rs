//! Optional native window colors; unsupported Windows versions keep system decorations.
use windows_sys::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{
        DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DwmSetWindowAttribute,
    },
};

/// Apply application colors to an existing Windows 11 title bar without replacing system controls.
/// Older systems ignore unsupported attributes and retain their native light/dark theme.
pub fn set_window_chrome_colors(
    window: isize,
    background: [u8; 3],
    foreground: [u8; 3],
    border: [u8; 3],
) {
    for (attribute, [red, green, blue]) in [
        (DWMWA_CAPTION_COLOR, background),
        (DWMWA_TEXT_COLOR, foreground),
        (DWMWA_BORDER_COLOR, border),
    ] {
        let color = u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16);
        // SAFETY: DWM validates the opaque HWND; the COLORREF buffer remains alive for this call.
        unsafe {
            DwmSetWindowAttribute(
                window as HWND,
                attribute as u32,
                (&color as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            );
        }
    }
}
