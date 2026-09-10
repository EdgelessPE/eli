use crate::Ctx;
use crate::dependency::RuntimeEnvironment;
use std::io;
use std::path::Path;

pub fn play_demo(ctx: &Ctx, background: &Path) -> io::Result<()> {
    let environment = ctx.dependencies().environment()?;
    if !matches!(
        environment,
        RuntimeEnvironment::WindowsNormal | RuntimeEnvironment::WindowsPE
    ) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!(
                "loadscreen play requires a Windows environment, current environment is {environment}"
            ),
        ));
    }

    validate_demo_background(background)?;
    platform::play_demo(background)
}

fn validate_demo_background(background: &Path) -> io::Result<()> {
    let metadata = std::fs::metadata(background).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to read loadscreen demo background {}: {error}",
                background.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "loadscreen demo background is not a file: {}",
                background.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
mod platform {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::ComponentHandle;
    use std::io;
    use std::path::Path;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_SHOWWINDOW, SetWindowLongPtrW, SetWindowPos,
        WS_POPUP, WS_VISIBLE,
    };

    slint::slint! {
        export component LoadScreenDemo inherits Window {
            title: "Edgeless LoadScreen Demo";
            full-screen: true;
            background: #000000;

            in property <image> background-image;
            in property <int> progress: 25;
            in property <string> progress-label: "25%";
            in property <string> message: "正在初始化系统";
            in-out property <bool> fullscreen-pending: true;
            callback apply-fullscreen() -> bool;

            Timer {
                interval: 16ms;
                running: root.fullscreen-pending;
                triggered => {
                    root.fullscreen-pending = !root.apply-fullscreen();
                }
            }

            Image {
                x: 0px;
                y: 0px;
                width: parent.width;
                height: parent.height;
                source: root.background-image;
                image-fit: cover;
                horizontal-alignment: center;
                vertical-alignment: center;
            }

            Rectangle {
                width: min(parent.width - 48px, 480px);
                height: 168px;
                background: rgba(0, 0, 0, 0.24);
                border-radius: 16px;

                VerticalLayout {
                    padding-top: 24px;
                    padding-right: 32px;
                    padding-bottom: 24px;
                    padding-left: 32px;
                    spacing: 12px;
                    alignment: center;

                    Text {
                        text: root.progress-label;
                        color: #ffffff;
                        font-size: 48px;
                        font-weight: 600;
                        horizontal-alignment: center;
                    }

                    Text {
                        text: root.message;
                        color: #ffffff;
                        font-size: 18px;
                        font-weight: 500;
                        horizontal-alignment: center;
                    }

                    Rectangle {
                        height: 2px;
                        background: rgba(255, 255, 255, 0.24);
                        border-radius: 1px;

                        Rectangle {
                            width: parent.width * root.progress / 100;
                            height: parent.height;
                            background: #ffffff;
                            border-radius: 1px;
                        }
                    }
                }
            }
        }
    }

    pub(super) fn play_demo(background: &Path) -> io::Result<()> {
        install_software_backend()?;
        let decoded = image::ImageReader::open(background)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("failed to open loadscreen demo background: {error}"),
                )
            })?
            .decode()
            .map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("failed to decode loadscreen demo background: {error}"),
                )
            })?
            .into_rgba8();
        let background = slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
            decoded.as_raw(),
            decoded.width(),
            decoded.height(),
        ));
        let window = LoadScreenDemo::new().map_err(io::Error::other)?;
        window.set_background_image(background);
        window.set_progress(25);
        window.set_progress_label("25%".into());
        window.set_message("正在初始化系统".into());
        configure_native_fullscreen(&window);
        window.window().set_fullscreen(true);
        window.run().map_err(io::Error::other)
    }

    fn configure_native_fullscreen(window: &LoadScreenDemo) {
        let window_weak = window.as_weak();
        window.on_apply_fullscreen(move || {
            let Some(window) = window_weak.upgrade() else {
                return false;
            };
            apply_native_fullscreen(&window)
        });
    }

    fn apply_native_fullscreen(window: &LoadScreenDemo) -> bool {
        let Some(window_handle) = native_window_handle(window) else {
            return false;
        };
        let monitor = unsafe { MonitorFromWindow(window_handle, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_null() {
            return false;
        }
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
            return false;
        }
        let width = info.rcMonitor.right - info.rcMonitor.left;
        let height = info.rcMonitor.bottom - info.rcMonitor.top;
        unsafe {
            SetWindowLongPtrW(window_handle, GWL_STYLE, (WS_POPUP | WS_VISIBLE) as isize);
            SetWindowPos(
                window_handle,
                HWND_TOPMOST,
                info.rcMonitor.left,
                info.rcMonitor.top,
                width,
                height,
                SWP_FRAMECHANGED | SWP_SHOWWINDOW,
            ) != 0
        }
    }

    fn native_window_handle(window: &LoadScreenDemo) -> Option<HWND> {
        let window_handle = window.window().window_handle();
        let raw_window_handle = window_handle.window_handle().ok()?;
        let RawWindowHandle::Win32(window_handle) = raw_window_handle.as_raw() else {
            return None;
        };
        Some(isize::from(window_handle.hwnd) as HWND)
    }

    fn install_software_backend() -> io::Result<()> {
        let backend = i_slint_backend_winit::Backend::builder()
            .with_renderer_name("software")
            .build()
            .map_err(|error| io::Error::other(error.to_string()))?;
        slint::platform::set_platform(Box::new(backend))
            .map_err(|error| io::Error::other(error.to_string()))
    }
}

#[cfg(not(windows))]
mod platform {
    use std::io;
    use std::path::Path;

    pub(super) fn play_demo(_background: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "loadscreen play is only available on Windows",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn rejects_a_directory_as_the_demo_background() {
        let error = validate_demo_background(Path::new(".")).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("not a file"));
    }

    #[test]
    fn rejects_a_missing_demo_background() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("eli-missing-loadscreen-{unique}.jpg"));
        let error = validate_demo_background(&path).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("failed to read"));
    }
}
