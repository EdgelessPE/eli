use clap::Subcommand;
use eli_lib::command::loadscreen::{BakeEvent, BakeJobPhase, DEFAULT_SLICES, MAX_SLICES};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Debug, Subcommand)]
pub(crate) enum LoadscreenCommand {
    /// Bake a static image into loading-screen WebP frames.
    Bake {
        /// Path to the source static image.
        #[arg(value_name = "IMAGE")]
        image: PathBuf,
        /// Directory to create for the baked images.
        #[arg(short = 'd', long, value_name = "DIRECTORY")]
        directory: PathBuf,
        /// Number of intervals between the blurred and clear frames.
        #[arg(
            short = 's',
            long,
            default_value_t = DEFAULT_SLICES,
            value_parser = clap::value_parser!(u16).range(1..=i64::from(MAX_SLICES))
        )]
        slices: u16,
        /// Maximum number of image-processing jobs to run concurrently.
        #[arg(short = 'j', long, value_name = "COUNT")]
        jobs: Option<NonZeroUsize>,
    },
}

pub(crate) fn execute(command: LoadscreenCommand) -> io::Result<()> {
    match command {
        LoadscreenCommand::Bake {
            image,
            directory,
            slices,
            jobs,
        } => {
            let progress = ProgressDisplay::new();
            let result =
                eli_lib::command::loadscreen::bake(&image, &directory, slices, jobs, &|event| {
                    progress.handle(event)
                })?;
            println!(
                "Baked {} loadscreen images at {} ({}x{}, {} workers, {:.2?})",
                result.files.len(),
                result.directory.display(),
                result.width,
                result.height,
                result.workers,
                result.elapsed
            );
            Ok(())
        }
    }
}

struct ProgressDisplay {
    interactive: bool,
    multi: MultiProgress,
    overall: Mutex<Option<ProgressBar>>,
    jobs: Mutex<HashMap<u16, ProgressBar>>,
}

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
        if !self.interactive {
            self.handle_non_interactive(event);
            return;
        }

        match event {
            BakeEvent::Started { total, workers } => {
                let bar = self.multi.add(ProgressBar::new(total as u64));
                bar.set_style(
                    ProgressStyle::with_template(
                        "{prefix:.bold} [{bar:36.cyan/blue}] {pos}/{len} {percent}% {elapsed}",
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
                let bar = self.multi.add(ProgressBar::new_spinner());
                bar.set_style(
                    ProgressStyle::with_template("{spinner:.cyan} {prefix:.bold} {msg}")
                        .expect("valid loadscreen job template"),
                );
                bar.set_prefix(file_name);
                bar.set_message("等待");
                bar.enable_steady_tick(Duration::from_millis(100));
                self.jobs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .insert(progress_mark, bar);
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
                    bar.finish_with_message(format!("完成 · {:.2?}", elapsed));
                }
                if let Some(bar) = self
                    .overall
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .as_ref()
                {
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
                    bar.finish_and_clear();
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

fn job_label(count: usize) -> &'static str {
    if count == 1 { "job" } else { "jobs" }
}

#[cfg(test)]
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
            "--directory",
            "baked",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Loadscreen {
                command: LoadscreenCommand::Bake {
                    slices: DEFAULT_SLICES,
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
            "-d",
            "baked",
            "-s",
            "16",
            "-j",
            "3",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            crate::Command::Loadscreen {
                command: LoadscreenCommand::Bake {
                    image,
                    directory,
                    slices: 16,
                    jobs: Some(jobs),
                }
            } if image == std::path::Path::new("wallpaper.webp")
                && directory == std::path::Path::new("baked")
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
                    "-d",
                    "baked",
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
                "-d",
                "baked",
                "--jobs",
                "0",
            ])
            .is_err()
        );
    }
}
