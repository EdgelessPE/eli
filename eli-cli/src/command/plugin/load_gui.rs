use eli_lib::Ctx;
use eli_lib::command::plugin::{LoadOptions, LoadSummary, LocalBoostHandling};
use eli_lib::dependency::RuntimeEnvironment;
use slint::ComponentHandle;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

#[allow(deprecated)]
mod ui {
    slint::slint! {
        import { Button, ButtonSize, ButtonVariant } from "ui/slintcn/components/button.slint";

        export component PluginLoadWindow inherits Window {
            title: "插件热加载工具";
            width: 420px;
            height: 190px;
            background: #ffffff;

            in-out property <string> prompt;
            in-out property <string> plugin-label;
            in-out property <string> status: "";
            in-out property <bool> busy: false;
            callback load-requested();
            callback localboost-requested();
            callback cancel-requested();

            Rectangle {
                width: parent.width;
                height: parent.height;
                background: #ffffff;
                border-color: #000000;
                border-width: 1px;
                border-radius: 12px;

                Text {
                    x: 24px;
                    y: 26px;
                    width: parent.width - 48px;
                    text: root.prompt;
                    font-size: 14px;
                    color: #374151;
                }
                Text {
                    x: 24px;
                    y: 54px;
                    width: parent.width - 48px;
                    text: root.plugin-label;
                    font-size: 16px;
                    font-weight: 600;
                    overflow: elide;
                    color: #111827;
                }
                if root.status != "": Text {
                    x: 24px;
                    y: 82px;
                    width: parent.width - 48px;
                    text: root.status;
                    font-size: 14px;
                    color: #374151;
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
                        text: root.busy ? "加载中…" : "加载";
                        variant: ButtonVariant.default;
                        size: ButtonSize.default;
                        disabled: root.busy;
                        clicked => { root.load-requested(); }
                    }
                }
            }
        }
    }
}

use ui::PluginLoadWindow;

pub(super) fn run(ctx: Arc<Ctx>, inputs: Vec<PathBuf>, options: LoadOptions) -> io::Result<()> {
    ctx.dependencies()
        .require_environment(RuntimeEnvironment::WindowsPE)?;

    let window = PluginLoadWindow::new().map_err(io::Error::other)?;
    window.set_prompt(input_prompt(&inputs).into());
    window.set_plugin_label(input_summary(&inputs).into());
    configure_cancel(&window);
    configure_load(&window, Arc::clone(&ctx), inputs.clone(), options, false);
    configure_load(&window, ctx, inputs, options, true);
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
    localboost: bool,
) {
    let window_weak = window.as_weak();
    let load = move || {
        let Some(window) = window_weak.upgrade() else {
            return;
        };
        let inputs = inputs.clone();
        let load_options = LoadOptions {
            local_boost: if localboost {
                LocalBoostHandling::Load
            } else {
                options.local_boost
            },
            ..options
        };
        window.set_busy(true);
        window.set_status(
            if localboost {
                "正在通过 LocalBoost 加载插件。"
            } else {
                "正在加载插件。"
            }
            .into(),
        );
        let window_weak = window.as_weak();
        let ctx = Arc::clone(&ctx);
        std::thread::spawn(move || {
            let status = match eli_lib::command::plugin::load(ctx.as_ref(), &inputs, load_options) {
                Ok(summary) => format_summary(&summary),
                Err(error) => format!("加载未开始：{error}"),
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = window_weak.upgrade() {
                    window.set_busy(false);
                    window.set_status(status.into());
                }
            });
        });
    };
    if localboost {
        window.on_localboost_requested(load);
    } else {
        window.on_load_requested(load);
    }
}

fn format_summary(summary: &LoadSummary) -> String {
    format!(
        "已完成：{} 成功，{} 失败，{} 跳过。",
        summary.succeeded(),
        summary.failed(),
        summary.skipped()
    )
}

fn input_summary(inputs: &[PathBuf]) -> String {
    let first = inputs
        .first()
        .map(|path| {
            path.file_name()
                .filter(|name| !name.is_empty())
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();
    if inputs.len() <= 1 {
        first
    } else {
        format!("{first} + {} 项", inputs.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eli_lib::command::plugin::{LoadResult, LoadStatus};

    #[test]
    fn summary_only_shows_the_aggregate_result() {
        let summary = LoadSummary {
            results: vec![
                LoadResult {
                    path: PathBuf::from("good.7z"),
                    result: Ok(LoadStatus::Loaded),
                },
                LoadResult {
                    path: PathBuf::from("bad.7z"),
                    result: Err(io::Error::other("失败")),
                },
            ],
        };

        assert_eq!(format_summary(&summary), "已完成：1 成功，1 失败，0 跳过。");
    }

    #[test]
    fn input_summary_shows_the_first_plugin_name_and_remaining_count() {
        let inputs = vec![
            PathBuf::from(r"D:\插件\工具.7z"),
            PathBuf::from(r"D:\插件\办公.7z"),
        ];

        assert_eq!(input_summary(&inputs), "工具.7z + 1 项");
    }

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
}
