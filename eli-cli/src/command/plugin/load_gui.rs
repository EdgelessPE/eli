use eli_lib::Ctx;
use eli_lib::command::plugin::{
    LoadOptions, LoadProgress, LoadStatus, LoadSummary, LocalBoostHandling,
};
use eli_lib::dependency::RuntimeEnvironment;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GWL_STYLE, GWLP_WNDPROC, GetWindowLongPtrW, GetWindowRect,
    HTCAPTION, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, SetWindowLongPtrW,
    SetWindowPos, WM_NCDESTROY, WM_NCHITTEST, WNDPROC, WS_POPUP, WS_VISIBLE,
};

#[allow(deprecated)]
mod ui {
    slint::slint! {
        import { Button, ButtonSize, ButtonVariant } from "ui/slintcn/components/button.slint";
        import { Tooltip } from "ui/slintcn/components/tooltip.slint";
        import { Tokens } from "ui/slintcn/theme/tokens.slint";
        import { Palette, ScrollView } from "std-widgets.slint";

        export struct PluginRow {
            label: string,
            icon-color: color,
            show-status: bool,
            loading: bool,
            detail: string,
        }

        export component PluginLoadWindow inherits Window {
            title: "加载插件";
            width: 420px;
            height: 248px;
            background: root.surface-color;

            in-out property <string> prompt;
            in-out property <[PluginRow]> rows: [];
            in-out property <bool> busy: false;
            in-out property <bool> retry-available: false;
            in-out property <int> spinner-frame: 0;
            in-out property <string> tooltip-text: "";
            in-out property <bool> tooltip-open: false;
            in-out property <length> tooltip-anchor-y: 0px;
            in-out property <bool> window-region-pending: true;
            in-out property <bool> titlebar-hit-test-pending: true;
            property <bool> dark-mode: Palette.color-scheme == ColorScheme.dark;
            property <color> surface-color: root.dark-mode ? #111827 : #ffffff;
            property <color> text-color: root.dark-mode ? #f9fafb : #111827;
            property <color> border-color: root.dark-mode ? #374151 : #d1d5db;
            property <color> close-hover-color: root.dark-mode ? #374151 : #f3f4f6;
            callback load-requested();
            callback localboost-requested();
            callback cancel-requested();
            callback apply-window-region() -> bool;
            callback install-titlebar-hit-test() -> bool;

            Timer {
                interval: 33ms;
                running: root.busy;
                triggered => {
                    root.spinner-frame = Math.mod(root.spinner-frame + 1, 24);
                }
            }
            Timer {
                interval: 16ms;
                running: root.window-region-pending;
                triggered => {
                    root.window-region-pending = !root.apply-window-region();
                }
            }
            Timer {
                interval: 200ms;
                running: root.titlebar-hit-test-pending;
                triggered => {
                    root.titlebar-hit-test-pending = !root.install-titlebar-hit-test();
                }
            }

            Rectangle {
                width: parent.width;
                height: parent.height;
                background: root.surface-color;
                border-color: root.border-color;
                border-width: 1px;
                border-radius: 12px;

                Text {
                    x: 16px;
                    y: 0px;
                    width: parent.width - 64px;
                    height: 36px;
                    text: "加载插件";
                    font-size: 16px;
                    font-weight: 600;
                    vertical-alignment: center;
                    color: root.text-color;
                }
                close-button := Rectangle {
                    x: parent.width - 32px;
                    y: 6px;
                    width: 24px;
                    height: 24px;
                    background: close-area.has-hover ? root.close-hover-color : transparent;
                    border-radius: 6px;

                    Path {
                        x: 5px;
                        y: 5px;
                        width: 14px;
                        height: 14px;
                        viewbox-x: 0;
                        viewbox-y: 0;
                        viewbox-width: 14;
                        viewbox-height: 14;
                        fill: transparent;
                        stroke: root.text-color;
                        stroke-width: 1.5px;
                        stroke-line-cap: round;
                        commands: "M 3 3 L 11 11 M 11 3 L 3 11";
                    }
                    close-area := TouchArea {
                        mouse-cursor: pointer;
                        clicked => { root.cancel-requested(); }
                    }
                }
                Text {
                    x: 16px;
                    y: 36px;
                    width: parent.width - 32px;
                    text: root.prompt;
                    font-size: 14px;
                    color: Tokens.color-muted-foreground;
                }
                scroll := ScrollView {
                    x: 20px;
                    y: 62px;
                    width: parent.width - 40px;
                    height: parent.height - 123px;
                    viewport-height: root.rows.length * 32px;
                    Rectangle {
                        width: parent.width - 28px;
                        height: root.rows.length * 32px;
                        for row[index] in root.rows: Rectangle {
                            y: index * 32px;
                            width: parent.width;
                            height: 28px;
                            Text { x: 0; y: 3px; width: parent.width - 36px; text: row.label; font-size: 14px; overflow: elide; color: root.text-color; }
                            if row.loading: Rectangle {
                                x: parent.width - 20px;
                                y: 5px;
                                width: 18px;
                                height: 18px;
                                if root.spinner-frame == 0: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-0.svg"); }
                                if root.spinner-frame == 1: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-1.svg"); }
                                if root.spinner-frame == 2: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-2.svg"); }
                                if root.spinner-frame == 3: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-3.svg"); }
                                if root.spinner-frame == 4: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-4.svg"); }
                                if root.spinner-frame == 5: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-5.svg"); }
                                if root.spinner-frame == 6: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-6.svg"); }
                                if root.spinner-frame == 7: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-7.svg"); }
                                if root.spinner-frame == 8: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-8.svg"); }
                                if root.spinner-frame == 9: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-9.svg"); }
                                if root.spinner-frame == 10: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-10.svg"); }
                                if root.spinner-frame == 11: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-11.svg"); }
                                if root.spinner-frame == 12: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-12.svg"); }
                                if root.spinner-frame == 13: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-13.svg"); }
                                if root.spinner-frame == 14: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-14.svg"); }
                                if root.spinner-frame == 15: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-15.svg"); }
                                if root.spinner-frame == 16: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-16.svg"); }
                                if root.spinner-frame == 17: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-17.svg"); }
                                if root.spinner-frame == 18: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-18.svg"); }
                                if root.spinner-frame == 19: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-19.svg"); }
                                if root.spinner-frame == 20: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-20.svg"); }
                                if root.spinner-frame == 21: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-21.svg"); }
                                if root.spinner-frame == 22: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-22.svg"); }
                                if root.spinner-frame == 23: Image { width: parent.width; height: parent.height; source: @image-url("ui/arco-spin-23.svg"); }
                            }
                            if row.show-status && !row.loading: Rectangle { x: parent.width - 16px; y: 6px; width: 12px; height: 12px; border-radius: 6px; background: row.icon-color; }
                            hover := TouchArea {
                                x: parent.width - 64px;
                                width: 64px;
                                height: parent.height;
                                changed has-hover => {
                                    if (self.has-hover && row.detail != "") {
                                        root.tooltip-text = row.detail;
                                        root.tooltip-anchor-y = scroll.y + parent.y + scroll.viewport-y;
                                        root.tooltip-open = true;
                                    } else if (!self.has-hover) {
                                        root.tooltip-open = false;
                                    }
                                }
                            }
                        }
                    }
                }
                HorizontalLayout {
                    x: 20px;
                    y: parent.height - 48px;
                    width: parent.width - 40px;
                    height: 36px;
                    spacing: 8px;
                    alignment: end;
                    Button {
                        text: "取消";
                        variant: ButtonVariant.ghost;
                        size: ButtonSize.default;
                        disabled: root.busy;
                        clicked => { root.cancel-requested(); }
                    }
                    Button {
                        text: "LocalBoost 加载";
                        variant: ButtonVariant.outline;
                        size: ButtonSize.default;
                        disabled: root.busy;
                        clicked => { root.localboost-requested(); }
                    }
                    Button {
                        text: root.retry-available ? "重试" : "加载";
                        variant: ButtonVariant.default;
                        size: ButtonSize.default;
                        disabled: root.busy;
                        clicked => { root.load-requested(); }
                    }
                }
                Tooltip {
                    x: 20px;
                    width: parent.width - 40px;
                    text: root.tooltip-text;
                    open: root.tooltip-open;
                    anchor-y: root.tooltip-anchor-y;
                    max-bottom: parent.height - 60px;
                }
            }
        }
    }
}

use ui::{PluginLoadWindow, PluginRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowState {
    Waiting,
    Loading,
    Succeeded,
    Failed,
}

struct Row {
    path: PathBuf,
    state: RowState,
    detail: String,
}

struct GuiState {
    rows: Vec<Row>,
    failed_paths: Vec<PathBuf>,
    retry_uses_localboost: bool,
}

impl GuiState {
    fn new(inputs: Vec<PathBuf>) -> Self {
        Self {
            rows: inputs
                .into_iter()
                .map(|path| Row {
                    path,
                    state: RowState::Waiting,
                    detail: String::new(),
                })
                .collect(),
            failed_paths: Vec::new(),
            retry_uses_localboost: false,
        }
    }

    fn begin(&mut self, inputs: Vec<PathBuf>, localboost: bool) {
        self.rows = inputs
            .into_iter()
            .map(|path| Row {
                path,
                state: RowState::Waiting,
                detail: String::new(),
            })
            .collect();
        self.failed_paths.clear();
        self.retry_uses_localboost = localboost;
    }

    fn apply_progress(&mut self, progress: LoadProgress) {
        let (path, state, detail) = match progress {
            LoadProgress::Started { path } => (path, RowState::Loading, String::new()),
            LoadProgress::Finished { path, result } => match result {
                Ok(_) => (path, RowState::Succeeded, String::new()),
                Err(error) => (path, RowState::Failed, error),
            },
        };
        if let Some(row) = self.rows.iter_mut().find(|row| row.path == path) {
            row.state = state;
            row.detail = detail;
        }
    }

    fn finish(&mut self, summary: LoadSummary) -> bool {
        self.rows = summary
            .results
            .into_iter()
            .map(|result| {
                let (state, detail) = match result.result {
                    Ok(LoadStatus::Loaded | LoadStatus::LoadedWithLocalBoost)
                    | Ok(LoadStatus::SkippedLocalBoost) => (RowState::Succeeded, String::new()),
                    Err(error) => (RowState::Failed, error.to_string()),
                };
                Row {
                    path: result.path,
                    state,
                    detail,
                }
            })
            .collect();
        self.failed_paths = self
            .rows
            .iter()
            .filter(|row| matches!(row.state, RowState::Failed))
            .map(|row| row.path.clone())
            .collect();
        self.failed_paths.is_empty()
    }

    fn fail_to_start(&mut self, inputs: Vec<PathBuf>, error: io::Error) {
        let detail = error.to_string();
        self.rows = inputs
            .iter()
            .cloned()
            .map(|path| Row {
                path,
                state: RowState::Failed,
                detail: detail.clone(),
            })
            .collect();
        self.failed_paths = inputs;
    }

    fn rows(&self) -> Vec<PluginRow> {
        self.rows
            .iter()
            .map(|row| {
                let (icon_color, show_status, loading) = match row.state {
                    RowState::Waiting => (Color::from_rgb_u8(17, 24, 39), false, false),
                    RowState::Loading => (Color::from_rgb_u8(37, 99, 235), false, true),
                    RowState::Succeeded => (Color::from_rgb_u8(22, 163, 74), true, false),
                    RowState::Failed => (Color::from_rgb_u8(220, 38, 38), true, false),
                };
                PluginRow {
                    label: file_label(&row.path).into(),
                    icon_color,
                    show_status,
                    loading,
                    detail: row.detail.clone().into(),
                }
            })
            .collect()
    }
}

pub(super) fn run(ctx: Arc<Ctx>, inputs: Vec<PathBuf>, options: LoadOptions) -> io::Result<()> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;

    install_software_backend()?;
    let window = PluginLoadWindow::new().map_err(io::Error::other)?;
    let state = Arc::new(Mutex::new(GuiState::new(inputs.clone())));
    window.set_prompt(input_prompt().into());
    update_rows(&window, &state);
    configure_cancel(&window);
    configure_window_region(&window);
    configure_titlebar_hit_test(&window);
    configure_load(
        &window,
        Arc::clone(&ctx),
        inputs.clone(),
        options.clone(),
        Arc::clone(&state),
        false,
    );
    configure_load(&window, ctx, inputs, options, state, true);
    window.run().map_err(io::Error::other)
}

fn install_software_backend() -> io::Result<()> {
    let backend = i_slint_backend_winit::Backend::builder()
        .with_renderer_name("software")
        .build()
        .map_err(|error| io::Error::other(error.to_string()))?;
    slint::platform::set_platform(Box::new(backend))
        .map_err(|error| io::Error::other(error.to_string()))
}

fn input_prompt() -> &'static str {
    "是否确认加载如下插件？"
}

fn configure_cancel(window: &PluginLoadWindow) {
    let window_weak = window.as_weak();
    window.on_cancel_requested(move || {
        if let Some(window) = window_weak.upgrade() {
            let _ = window.hide();
        }
    });
}

fn configure_window_region(window: &PluginLoadWindow) {
    let window_weak = window.as_weak();
    window.on_apply_window_region(move || {
        let Some(window) = window_weak.upgrade() else {
            return false;
        };
        apply_window_region(&window)
    });
}

fn native_window_handle(window: &PluginLoadWindow) -> Option<HWND> {
    let window_handle = window.window().window_handle();
    let raw_window_handle = window_handle.window_handle().ok()?;
    let RawWindowHandle::Win32(window_handle) = raw_window_handle.as_raw() else {
        return None;
    };
    Some(isize::from(window_handle.hwnd) as HWND)
}

fn apply_window_region(window: &PluginLoadWindow) -> bool {
    let Some(window_handle) = native_window_handle(window) else {
        return false;
    };
    let size = window.window().size();
    let corner_diameter = rounded_corner_diameter(window.window().scale_factor());
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

fn configure_titlebar_hit_test(window: &PluginLoadWindow) {
    let window_weak = window.as_weak();
    window.on_install_titlebar_hit_test(move || {
        let Some(window) = window_weak.upgrade() else {
            return false;
        };
        let Some(window_handle) = native_window_handle(&window) else {
            return false;
        };
        let size = window.window().size();
        install_titlebar_hit_test(
            window_handle,
            size.width as i32,
            window.window().scale_factor(),
        )
    });
}

fn popup_window_style(current_style: u32) -> u32 {
    WS_POPUP | (current_style & WS_VISIBLE)
}

fn rounded_corner_diameter(scale_factor: f32) -> i32 {
    (24.0 * scale_factor).round() as i32
}

#[derive(Clone, Copy)]
struct TitlebarHitTest {
    original_procedure: WNDPROC,
    width: i32,
    scale_factor: f32,
}

fn titlebar_hit_tests() -> &'static Mutex<HashMap<isize, TitlebarHitTest>> {
    static TITLEBAR_HIT_TESTS: OnceLock<Mutex<HashMap<isize, TitlebarHitTest>>> = OnceLock::new();
    TITLEBAR_HIT_TESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn install_titlebar_hit_test(window_handle: HWND, width: i32, scale_factor: f32) -> bool {
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
            scale_factor,
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
    let titlebar_height = logical_to_physical(36, hit_test.scale_factor);
    let left_inset = logical_to_physical(8, hit_test.scale_factor);
    let close_button_inset = logical_to_physical(40, hit_test.scale_factor);
    screen_x >= window_rect.left + left_inset
        && screen_x < window_rect.left + hit_test.width - close_button_inset
        && screen_y >= window_rect.top
        && screen_y < window_rect.top + titlebar_height
}

fn logical_to_physical(value: i32, scale_factor: f32) -> i32 {
    (value as f32 * scale_factor).round() as i32
}

fn configure_load(
    window: &PluginLoadWindow,
    ctx: Arc<Ctx>,
    inputs: Vec<PathBuf>,
    options: LoadOptions,
    state: Arc<Mutex<GuiState>>,
    localboost: bool,
) {
    let window_weak = window.as_weak();
    let load = move || {
        let Some(window) = window_weak.upgrade() else {
            return;
        };
        let (attempt_inputs, attempt_localboost) = match state.lock() {
            Ok(mut state) if !localboost && !state.failed_paths.is_empty() => {
                let inputs = state.failed_paths.clone();
                let localboost = state.retry_uses_localboost;
                state.begin(inputs.clone(), localboost);
                (inputs, localboost)
            }
            Ok(mut state) => {
                state.begin(inputs.clone(), localboost);
                (inputs.clone(), localboost)
            }
            Err(_) => return,
        };
        window.set_prompt(loading_prompt(attempt_inputs.len()).into());
        update_rows(&window, &state);
        window.set_busy(true);
        window.set_retry_available(false);
        let load_options = LoadOptions {
            local_boost: if attempt_localboost {
                LocalBoostHandling::Load
            } else {
                options.local_boost
            },
            on_inputs_expanded: Some(Arc::new({
                let window_weak = window.as_weak();
                let state = Arc::clone(&state);
                move |expanded_paths| {
                    let window_weak = window_weak.clone();
                    let state = Arc::clone(&state);
                    let loading_count = expanded_paths.len();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(window) = window_weak.upgrade() else {
                            return;
                        };
                        let rows = if let Ok(mut state) = state.lock() {
                            state.begin(expanded_paths, attempt_localboost);
                            state.rows()
                        } else {
                            return;
                        };
                        window.set_rows(ModelRc::new(VecModel::from(rows)));
                        window.set_prompt(loading_prompt(loading_count).into());
                    });
                }
            })),
            on_progress: Some(Arc::new({
                let window_weak = window.as_weak();
                let state = Arc::clone(&state);
                move |progress| {
                    let window_weak = window_weak.clone();
                    let state = Arc::clone(&state);
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(window) = window_weak.upgrade() else {
                            return;
                        };
                        let rows = if let Ok(mut state) = state.lock() {
                            state.apply_progress(progress);
                            state.rows()
                        } else {
                            return;
                        };
                        window.set_rows(ModelRc::new(VecModel::from(rows)));
                    });
                }
            })),
            ..options.clone()
        };
        let window_weak = window.as_weak();
        let ctx = Arc::clone(&ctx);
        let state = Arc::clone(&state);
        std::thread::spawn(move || {
            let result =
                eli_lib::command::plugin::load(ctx.as_ref(), &attempt_inputs, load_options);
            let _ = slint::invoke_from_event_loop(move || {
                let Some(window) = window_weak.upgrade() else {
                    return;
                };
                let all_succeeded = match state.lock() {
                    Ok(mut state) => match result {
                        Ok(summary) => state.finish(summary),
                        Err(error) => {
                            state.fail_to_start(attempt_inputs, error);
                            false
                        }
                    },
                    Err(_) => false,
                };
                if all_succeeded {
                    update_rows(&window, &state);
                    let window_weak = window.as_weak();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(1200));
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(window) = window_weak.upgrade() {
                                let _ = window.hide();
                            }
                        });
                    });
                    return;
                }
                update_rows(&window, &state);
                window.set_prompt("部分插件包加载失败，请重试".into());
                window.set_busy(false);
                window.set_retry_available(true);
            });
        });
    };
    if localboost {
        window.on_localboost_requested(load);
    } else {
        window.on_load_requested(load);
    }
}

fn update_rows(window: &PluginLoadWindow, state: &Arc<Mutex<GuiState>>) {
    let rows = state.lock().map(|state| state.rows()).unwrap_or_default();
    window.set_rows(ModelRc::new(VecModel::from(rows)));
}

fn loading_prompt(package_count: usize) -> String {
    format!("正在加载 {package_count} 个插件包...")
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .filter(|name| !name.is_empty())
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eli_lib::command::plugin::LoadResult;

    #[test]
    fn input_prompt_asks_for_plugin_confirmation() {
        assert_eq!(input_prompt(), "是否确认加载如下插件？");
    }

    #[test]
    fn loading_prompt_uses_the_list_count() {
        assert_eq!(loading_prompt(3), "正在加载 3 个插件包...");
    }

    #[test]
    fn rounded_corner_region_scales_with_the_display() {
        assert_eq!(rounded_corner_diameter(1.0), 24);
        assert_eq!(rounded_corner_diameter(1.25), 30);
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
            scale_factor: 1.0,
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

    #[test]
    fn progress_only_marks_the_dispatched_package_as_loading() {
        let first = PathBuf::from("first.7z");
        let second = PathBuf::from("second.7z");
        let mut state = GuiState::new(vec![first.clone(), second.clone()]);
        state.begin(vec![first.clone(), second], false);

        state.apply_progress(LoadProgress::Started { path: first });

        assert_eq!(state.rows[0].state, RowState::Loading);
        assert_eq!(state.rows[1].state, RowState::Waiting);
    }

    #[test]
    fn failed_results_are_the_only_paths_retried() {
        let mut state = GuiState::new(vec![PathBuf::from("first.7z")]);
        assert!(!state.finish(LoadSummary {
            results: vec![
                LoadResult {
                    path: PathBuf::from("first.7z"),
                    result: Ok(LoadStatus::Loaded),
                },
                LoadResult {
                    path: PathBuf::from("second.7z"),
                    result: Err(io::Error::other("failed to extract")),
                },
            ],
        }));

        assert_eq!(state.failed_paths, vec![PathBuf::from("second.7z")]);
        assert_eq!(state.rows.len(), 2);
    }
}
