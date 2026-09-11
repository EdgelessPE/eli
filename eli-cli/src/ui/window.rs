use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GWL_STYLE, GWLP_WNDPROC, GetWindowLongPtrW, GetWindowRect,
    HTCAPTION, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowLongPtrW,
    SetWindowPos, WM_NCDESTROY, WM_NCHITTEST, WNDPROC, WS_POPUP, WS_VISIBLE,
};

pub(crate) fn apply_window_region(window: &slint::Window, corner_radius: f32) -> bool {
    let Some(window_handle) = native_window_handle(window) else {
        return false;
    };
    let size = window.size();
    let corner_diameter = rounded_corner_diameter(corner_radius, window.scale_factor());
    let region = unsafe {
        CreateRoundRectRgn(
            0,
            0,
            size.width as i32,
            size.height as i32,
            corner_diameter,
            corner_diameter,
        )
    };
    if region.is_null() {
        return false;
    }
    let popup_applied =
        apply_popup_window_style(window_handle, size.width as i32, size.height as i32);
    let region_applied = unsafe { SetWindowRgn(window_handle, region, 1) } != 0;
    if !region_applied {
        unsafe {
            DeleteObject(region);
        }
    }
    popup_applied && region_applied
}

pub(crate) fn install_titlebar_hit_test(
    window: &slint::Window,
    titlebar_height: f32,
    left_inset: f32,
    right_inset: f32,
) -> bool {
    let Some(window_handle) = native_window_handle(window) else {
        return false;
    };
    let size = window.size();
    let scale_factor = window.scale_factor();
    install_titlebar_hit_test_for_handle(
        window_handle,
        size.width as i32,
        logical_to_physical(titlebar_height, scale_factor),
        logical_to_physical(left_inset, scale_factor),
        logical_to_physical(right_inset, scale_factor),
    )
}

fn native_window_handle(window: &slint::Window) -> Option<HWND> {
    let window_handle = window.window_handle();
    let raw_window_handle = window_handle.window_handle().ok()?;
    let RawWindowHandle::Win32(window_handle) = raw_window_handle.as_raw() else {
        return None;
    };
    Some(isize::from(window_handle.hwnd) as HWND)
}

fn apply_popup_window_style(window_handle: HWND, width: i32, height: i32) -> bool {
    let current_style = unsafe { GetWindowLongPtrW(window_handle, GWL_STYLE) as u32 };
    let popup_style = popup_window_style(current_style);
    unsafe {
        SetWindowLongPtrW(window_handle, GWL_STYLE, popup_style as isize);
        SetWindowPos(
            window_handle,
            std::ptr::null_mut(),
            0,
            0,
            width,
            height,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
    let applied_style = unsafe { GetWindowLongPtrW(window_handle, GWL_STYLE) as u32 };
    applied_style == popup_style
}

fn popup_window_style(current_style: u32) -> u32 {
    WS_POPUP | (current_style & WS_VISIBLE)
}

fn rounded_corner_diameter(corner_radius: f32, scale_factor: f32) -> i32 {
    (corner_radius * 2.0 * scale_factor).round() as i32
}

#[derive(Clone, Copy)]
struct TitlebarHitTest {
    original_procedure: WNDPROC,
    width: i32,
    titlebar_height: i32,
    left_inset: i32,
    right_inset: i32,
}

fn titlebar_hit_tests() -> &'static Mutex<HashMap<isize, TitlebarHitTest>> {
    static TITLEBAR_HIT_TESTS: OnceLock<Mutex<HashMap<isize, TitlebarHitTest>>> = OnceLock::new();
    TITLEBAR_HIT_TESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn install_titlebar_hit_test_for_handle(
    window_handle: HWND,
    width: i32,
    titlebar_height: i32,
    left_inset: i32,
    right_inset: i32,
) -> bool {
    let key = window_handle as isize;
    let Ok(mut hit_tests) = titlebar_hit_tests().lock() else {
        return false;
    };
    if hit_tests.contains_key(&key) {
        return true;
    }
    let original_procedure = unsafe {
        SetWindowLongPtrW(
            window_handle,
            GWLP_WNDPROC,
            titlebar_window_procedure as *const () as isize,
        )
    };
    if original_procedure == 0 {
        return false;
    }
    let original_procedure = unsafe { std::mem::transmute::<isize, WNDPROC>(original_procedure) };
    if original_procedure.is_none() {
        return false;
    }
    hit_tests.insert(
        key,
        TitlebarHitTest {
            original_procedure,
            width,
            titlebar_height,
            left_inset,
            right_inset,
        },
    );
    true
}

unsafe extern "system" fn titlebar_window_procedure(
    window_handle: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let key = window_handle as isize;
    let hit_test = titlebar_hit_tests()
        .lock()
        .ok()
        .and_then(|hit_tests| hit_tests.get(&key).copied());
    if message == WM_NCHITTEST
        && hit_test.is_some_and(|hit_test| is_titlebar_hit(window_handle, lparam, hit_test))
    {
        return HTCAPTION as LRESULT;
    }
    let result = if let Some(hit_test) = hit_test {
        unsafe {
            CallWindowProcW(
                hit_test.original_procedure,
                window_handle,
                message,
                wparam,
                lparam,
            )
        }
    } else {
        unsafe { DefWindowProcW(window_handle, message, wparam, lparam) }
    };
    if message == WM_NCDESTROY
        && let Ok(mut hit_tests) = titlebar_hit_tests().lock()
    {
        hit_tests.remove(&key);
    }
    result
}

fn is_titlebar_hit(window_handle: HWND, lparam: LPARAM, hit_test: TitlebarHitTest) -> bool {
    let mut window_rect: RECT = unsafe { std::mem::zeroed() };
    if unsafe { GetWindowRect(window_handle, &mut window_rect) } == 0 {
        return false;
    }
    is_titlebar_hit_in_rect(window_rect, lparam, hit_test)
}

fn is_titlebar_hit_in_rect(window_rect: RECT, lparam: LPARAM, hit_test: TitlebarHitTest) -> bool {
    let screen_x = lparam as i16 as i32;
    let screen_y = (lparam >> 16) as i16 as i32;
    screen_x >= window_rect.left + hit_test.left_inset
        && screen_x < window_rect.left + hit_test.width - hit_test.right_inset
        && screen_y >= window_rect.top
        && screen_y < window_rect.top + hit_test.titlebar_height
}

fn logical_to_physical(value: f32, scale_factor: f32) -> i32 {
    (value * scale_factor).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_corner_region_scales_with_the_display() {
        assert_eq!(rounded_corner_diameter(12.0, 1.0), 24);
        assert_eq!(rounded_corner_diameter(12.0, 1.25), 30);
    }

    #[test]
    fn popup_window_style_only_keeps_visibility() {
        assert_eq!(popup_window_style(0), WS_POPUP);
        assert_eq!(popup_window_style(WS_VISIBLE), WS_POPUP | WS_VISIBLE);
    }

    #[test]
    fn titlebar_hit_test_excludes_content_and_close_button() {
        let hit_test = TitlebarHitTest {
            original_procedure: None,
            width: 420,
            titlebar_height: 36,
            left_inset: 8,
            right_inset: 40,
        };
        let rect = RECT {
            left: 100,
            top: 200,
            right: 520,
            bottom: 448,
        };

        assert!(is_titlebar_hit_in_rect(
            rect,
            screen_position_lparam(160, 235),
            hit_test
        ));
        assert!(!is_titlebar_hit_in_rect(
            rect,
            screen_position_lparam(160, 236),
            hit_test
        ));
        assert!(!is_titlebar_hit_in_rect(
            rect,
            screen_position_lparam(490, 220),
            hit_test
        ));
    }

    fn screen_position_lparam(x: i32, y: i32) -> LPARAM {
        ((y as u16 as isize) << 16) | x as u16 as isize
    }
}
