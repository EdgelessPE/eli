use clap::Subcommand;
use eli_lib::Ctx;
#[cfg(feature = "loadscreen-bake")]
use eli_lib::command::loadscreen::{
    BakeEvent, BakeJobPhase, BakePreparationStage, DEFAULT_QUALITY, DEFAULT_SLICES, MAX_QUALITY,
    MAX_SLICES,
};
#[cfg(feature = "loadscreen-bake")]
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
#[cfg(feature = "loadscreen-bake")]
use std::collections::HashMap;
use std::io;
#[cfg(feature = "loadscreen-bake")]
use std::io::IsTerminal;
#[cfg(feature = "loadscreen-bake")]
use std::num::NonZeroUsize;
use std::path::PathBuf;
#[cfg(feature = "loadscreen-bake")]
use std::sync::Mutex;
#[cfg(feature = "loadscreen-bake")]
use std::time::Duration;

#[cfg(feature = "loadscreen-bake")]
const MAX_VISIBLE_JOB_BARS: usize = 8;

#[derive(Debug, Subcommand)]
pub(crate) enum LoadscreenCommand {
    /// Preview the loading-screen player.
    Play {
        /// Show the fixed demo state over a static background image.
        #[arg(long, value_name = "IMAGE")]
        demo: PathBuf,
    },
    #[cfg(feature = "loadscreen-bake")]
    /// Bake a static image into loading-screen WebP frames.
    Bake {
        /// Path to the source static image.
        #[arg(value_name = "IMAGE")]
        image: PathBuf,
        /// Tar archive to create for the baked images.
        #[arg(short = 'o', long, value_name = "FILE.tar")]
        output: PathBuf,
        /// Number of intervals between the blurred and clear frames.
        #[arg(
            short = 's',
            long,
            default_value_t = DEFAULT_SLICES,
            value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_SLICES))
        )]
        slices: u16,
        /// Lossy WebP quality from 0 (smallest) to 100 (highest quality).
        #[arg(
            short = 'q',
            long,
            default_value_t = DEFAULT_QUALITY,
            value_parser = clap::value_parser!(u8).range(0..=i64::from(MAX_QUALITY))
        )]
        quality: u8,
        /// Maximum number of image-processing jobs to run concurrently.
        #[arg(short = 'j', long, value_name = "COUNT")]
        jobs: Option<NonZeroUsize>,
        /// Keep images larger than 4096 pixels on their longest edge.
        #[arg(long)]
        no_downsample: bool,
    },
}

pub(crate) fn execute(ctx: &Ctx, command: LoadscreenCommand) -> io::Result<()> {
    match command {
        LoadscreenCommand::Play { demo } => eli_lib::command::loadscreen::play_demo(ctx, &demo),
        #[cfg(feature = "loadscreen-bake")]
        LoadscreenCommand::Bake {
            image,
            output,
            slices,
            quality,
            jobs,
            no_downsample,
        } => {
            let progress = ProgressDisplay::new();
            let result = eli_lib::command::loadscreen::bake(
                &image,
                &output,
                slices,
                jobs,
                quality,
                !no_downsample,
                &|event| progress.handle(event),
            )?;
            println!(
                "Baked {} loadscreen images to {} ({}x{}, quality {}, {} workers, {:.2?})",
                result.entries.len(),
                result.output.display(),
                result.width,
                result.height,
                result.quality,
                result.workers,
                result.elapsed
            );
            Ok(())
        }
    }
}

#[cfg(feature = "loadscreen-bake")]
struct ProgressDisplay {
    interactive: bool,
    multi: MultiProgress,
    overall: Mutex<Option<ProgressBar>>,
    jobs: Mutex<HashMap<u16, ProgressBar>>,
}

#[cfg(feature = "loadscreen-bake")]
impl ProgressDisplay {
    fn new() -> Self {
        Self {
            interactive: io::stderr().is_terminal(),
            multi: MultiProgress::new(),
            overall: Mutex::new(None),
            jobs: Mutex::new(HashMap::new()),
        }
    }

    fn handle(&self, event: BakeEvent) {
        if let BakeEvent::Preparing { stage } = &event {
            eprintln!("{}", preparation_message(*stage));
            return;
        }
        if !self.interactive {
            self.handle_non_interactive(event);
            return;
        }

        match event {
            BakeEvent::Preparing { .. } => unreachable!("preparation events are handled above"),
            BakeEvent::Started { total, workers } => {
                let bar = self.multi.add(ProgressBar::new(total as u64));
                bar.set_style(
                    ProgressStyle::with_template(
                        "{prefix:.bold} [{bar:36.cyan/blue}] {pos}/{len} {percent}% {elapsed} {msg}",
                    )
                    .expect("valid loadscreen progress template")
                    .progress_chars("=>-"),
                );
                bar.set_prefix(format!("总体 · {workers} {}", job_label(workers)));
                *self
                    .overall
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(bar);
            }
            BakeEvent::JobStarted {
                progress_mark,
                file_name,
            } => {
                let mut jobs = self
                    .jobs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if jobs.len() >= MAX_VISIBLE_JOB_BARS {
                    return;
                }
                let bar = self.multi.add(ProgressBar::new_spinner());
                bar.set_style(
                    ProgressStyle::with_template("{spinner:.cyan} {prefix:.bold} {msg}")
                        .expect("valid loadscreen job template"),
                );
                bar.set_prefix(file_name);
                bar.set_message("等待");
                bar.enable_steady_tick(Duration::from_millis(100));
                jobs.insert(progress_mark, bar);
            }
            BakeEvent::JobPhase {
                progress_mark,
                phase,
            } => {
                if let Some(bar) = self
                    .jobs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .get(&progress_mark)
                {
                    bar.set_message(match phase {
                        BakeJobPhase::Blur => "模糊",
                        BakeJobPhase::Encode => "编码",
                        BakeJobPhase::Write => "写入",
                    });
                }
            }
            BakeEvent::JobFinished {
                progress_mark,
                file_name,
                completed,
                elapsed,
                ..
            } => {
                if let Some(bar) = self
                    .jobs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&progress_mark)
                {
                    bar.finish_and_clear();
                }
                if let Some(bar) = self
                    .overall
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                {
                    bar.set_message(completed_job_message(&file_name, elapsed));
                    bar.set_position(bar.position().max(completed as u64));
                }
            }
            BakeEvent::JobFailed {
                progress_mark,
                error,
                ..
            } => {
                if let Some(bar) = self
                    .jobs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&progress_mark)
                {
                    bar.abandon_with_message(format!("失败 · {error}"));
                }
            }
            BakeEvent::Finished { .. } => {
                if let Some(bar) = self
                    .overall
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    bar.finish();
                }
            }
            BakeEvent::Aborted => {
                for (_, bar) in self
                    .jobs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .drain()
                {
                    bar.abandon_with_message("取消");
                }
                if let Some(bar) = self
                    .overall
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .take()
                {
                    bar.abandon_with_message("烘焙失败");
                }
            }
        }
    }

    fn handle_non_interactive(&self, event: BakeEvent) {
        match event {
            BakeEvent::Started { total, workers } => {
                eprintln!(
                    "Baking {total} loadscreen images with {workers} {}",
                    job_label(workers)
                );
            }
            BakeEvent::JobFinished {
                file_name,
                completed,
                total,
                elapsed,
                ..
            } => {
                eprintln!(
                    "[{completed}/{total}] {file_name} completed in {:.2?}",
                    elapsed
                );
            }
            BakeEvent::JobFailed {
                file_name, error, ..
            } => {
                eprintln!("{file_name} failed: {error}");
            }
            _ => {}
        }
    }
}

#[cfg(feature = "loadscreen-bake")]
fn job_label(count: usize) -> &'static str {
    if count == 1 { "job" } else { "jobs" }
}

#[cfg(feature = "loadscreen-bake")]
fn completed_job_message(file_name: &str, elapsed: Duration) -> String {
    format!("{file_name} 完成 · {elapsed:.2?}")
}

#[cfg(feature = "loadscreen-bake")]
fn preparation_message(stage: BakePreparationStage) -> String {
    match stage {
        BakePreparationStage::DecodeInput => "正在读取并解码输入图片…".to_owned(),
        BakePreparationStage::Downsample {
            source_width,
            source_height,
            output_width,
            output_height,
        } => format!(
            "输入图片为 {source_width}×{source_height}，正在等比降采样到 {output_width}×{output_height}…"
        ),
        BakePreparationStage::PrepareOutput => "正在准备输出文件和临时工作区…".to_owned(),
        BakePreparationStage::PreparePixels => "正在检查透明度并准备像素缓冲区…".to_owned(),
    }
}

#[cfg(all(test, feature = "loadscreen-bake"))]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_bake_with_default_slices() {
        let cli = crate::Cli::try_parse_from([
            "eli",
            "loadscreen",
            "bake",
            "wallpaper.png",
            "--output",
            "baked.tar",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Loadscreen {
                command: LoadscreenCommand::Bake {
                    slices: DEFAULT_SLICES,
                    quality: DEFAULT_QUALITY,
                    jobs: None,
                    ..
                }
            }
        ));
    }

    #[test]
    fn parses_bake_with_short_options() {
        let cli = crate::Cli::try_parse_from([
            "eli",
            "loadscreen",
            "bake",
            "wallpaper.webp",
            "-o",
            "baked.tar",
            "-s",
            "16",
            "-j",
            "3",
            "-q",
            "75",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Loadscreen {
                command: LoadscreenCommand::Bake {
                    image,
                    output,
                    slices: 16,
                    quality: 75,
                    jobs: Some(jobs),
                    no_downsample: false,
                }
            } if image == std::path::Path::new("wallpaper.webp")
                && output == std::path::Path::new("baked.tar")
                && jobs.get() == 3
        ));
    }

    #[test]
    fn rejects_slice_counts_outside_the_supported_range() {
        for value in ["0", "1001"] {
            assert!(
                crate::Cli::try_parse_from([
                    "eli",
                    "loadscreen",
                    "bake",
                    "wallpaper.png",
                    "-o",
                    "baked.tar",
                    "--slices",
                    value,
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn rejects_zero_jobs() {
        assert!(
            crate::Cli::try_parse_from([
                "eli",
                "loadscreen",
                "bake",
                "wallpaper.png",
                "-o",
                "baked.tar",
                "--jobs",
                "0",
            ])
            .is_err()
        );
    }

    #[test]
    fn accepts_quality_boundaries_and_rejects_values_above_one_hundred() {
        for value in ["0", "100"] {
            assert!(
                crate::Cli::try_parse_from([
                    "eli",
                    "loadscreen",
                    "bake",
                    "wallpaper.png",
                    "-o",
                    "baked.tar",
                    "--quality",
                    value,
                ])
                .is_ok()
            );
        }
        assert!(
            crate::Cli::try_parse_from([
                "eli",
                "loadscreen",
                "bake",
                "wallpaper.png",
                "-o",
                "baked.tar",
                "--quality",
                "101",
            ])
            .is_err()
        );
    }

    #[test]
    fn parses_the_downsample_opt_out() {
        let cli = crate::Cli::try_parse_from([
            "eli",
            "loadscreen",
            "bake",
            "wallpaper.png",
            "-o",
            "baked.tar",
            "--no-downsample",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Loadscreen {
                command: LoadscreenCommand::Bake {
                    no_downsample: true,
                    ..
                }
            }
        ));
    }

    #[test]
    fn formats_preparation_messages() {
        assert_eq!(
            preparation_message(BakePreparationStage::DecodeInput),
            "正在读取并解码输入图片…"
        );
        assert_eq!(
            preparation_message(BakePreparationStage::Downsample {
                source_width: 6000,
                source_height: 4000,
                output_width: 4096,
                output_height: 2731,
            }),
            "输入图片为 6000×4000，正在等比降采样到 4096×2731…"
        );
    }

    #[test]
    fn formats_the_latest_completed_job_for_the_overall_progress_line() {
        assert_eq!(
            completed_job_message("lsbp_0560.webp", Duration::from_millis(10_550)),
            "lsbp_0560.webp 完成 · 10.55s"
        );
    }

    #[test]
    fn limits_the_number_of_visible_concurrent_job_bars() {
        let progress = ProgressDisplay {
            interactive: true,
            multi: MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::hidden()),
            overall: Mutex::new(None),
            jobs: Mutex::new(HashMap::new()),
        };
        progress.handle(BakeEvent::Started {
            total: MAX_VISIBLE_JOB_BARS + 4,
            workers: MAX_VISIBLE_JOB_BARS + 4,
        });
        for progress_mark in 0..(MAX_VISIBLE_JOB_BARS as u16 + 4) {
            progress.handle(BakeEvent::JobStarted {
                progress_mark,
                file_name: format!("lsbp_{progress_mark:04}.webp"),
            });
        }

        assert_eq!(
            progress
                .jobs
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            MAX_VISIBLE_JOB_BARS
        );
        progress.handle(BakeEvent::Aborted);
    }
}

#[cfg(test)]
mod play_tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_play_demo_with_a_background_image() {
        let cli = crate::Cli::try_parse_from([
            "eli",
            "loadscreen",
            "play",
            "--demo",
            r"D:\Download\img0.jpg",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Loadscreen {
                command: LoadscreenCommand::Play { demo }
            } if demo == std::path::Path::new(r"D:\Download\img0.jpg")
        ));
    }

    #[test]
    fn play_demo_requires_a_background_image() {
        assert!(crate::Cli::try_parse_from(["eli", "loadscreen", "play", "--demo"]).is_err());
    }
}
