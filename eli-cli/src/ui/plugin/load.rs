use crate::ui::window as window_support;
use eli_lib::Ctx;
use eli_lib::command::plugin::{
    LoadOptions, LoadProgress, LoadStatus, LoadSummary, LocalBoostHandling,
};
use eli_lib::dependency::RuntimeEnvironment;
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

slint::include_modules!();

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

pub(crate) fn run(ctx: Arc<Ctx>, inputs: Vec<PathBuf>, options: LoadOptions) -> io::Result<()> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;

    install_software_backend()?;
    let window = PluginLoadWindow::new().map_err(io::Error::other)?;
    let state = Arc::new(Mutex::new(GuiState::new(inputs.clone())));
    window.set_prompt(input_prompt().into());
    update_rows(&window, &state);
    configure_cancel(&window);
    configure_window(&window);
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

fn configure_window(window: &PluginLoadWindow) {
    let window_weak = window.as_weak();
    window
        .global::<WindowAdapter>()
        .on_apply_window_region(move |corner_radius| {
            let Some(window) = window_weak.upgrade() else {
                return false;
            };
            window_support::apply_window_region(window.window(), corner_radius)
        });

    let window_weak = window.as_weak();
    window
        .global::<WindowAdapter>()
        .on_install_titlebar_hit_test(move |titlebar_height, left_inset, right_inset| {
            let Some(window) = window_weak.upgrade() else {
                return false;
            };
            window_support::install_titlebar_hit_test(
                window.window(),
                titlebar_height,
                left_inset,
                right_inset,
            )
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
