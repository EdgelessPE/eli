use eli_lib::Ctx;
use eli_lib::command::plugin::{LoadOptions, LoadStatus, LoadSummary, LocalBoostHandling};
use eli_lib::dependency::RuntimeEnvironment;
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[allow(deprecated)]
mod ui {
    slint::slint! {
        import { Button, ButtonSize, ButtonVariant } from "ui/slintcn/components/button.slint";
        import { Tooltip } from "ui/slintcn/components/tooltip.slint";
        import { ScrollView } from "std-widgets.slint";

        export struct PluginRow {
            label: string,
            icon-color: color,
            show-status: bool,
            loading: bool,
            detail: string,
        }

        export component PluginLoadWindow inherits Window {
            title: "插件热加载工具";
            width: 420px;
            height: 248px;
            background: #ffffff;

            in-out property <string> prompt;
            in-out property <[PluginRow]> rows: [];
            in-out property <bool> busy: false;
            in-out property <bool> retry-available: false;
            in-out property <int> spinner-frame: 0;
            in-out property <string> tooltip-text: "";
            in-out property <bool> tooltip-open: false;
            in-out property <length> tooltip-anchor-y: 0px;
            callback load-requested();
            callback localboost-requested();
            callback cancel-requested();

            Timer {
                interval: 33ms;
                running: root.busy;
                triggered => {
                    root.spinner-frame = Math.mod(root.spinner-frame + 1, 24);
                }
            }

            Rectangle {
                width: parent.width;
                height: parent.height;
                background: #ffffff;
                border-color: #000000;
                border-width: 1px;
                border-radius: 12px;

                Text {
                    x: 24px;
                    y: 24px;
                    width: parent.width - 40px;
                    text: root.prompt;
                    font-size: 14px;
                    color: #374151;
                }
                scroll := ScrollView {
                    x: 24px;
                    y: 52px;
                    width: parent.width - 40px;
                    height: 116px;
                    viewport-height: root.rows.length * 32px;
                    Rectangle {
                        width: parent.width;
                        height: root.rows.length * 32px;
                        for row[index] in root.rows: Rectangle {
                            y: index * 32px;
                            width: parent.width;
                            height: 28px;
                            Text { x: 0; y: 3px; width: parent.width - 36px; text: row.label; font-size: 16px; font-weight: 600; overflow: elide; color: #111827; }
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
                        }
                    }
                }
                for row[index] in root.rows: Rectangle {
                    x: 24px;
                    y: 52px + index * 32px - scroll.viewport-y;
                    width: parent.width - 40px;
                    height: 28px;
                    background: transparent;
                    hover := TouchArea {
                        x: parent.width - 20px;
                        width: 20px;
                        height: parent.height;
                        changed has-hover => {
                            if (self.has-hover && row.detail != "") {
                                root.tooltip-text = row.detail;
                                root.tooltip-anchor-y = parent.y;
                                root.tooltip-open = true;
                            } else if (!self.has-hover) {
                                root.tooltip-open = false;
                            }
                        }
                    }
                }
                HorizontalLayout {
                    x: 24px;
                    y: parent.height - 60px;
                    width: parent.width - 48px;
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
                    x: 24px;
                    width: parent.width - 48px;
                    text: root.tooltip-text;
                    open: root.tooltip-open;
                    anchor-y: root.tooltip-anchor-y;
                }
            }
        }
    }
}

use ui::{PluginLoadWindow, PluginRow};

#[derive(Clone, Copy)]
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
                state: RowState::Loading,
                detail: String::new(),
            })
            .collect();
        self.failed_paths.clear();
        self.retry_uses_localboost = localboost;
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

    let window = PluginLoadWindow::new().map_err(io::Error::other)?;
    let state = Arc::new(Mutex::new(GuiState::new(inputs.clone())));
    window.set_prompt(input_prompt(&inputs).into());
    update_rows(&window, &state);
    configure_cancel(&window);
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

fn input_prompt(inputs: &[PathBuf]) -> &'static str {
    let directory_count = inputs
        .iter()
        .filter(|path| fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        .count();
    let file_count = inputs.len().saturating_sub(directory_count);

    match (file_count, directory_count) {
        (1, 0) => "是否将这个文件作为插件包加载？",
        (_, 0) => "是否将这些文件作为插件包加载？",
        (0, 1) => "是否加载此目录中的插件包？",
        (0, _) => "是否加载这些目录中的插件包？",
        _ => "是否加载这些文件和目录中的插件包？",
    }
}

fn configure_cancel(window: &PluginLoadWindow) {
    let window_weak = window.as_weak();
    window.on_cancel_requested(move || {
        if let Some(window) = window_weak.upgrade() {
            let _ = window.hide();
        }
    });
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

fn file_label(path: &PathBuf) -> String {
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
    fn input_prompt_describes_missing_paths_as_files() {
        assert_eq!(
            input_prompt(&[PathBuf::from("plugin.7z")]),
            "是否将这个文件作为插件包加载？"
        );
    }

    #[test]
    fn input_prompt_describes_mixed_file_and_directory_inputs() {
        assert_eq!(
            input_prompt(&[PathBuf::from("plugin.7z"), PathBuf::from(".")]),
            "是否加载这些文件和目录中的插件包？"
        );
    }

    #[test]
    fn directory_prompt_does_not_embed_the_directory_name() {
        assert_eq!(
            input_prompt(&[PathBuf::from(".")]),
            "是否加载此目录中的插件包？"
        );
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
