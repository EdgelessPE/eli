use fs2::FileExt;
use image::codecs::webp::WebPDecoder;
use image::imageops::{FilterType, fast_blur, resize};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use std::borrow::Cow;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use tar::{Builder as TarBuilder, HeaderMode};
use webp::Encoder as WebpEncoder;

pub const DEFAULT_QUALITY: u8 = 90;
pub const DEFAULT_SLICES: u16 = 25;
pub const MAX_OUTPUT_EDGE: u32 = 4096;
pub const MAX_QUALITY: u8 = 100;
pub const MAX_SLICES: u16 = 1000;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BakeJobPhase {
    Blur,
    Encode,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BakePreparationStage {
    DecodeInput,
    Downsample {
        source_width: u32,
        source_height: u32,
        output_width: u32,
        output_height: u32,
    },
    PrepareOutput,
    PreparePixels,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BakeEvent {
    Preparing {
        stage: BakePreparationStage,
    },
    Started {
        total: usize,
        workers: usize,
    },
    JobStarted {
        progress_mark: u16,
        file_name: String,
    },
    JobPhase {
        progress_mark: u16,
        phase: BakeJobPhase,
    },
    JobFinished {
        progress_mark: u16,
        file_name: String,
        completed: usize,
        total: usize,
        elapsed: Duration,
    },
    JobFailed {
        progress_mark: u16,
        file_name: String,
        error: String,
    },
    Finished {
        elapsed: Duration,
    },
    Aborted,
}

#[derive(Debug)]
pub struct BakeResult {
    pub output: PathBuf,
    pub entries: Vec<String>,
    pub width: u32,
    pub height: u32,
    pub workers: usize,
    pub quality: u8,
    pub elapsed: Duration,
}

#[derive(Clone, Debug)]
struct FrameSpec {
    progress_mark: u16,
    sigma: f32,
    file_name: String,
}

pub fn bake(
    source: &Path,
    output: &Path,
    slices: u16,
    jobs: Option<NonZeroUsize>,
    quality: u8,
    downsample: bool,
    report: &(dyn Fn(BakeEvent) + Sync),
) -> io::Result<BakeResult> {
    let started_at = Instant::now();
    validate_slices(slices)?;
    validate_quality(quality)?;
    validate_output_path(output)?;
    let source = fs::canonicalize(source).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!(
                "failed to resolve input image {}: {error}",
                source.display()
            ),
        )
    })?;
    validate_regular_file(&source)?;

    report(BakeEvent::Preparing {
        stage: BakePreparationStage::DecodeInput,
    });
    let original = decode_static_image(&source)?;
    let original = downsample_image(original, downsample, report);
    let (width, height) = original.dimensions();
    let frames = frame_specs(width, height, slices);
    let workers = worker_count(frames.len(), jobs);

    report(BakeEvent::Preparing {
        stage: BakePreparationStage::PrepareOutput,
    });
    let mut transaction = OutputTransaction::new(output, &source)?;
    report(BakeEvent::Preparing {
        stage: BakePreparationStage::PreparePixels,
    });
    let blur_source = premultiplied_source(&original);
    let blur_source = blur_source.as_ref().unwrap_or(&original);
    let has_transparency = !std::ptr::eq(blur_source, &original);
    let next_job = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    let completion_report = Mutex::new(());
    let failures = Mutex::new(Vec::<io::Error>::new());

    report(BakeEvent::Started {
        total: frames.len(),
        workers,
    });

    let panicked = thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            handles.push(scope.spawn(|| {
                loop {
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }
                    let index = next_job.fetch_add(1, Ordering::Relaxed);
                    let Some(frame) = frames.get(index) else {
                        break;
                    };
                    if cancelled.load(Ordering::Acquire) {
                        break;
                    }

                    report(BakeEvent::JobStarted {
                        progress_mark: frame.progress_mark,
                        file_name: frame.file_name.clone(),
                    });
                    let job_started_at = Instant::now();
                    let result = process_frame(
                        &original,
                        blur_source,
                        has_transparency,
                        frame,
                        transaction.staging_path(),
                        quality,
                        report,
                    );
                    match result {
                        Ok(()) => {
                            let _report_guard = completion_report
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner());
                            let count = completed.fetch_add(1, Ordering::AcqRel) + 1;
                            report(BakeEvent::JobFinished {
                                progress_mark: frame.progress_mark,
                                file_name: frame.file_name.clone(),
                                completed: count,
                                total: frames.len(),
                                elapsed: job_started_at.elapsed(),
                            });
                        }
                        Err(error) => {
                            cancelled.store(true, Ordering::Release);
                            report(BakeEvent::JobFailed {
                                progress_mark: frame.progress_mark,
                                file_name: frame.file_name.clone(),
                                error: error.to_string(),
                            });
                            failures
                                .lock()
                                .unwrap_or_else(|poisoned| poisoned.into_inner())
                                .push(error);
                            break;
                        }
                    }
                }
            }));
        }

        let mut panicked = false;
        for handle in handles {
            panicked |= handle.join().is_err();
        }
        panicked
    });

    if panicked {
        report(BakeEvent::Aborted);
        return Err(io::Error::other("a loadscreen bake worker panicked"));
    }

    let mut failures = failures
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !failures.is_empty() {
        report(BakeEvent::Aborted);
        return Err(failures.remove(0));
    }

    let finished = completed.load(Ordering::Acquire);
    if finished != frames.len() {
        report(BakeEvent::Aborted);
        return Err(io::Error::other(format!(
            "loadscreen bake stopped after completing {} of {} jobs",
            finished,
            frames.len()
        )));
    }

    if let Err(error) = transaction.publish(&frames) {
        report(BakeEvent::Aborted);
        return Err(error);
    }
    let elapsed = started_at.elapsed();
    report(BakeEvent::Finished { elapsed });
    let output = transaction.reported_output().to_owned();
    let entries = frames.iter().map(|frame| frame.file_name.clone()).collect();

    Ok(BakeResult {
        output,
        entries,
        width,
        height,
        workers,
        quality,
        elapsed,
    })
}

fn validate_slices(slices: u16) -> io::Result<()> {
    if !(1..=MAX_SLICES).contains(&slices) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("--slices must be between 1 and {MAX_SLICES}"),
        ));
    }
    Ok(())
}

fn validate_quality(quality: u8) -> io::Result<()> {
    if quality > MAX_QUALITY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("--quality must be between 0 and {MAX_QUALITY}"),
        ));
    }
    Ok(())
}

fn validate_output_path(output: &Path) -> io::Result<()> {
    let has_tar_extension = output
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tar"));
    if !has_tar_extension {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "--output must use a file name with a .tar extension: {}",
                output.display()
            ),
        ));
    }
    Ok(())
}

fn validate_regular_file(source: &Path) -> io::Result<()> {
    let metadata = fs::metadata(source)?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("input image is not a regular file: {}", source.display()),
        ));
    }
    Ok(())
}

fn decode_static_image(source: &Path) -> io::Result<RgbaImage> {
    let reader = ImageReader::open(source)
        .and_then(ImageReader::with_guessed_format)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to inspect input image {}: {error}",
                    source.display()
                ),
            )
        })?;
    let format = reader.format().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported input image format: {}", source.display()),
        )
    })?;
    validate_format(source, format)?;
    reject_animation(source, format)?;

    let mut decoder = reader
        .into_decoder()
        .map_err(|error| invalid_image_error("create image decoder", source, error))?;
    let orientation = decoder
        .orientation()
        .map_err(|error| invalid_image_error("read image orientation", source, error))?;
    let mut image = DynamicImage::from_decoder(decoder)
        .map_err(|error| invalid_image_error("decode image", source, error))?;
    image.apply_orientation(orientation);
    if image.width() == 0 || image.height() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("input image has an empty dimension: {}", source.display()),
        ));
    }
    Ok(image.to_rgba8())
}

fn downsample_image(
    original: RgbaImage,
    enabled: bool,
    report: &(dyn Fn(BakeEvent) + Sync),
) -> RgbaImage {
    let (source_width, source_height) = original.dimensions();
    let Some((output_width, output_height)) =
        downsampled_dimensions(source_width, source_height, enabled)
    else {
        return original;
    };
    report(BakeEvent::Preparing {
        stage: BakePreparationStage::Downsample {
            source_width,
            source_height,
            output_width,
            output_height,
        },
    });
    resize(&original, output_width, output_height, FilterType::Lanczos3)
}

fn downsampled_dimensions(width: u32, height: u32, enabled: bool) -> Option<(u32, u32)> {
    let longest_edge = width.max(height);
    if !enabled || longest_edge <= MAX_OUTPUT_EDGE {
        return None;
    }
    let scaled = |edge: u32| {
        ((u64::from(edge) * u64::from(MAX_OUTPUT_EDGE) + u64::from(longest_edge) / 2)
            / u64::from(longest_edge))
        .max(1) as u32
    };
    Some(if width >= height {
        (MAX_OUTPUT_EDGE, scaled(height))
    } else {
        (scaled(width), MAX_OUTPUT_EDGE)
    })
}

fn validate_format(source: &Path, format: ImageFormat) -> io::Result<()> {
    if matches!(format, ImageFormat::Gif) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("GIF images are not supported: {}", source.display()),
        ));
    }
    if !matches!(
        format,
        ImageFormat::Jpeg
            | ImageFormat::Png
            | ImageFormat::WebP
            | ImageFormat::Bmp
            | ImageFormat::Tiff
            | ImageFormat::Ico
            | ImageFormat::Tga
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported input image format: {}", source.display()),
        ));
    }
    Ok(())
}

fn reject_animation(source: &Path, format: ImageFormat) -> io::Result<()> {
    let animated = match format {
        ImageFormat::WebP => webp_is_animated(source)?,
        ImageFormat::Png => png_is_animated(source)?,
        _ => false,
    };
    if animated {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("animated images are not supported: {}", source.display()),
        ));
    }
    Ok(())
}

fn webp_is_animated(source: &Path) -> io::Result<bool> {
    let reader = BufReader::new(File::open(source)?);
    let decoder = WebPDecoder::new(reader)
        .map_err(|error| invalid_image_error("inspect WebP animation", source, error))?;
    Ok(decoder.has_animation())
}

fn png_is_animated(source: &Path) -> io::Result<bool> {
    let file = File::open(source)?;
    let length = file.metadata()?.len();
    let mut reader = BufReader::new(file);
    let mut signature = [0_u8; 8];
    reader.read_exact(&mut signature)?;
    if signature != [137, 80, 78, 71, 13, 10, 26, 10] {
        return Ok(false);
    }

    let mut position = 8_u64;
    while position.checked_add(12).is_some_and(|end| end <= length) {
        let mut chunk_header = [0_u8; 8];
        reader.read_exact(&mut chunk_header)?;
        let size = u32::from_be_bytes(chunk_header[..4].try_into().unwrap()) as u64;
        let chunk_end = position
            .checked_add(12)
            .and_then(|value| value.checked_add(size))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid PNG chunk"))?;
        if chunk_end > length {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("truncated PNG image: {}", source.display()),
            ));
        }
        if &chunk_header[4..] == b"acTL" {
            return Ok(true);
        }
        if &chunk_header[4..] == b"IDAT" || &chunk_header[4..] == b"IEND" {
            return Ok(false);
        }
        reader.seek(SeekFrom::Current((size + 4) as i64))?;
        position = chunk_end;
    }
    Ok(false)
}

fn invalid_image_error(action: &str, source: &Path, error: image::ImageError) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("failed to {action} {}: {error}", source.display()),
    )
}

fn frame_specs(width: u32, height: u32, slices: u16) -> Vec<FrameSpec> {
    let maximum_sigma = ((width.min(height) as f32) / 32.0).max(1.0);
    (0..=slices)
        .map(|index| {
            let progress_mark = rounded_progress_mark(index, slices);
            let progress = f32::from(index) / f32::from(slices);
            FrameSpec {
                progress_mark,
                sigma: maximum_sigma * (1.0 - progress),
                file_name: format!("lsbp_{progress_mark:04}.webp"),
            }
        })
        .collect()
}

fn rounded_progress_mark(index: u16, slices: u16) -> u16 {
    ((u32::from(index) * 1000 + u32::from(slices) / 2) / u32::from(slices)) as u16
}

fn worker_count(frame_count: usize, requested_jobs: Option<NonZeroUsize>) -> usize {
    let parallelism = thread::available_parallelism().map_or(1, usize::from);
    let workers = requested_jobs.map(NonZeroUsize::get).unwrap_or(parallelism);
    frame_count.min(workers).max(1)
}

fn premultiplied_source(original: &RgbaImage) -> Option<RgbaImage> {
    if original.pixels().all(|pixel| pixel[3] == 255) {
        return None;
    }
    let mut premultiplied = original.clone();
    for pixel in premultiplied.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    Some(premultiplied)
}

fn unpremultiply(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        if alpha == 0 {
            pixel.0[..3].fill(0);
            continue;
        }
        for channel in &mut pixel.0[..3] {
            *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}

fn process_frame(
    original: &RgbaImage,
    blur_source: &RgbaImage,
    has_transparency: bool,
    frame: &FrameSpec,
    staging: &Path,
    quality: u8,
    report: &(dyn Fn(BakeEvent) + Sync),
) -> io::Result<()> {
    let image = if frame.sigma <= f32::EPSILON {
        Cow::Borrowed(original)
    } else {
        report(BakeEvent::JobPhase {
            progress_mark: frame.progress_mark,
            phase: BakeJobPhase::Blur,
        });
        let mut image = fast_blur(blur_source, frame.sigma);
        if has_transparency {
            unpremultiply(&mut image);
        }
        Cow::Owned(image)
    };

    report(BakeEvent::JobPhase {
        progress_mark: frame.progress_mark,
        phase: BakeJobPhase::Encode,
    });
    let temporary = staging.join(format!(".{}.tmp", frame.file_name));
    let destination = staging.join(&frame.file_name);
    let result = encode_webp(image.as_ref(), &temporary, quality).and_then(|()| {
        report(BakeEvent::JobPhase {
            progress_mark: frame.progress_mark,
            phase: BakeJobPhase::Write,
        });
        fs::rename(&temporary, &destination)
    });
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("failed to create {}: {error}", destination.display()),
        )
    })
}

fn encode_webp(image: &RgbaImage, destination: &Path, quality: u8) -> io::Result<()> {
    let encoded = WebpEncoder::from_rgba(image.as_raw(), image.width(), image.height())
        .encode(f32::from(quality));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(&encoded)?;
    writer.flush()?;
    writer.get_ref().sync_all()
}

struct OutputTransaction {
    output: PathBuf,
    reported_output: PathBuf,
    staging: PathBuf,
    _lock: OutputLock,
}

struct OutputLock {
    file: Option<File>,
    path: PathBuf,
}

impl OutputLock {
    fn acquire(path: PathBuf, output: &Path) -> io::Result<Self> {
        loop {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    FileExt::lock_exclusive(&file)?;
                    return Ok(Self {
                        file: Some(file),
                        path,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }

            let file = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            match FileExt::try_lock_exclusive(&file) {
                Err(error) => {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        format!(
                            "output file is already being baked {}: {error}",
                            output.display()
                        ),
                    ));
                }
                Ok(()) => {
                    // 已解锁的同名文件来自异常退出或刚结束的任务，先删除再重新原子创建。
                    let _ = fs::remove_file(&path);
                    let _ = FileExt::unlock(&file);
                }
            }
        }
    }
}

impl Drop for OutputLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            // 保持锁直到目录项删除，避免新任务取得即将被删除的旧锁文件。
            let _ = fs::remove_file(&self.path);
            let _ = FileExt::unlock(&file);
            drop(file);
        }
    }
}

impl OutputTransaction {
    fn new(output: &Path, source: &Path) -> io::Result<Self> {
        let reported_output = if output.is_absolute() {
            output.to_owned()
        } else {
            std::env::current_dir()?.join(output)
        };
        let file_name = reported_output.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("--output must name a .tar file: {}", output.display()),
            )
        })?;
        let parent = reported_output.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let parent = fs::canonicalize(parent)?;
        let output = parent.join(file_name);

        if source == output {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "input image and output file must be different",
            ));
        }

        let mut lock_name = OsString::from(".");
        lock_name.push(file_name);
        lock_name.push(".eli-loadscreen-bake.lock");
        let lock_path = parent.join(lock_name);
        let lock = OutputLock::acquire(lock_path, &reported_output)?;
        validate_output_available(&output, &reported_output)?;

        let staging = create_staging_directory(&parent, file_name)?;
        Ok(Self {
            output,
            reported_output,
            staging,
            _lock: lock,
        })
    }

    fn staging_path(&self) -> &Path {
        &self.staging
    }

    fn reported_output(&self) -> &Path {
        &self.reported_output
    }

    fn publish(&mut self, frames: &[FrameSpec]) -> io::Result<()> {
        let archive = self.staging.join(".eli-loadscreen-bake.tar");
        create_tar_archive(&self.staging, &archive, frames)?;
        validate_output_available(&self.output, &self.reported_output)?;
        fs::rename(archive, &self.output)
    }
}

impl Drop for OutputTransaction {
    fn drop(&mut self) {
        if self.staging.exists() {
            let _ = fs::remove_dir_all(&self.staging);
        }
    }
}

fn validate_output_available(output: &Path, reported_output: &Path) -> io::Result<()> {
    match fs::symlink_metadata(output) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("output file already exists: {}", reported_output.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn create_tar_archive(staging: &Path, archive: &Path, frames: &[FrameSpec]) -> io::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive)?;
    let writer = BufWriter::new(file);
    let mut builder = TarBuilder::new(writer);
    builder.mode(HeaderMode::Deterministic);
    builder.follow_symlinks(false);
    for frame in frames {
        builder.append_path_with_name(staging.join(&frame.file_name), &frame.file_name)?;
    }
    let mut writer = builder.into_inner()?;
    writer.flush()?;
    writer.get_ref().sync_all()
}

fn create_staging_directory(parent: &Path, file_name: &std::ffi::OsStr) -> io::Result<PathBuf> {
    for _ in 0..100 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut name = OsString::from(".");
        name.push(file_name);
        name.push(format!(".eli-staging-{}-{sequence}", std::process::id()));
        let staging = parent.join(name);
        match fs::create_dir(&staging) {
            Ok(()) => return Ok(staging),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "failed to allocate a staging directory beside {}",
            parent.display()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    use image::codecs::webp::WebPEncoder;
    use image::{ImageBuffer, Rgba};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn default_slices_have_the_expected_names_and_linear_sigma() {
        let frames = frame_specs(1920, 1080, DEFAULT_SLICES);

        assert_eq!(DEFAULT_SLICES, 25);
        assert_eq!(frames.len(), 26);
        assert_eq!(frames[0].file_name, "lsbp_0000.webp");
        assert_eq!(frames[1].file_name, "lsbp_0040.webp");
        assert_eq!(frames[25].file_name, "lsbp_1000.webp");
        assert!(frames.windows(2).all(|pair| pair[0].sigma > pair[1].sigma));
        let delta = frames[0].sigma - frames[1].sigma;
        let tolerance = frames[0].sigma * f32::EPSILON * 4.0;
        assert!(
            frames
                .windows(2)
                .all(|pair| { ((pair[0].sigma - pair[1].sigma) - delta).abs() < tolerance })
        );
        assert_eq!(frames.last().unwrap().sigma, 0.0);
    }

    #[test]
    fn progress_names_are_unique_up_to_the_supported_limit() {
        for slices in 1..=MAX_SLICES {
            let marks = (0..=slices)
                .map(|index| rounded_progress_mark(index, slices))
                .collect::<Vec<_>>();
            assert!(marks.windows(2).all(|pair| pair[0] < pair[1]));
            assert_eq!(marks[0], 0);
            assert_eq!(*marks.last().unwrap(), 1000);
        }
    }

    #[test]
    fn calculates_proportional_downsample_dimensions() {
        assert_eq!(downsampled_dimensions(6000, 4000, true), Some((4096, 2731)));
        assert_eq!(downsampled_dimensions(4000, 6000, true), Some((2731, 4096)));
        assert_eq!(downsampled_dimensions(4096, 2160, true), None);
        assert_eq!(downsampled_dimensions(6000, 4000, false), None);
    }

    #[test]
    fn rejects_invalid_slice_counts() {
        assert_eq!(
            validate_slices(0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            validate_slices(MAX_SLICES + 1).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(validate_quality(0).is_ok());
        assert!(validate_quality(MAX_QUALITY).is_ok());
        assert_eq!(
            validate_quality(MAX_QUALITY + 1).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn requires_a_tar_output_extension_before_reading_the_source() {
        for output in ["output", "output.bin"] {
            let error = bake(
                Path::new("missing-source.png"),
                Path::new(output),
                1,
                None,
                DEFAULT_QUALITY,
                true,
                &|_| {},
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains(".tar"));
        }
        assert!(validate_output_path(Path::new("output.TAR")).is_ok());
    }

    #[test]
    fn encodes_quality_boundaries_as_decodable_lossy_webp() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let image = ImageBuffer::from_fn(16, 16, |x, y| {
            Rgba([
                (x * 13) as u8,
                (y * 11) as u8,
                ((x + y) * 7) as u8,
                ((x * 16 + y) % 256) as u8,
            ])
        });

        for quality in [0, MAX_QUALITY] {
            let output = root.join(format!("quality-{quality}.webp"));
            encode_webp(&image, &output, quality).unwrap();
            let bytes = fs::read(&output).unwrap();
            assert!(bytes.windows(4).any(|chunk| chunk == b"VP8 "));
            let decoded = image::open(output).unwrap().to_rgba8();
            assert_eq!(decoded.dimensions(), image.dimensions());
            assert_eq!(
                decoded.pixels().map(|pixel| pixel[3]).collect::<Vec<_>>(),
                image.pixels().map(|pixel| pixel[3]).collect::<Vec<_>>()
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_animated_webp_extended_headers() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("animated.webp");
        fs::write(&source, animated_webp()).unwrap();

        assert!(webp_is_animated(&source).unwrap());
        let error = decode_static_image(&source).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("animated"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_gif_before_decoding() {
        let error = validate_format(Path::new("wallpaper.gif"), ImageFormat::Gif).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("GIF"));
    }

    #[test]
    fn rejects_a_gif_file_even_when_it_has_one_frame() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("wallpaper.gif");
        fs::write(
            &source,
            b"GIF89a\x01\0\x01\0\x80\0\0\0\0\0\xff\xff\xff!\xf9\x04\x01\0\0\0\0,\0\0\0\0\x01\0\x01\0\0\x02\x02D\x01\0;",
        )
        .unwrap();

        let error = decode_static_image(&source).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("GIF"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn bakes_default_frames_without_changing_resolution() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let output = root.join("output.tar");
        let image = ImageBuffer::from_fn(32, 18, |x, y| {
            Rgba([(x * 7) as u8, (y * 11) as u8, ((x + y) * 3) as u8, 255])
        });
        image.save(&source).unwrap();
        let events = Mutex::new(Vec::new());

        let result = bake(
            &source,
            &output,
            DEFAULT_SLICES,
            None,
            DEFAULT_QUALITY,
            true,
            &|event| {
                events
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(event);
            },
        )
        .unwrap();

        assert_eq!((result.width, result.height), (32, 18));
        assert_eq!(result.quality, DEFAULT_QUALITY);
        assert_eq!(result.output, output);
        assert_eq!(result.entries.len(), 26);
        let entries = read_archive(&output);
        assert_eq!(
            entries
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            result
                .entries
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        for (_, bytes) in &entries {
            let baked = image::load_from_memory_with_format(bytes, ImageFormat::WebP).unwrap();
            assert_eq!((baked.width(), baked.height()), (32, 18));
        }
        let clear_frame = &entries.last().unwrap().1;
        assert!(clear_frame.windows(4).any(|chunk| chunk == b"VP8 "));
        let events = events.into_inner().unwrap();
        assert!(matches!(
            events.first(),
            Some(BakeEvent::Preparing {
                stage: BakePreparationStage::DecodeInput
            })
        ));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, BakeEvent::Started { total: 26, .. }))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            BakeEvent::Preparing {
                stage: BakePreparationStage::PrepareOutput
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            BakeEvent::Preparing {
                stage: BakePreparationStage::PreparePixels
            }
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            BakeEvent::Preparing {
                stage: BakePreparationStage::Downsample { .. }
            }
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, BakeEvent::JobFinished { .. }))
                .count(),
            26
        );
        let completed = events
            .iter()
            .filter_map(|event| match event {
                BakeEvent::JobFinished { completed, .. } => Some(*completed),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(completed, (1..=26).collect::<Vec<_>>());
        assert!(matches!(events.last(), Some(BakeEvent::Finished { .. })));
        assert!(!root.join(".output.tar.eli-loadscreen-bake.lock").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn downsamples_large_images_unless_disabled() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        ImageBuffer::from_fn(4097, 3, |x, y| {
            Rgba([(x % 256) as u8, (y * 80) as u8, 100, 255])
        })
        .save(&source)
        .unwrap();

        let capped_output = root.join("capped.tar");
        let capped_events = Mutex::new(Vec::new());
        let capped = bake(&source, &capped_output, 1, None, 0, true, &|event| {
            capped_events.lock().unwrap().push(event)
        })
        .unwrap();
        assert_eq!((capped.width, capped.height), (4096, 3));
        assert!(capped_events.into_inner().unwrap().iter().any(|event| {
            matches!(
                event,
                BakeEvent::Preparing {
                    stage: BakePreparationStage::Downsample {
                        source_width: 4097,
                        source_height: 3,
                        output_width: 4096,
                        output_height: 3,
                    }
                }
            )
        }));
        for (_, bytes) in read_archive(&capped_output) {
            let image = image::load_from_memory_with_format(&bytes, ImageFormat::WebP).unwrap();
            assert_eq!((image.width(), image.height()), (4096, 3));
        }

        let original_output = root.join("original.tar");
        let original_events = Mutex::new(Vec::new());
        let uncapped = bake(&source, &original_output, 1, None, 0, false, &|event| {
            original_events.lock().unwrap().push(event)
        })
        .unwrap();
        assert_eq!((uncapped.width, uncapped.height), (4097, 3));
        assert!(!original_events.into_inner().unwrap().iter().any(|event| {
            matches!(
                event,
                BakeEvent::Preparing {
                    stage: BakePreparationStage::Downsample { .. }
                }
            )
        }));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_replace_an_existing_output_file() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let output = root.join("output.tar");
        fs::write(&output, "keep").unwrap();
        ImageBuffer::from_pixel(2, 2, Rgba([1_u8, 2, 3, 255]))
            .save(&source)
            .unwrap();

        let error = bake(&source, &output, 1, None, DEFAULT_QUALITY, true, &|_| {}).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(output).unwrap(), "keep");
        assert!(!root.join(".output.tar.eli-loadscreen-bake.lock").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_and_removes_a_stale_output_lock() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let output = root.join("output.tar");
        let lock = root.join(".output.tar.eli-loadscreen-bake.lock");
        ImageBuffer::from_pixel(2, 2, Rgba([1_u8, 2, 3, 255]))
            .save(&source)
            .unwrap();
        fs::write(&lock, []).unwrap();

        let result = bake(&source, &output, 1, None, DEFAULT_QUALITY, true, &|_| {}).unwrap();

        assert_eq!(result.entries.len(), 2);
        assert!(!lock.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_bakes_to_one_file_publish_only_one_result() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let output = root.join("output.tar");
        ImageBuffer::from_pixel(8, 8, Rgba([1_u8, 2, 3, 255]))
            .save(&source)
            .unwrap();

        let results = thread::scope(|scope| {
            let first =
                scope.spawn(|| bake(&source, &output, 1, None, DEFAULT_QUALITY, true, &|_| {}));
            let second =
                scope.spawn(|| bake(&source, &output, 1, None, DEFAULT_QUALITY, true, &|_| {}));
            [first.join().unwrap(), second.join().unwrap()]
        });

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert!(matches!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .next()
                .unwrap()
                .kind(),
            io::ErrorKind::AlreadyExists | io::ErrorKind::WouldBlock
        ));
        assert_eq!(read_archive(&output).len(), 2);
        assert!(!root.join(".output.tar.eli-loadscreen-bake.lock").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_jobs_are_capped_by_the_frame_count() {
        let requested = NonZeroUsize::new(6).unwrap();

        assert_eq!(worker_count(9, Some(requested)), 6);
        assert_eq!(worker_count(3, Some(requested)), 3);
    }

    #[test]
    fn automatic_jobs_use_the_logical_processor_count() {
        let parallelism = thread::available_parallelism().map_or(1, usize::from);

        assert_eq!(worker_count(usize::MAX, None), parallelism);
        assert_eq!(worker_count(1, None), 1);
    }

    fn read_archive(output: &Path) -> Vec<(String, Vec<u8>)> {
        let mut archive = tar::Archive::new(File::open(output).unwrap());
        archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let name = entry.path().unwrap().to_string_lossy().into_owned();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).unwrap();
                (name, bytes)
            })
            .collect()
    }

    fn animated_webp() -> Vec<u8> {
        let mut still = Vec::new();
        WebPEncoder::new_lossless(&mut still)
            .write_image(&[10, 20, 30, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();

        let mut chunks = Vec::new();
        append_webp_chunk(&mut chunks, b"VP8X", &[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        append_webp_chunk(&mut chunks, b"ANIM", &[0, 0, 0, 0, 0, 0]);
        let mut frame = vec![0_u8; 16];
        frame.extend_from_slice(&still[12..]);
        append_webp_chunk(&mut chunks, b"ANMF", &frame);

        let mut animated = b"RIFF".to_vec();
        animated.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
        animated.extend_from_slice(b"WEBP");
        animated.extend_from_slice(&chunks);
        animated
    }

    fn append_webp_chunk(output: &mut Vec<u8>, name: &[u8; 4], payload: &[u8]) {
        output.extend_from_slice(name);
        output.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        output.extend_from_slice(payload);
        if !payload.len().is_multiple_of(2) {
            output.push(0);
        }
    }

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "eli-loadscreen-bake-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
