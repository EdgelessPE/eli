use eli_lib::Ctx;
use eli_lib::command::plugin::{LoadOptions, LoadSummary, LocalBoostHandling};
use eli_lib::dependency::RuntimeEnvironment;
use slint::ComponentHandle;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

#[allow(deprecated)]
mod ui {
    slint::slint! {
        import { Button, ButtonSize, ButtonVariant } from "ui/slintcn/components/button.slint";

        export component PluginLoadWindow inherits Window {
            title: "加载插件";
            width: 420px;
            height: 200px;
            background: #ffffff;

            in-out property <string> input-count;
            in-out property <string> status: "请选择此批插件的加载方式。";
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
                    y: 24px;
                    text: "加载插件";
                    font-size: 16px;
                    font-weight: 600;
                    color: #111827;
                }
                Text {
                    x: parent.width - self.width - 24px;
                    y: 26px;
                    text: root.input-count;
                    font-size: 14px;
                    color: #6b7280;
                }
                Text {
                    x: 24px;
                    y: 70px;
                    width: parent.width - 48px;
                    text: root.status;
                    font-size: 14px;
                    wrap: word-wrap;
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
    window.set_input_count(format!("{} 项输入", inputs.len()).into());
    configure_cancel(&window);
    configure_load(&window, Arc::clone(&ctx), inputs.clone(), options, false);
    configure_load(&window, ctx, inputs, options, true);
    window.run().map_err(io::Error::other)
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
}
