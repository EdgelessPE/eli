use fs2::FileExt;
use image::codecs::webp::{WebPDecoder, WebPEncoder};
use image::imageops::fast_blur;
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, RgbaImage};
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

pub const DEFAULT_SLICES: u16 = 8;
pub const MAX_SLICES: u16 = 1000;

const MAX_AUTOMATIC_WORKERS: usize = 4;
const WORKING_MEMORY_BUDGET: u64 = 512 * 1024 * 1024;
const ESTIMATED_BYTES_PER_WORKER_PIXEL: u64 = 16;

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BakeJobPhase {
    Blur,
    Encode,
    Write,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BakeEvent {
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
    pub directory: PathBuf,
    pub files: Vec<PathBuf>,
    pub width: u32,
    pub height: u32,
    pub workers: usize,
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
    directory: &Path,
    slices: u16,
    jobs: Option<NonZeroUsize>,
    report: &(dyn Fn(BakeEvent) + Sync),
) -> io::Result<BakeResult> {
    validate_slices(slices)?;
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

    let original = decode_static_image(&source)?;
    let (width, height) = original.dimensions();
    let frames = frame_specs(width, height, slices);
    let workers = worker_count(width, height, frames.len(), jobs);
    let started_at = Instant::now();

    let mut transaction = OutputTransaction::new(directory, &source)?;
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

    if let Err(error) = transaction.publish() {
        report(BakeEvent::Aborted);
        return Err(error);
    }
    let elapsed = started_at.elapsed();
    report(BakeEvent::Finished { elapsed });
    let directory = transaction.reported_destination().to_owned();
    let files = frames
        .iter()
        .map(|frame| directory.join(&frame.file_name))
        .collect();

    Ok(BakeResult {
        directory,
        files,
        width,
        height,
        workers,
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

fn worker_count(
    width: u32,
    height: u32,
    frame_count: usize,
    requested_jobs: Option<NonZeroUsize>,
) -> usize {
    let parallelism = thread::available_parallelism().map_or(1, usize::from);
    let bytes_per_worker = u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(ESTIMATED_BYTES_PER_WORKER_PIXEL)
        .max(1);
    let memory_workers = (WORKING_MEMORY_BUDGET / bytes_per_worker).max(1) as usize;
    let requested_workers = requested_jobs
        .map(NonZeroUsize::get)
        .unwrap_or_else(|| parallelism.min(MAX_AUTOMATIC_WORKERS));
    frame_count
        .min(requested_workers)
        .min(memory_workers)
        .max(1)
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
    let result = encode_webp(image.as_ref(), &temporary).and_then(|()| {
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

fn encode_webp(image: &RgbaImage, destination: &Path) -> io::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut writer = BufWriter::new(file);
    WebPEncoder::new_lossless(&mut writer)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| io::Error::other(format!("WebP encoding failed: {error}")))?;
    writer.flush()?;
    writer.get_ref().sync_all()
}

struct OutputTransaction {
    destination: PathBuf,
    reported_destination: PathBuf,
    staging: PathBuf,
    _lock: File,
    published: bool,
}

impl OutputTransaction {
    fn new(directory: &Path, source: &Path) -> io::Result<Self> {
        let reported_destination = if directory.is_absolute() {
            directory.to_owned()
        } else {
            std::env::current_dir()?.join(directory)
        };
        let file_name = reported_destination.file_name().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "output directory must name a new directory: {}",
                    directory.display()
                ),
            )
        })?;
        let parent = reported_destination
            .parent()
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let parent = fs::canonicalize(parent)?;
        let destination = parent.join(file_name);

        if source.starts_with(&destination) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "input image must not be inside the output directory",
            ));
        }

        let mut lock_name = OsString::from(".");
        lock_name.push(file_name);
        lock_name.push(".eli-loadscreen-bake.lock");
        let lock_path = parent.join(lock_name);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)?;
        FileExt::lock_exclusive(&lock).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "failed to lock output directory {}: {error}",
                    reported_destination.display()
                ),
            )
        })?;
        validate_destination(&destination, &reported_destination)?;

        let staging = create_staging_directory(&parent, file_name)?;
        Ok(Self {
            destination,
            reported_destination,
            staging,
            _lock: lock,
            published: false,
        })
    }

    fn staging_path(&self) -> &Path {
        &self.staging
    }

    fn reported_destination(&self) -> &Path {
        &self.reported_destination
    }

    fn publish(&mut self) -> io::Result<()> {
        match fs::symlink_metadata(&self.destination) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!(
                            "output path is not an empty directory: {}",
                            self.reported_destination.display()
                        ),
                    ));
                }
                if fs::read_dir(&self.destination)?.next().is_some() {
                    return Err(non_empty_destination_error(&self.reported_destination));
                }
                fs::remove_dir(&self.destination)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::rename(&self.staging, &self.destination)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for OutputTransaction {
    fn drop(&mut self) {
        if !self.published && self.staging.exists() {
            let _ = fs::remove_dir_all(&self.staging);
        }
    }
}

fn validate_destination(destination: &Path, reported_destination: &Path) -> io::Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!(
                        "output path is not an empty directory: {}",
                        reported_destination.display()
                    ),
                ));
            }
            if fs::read_dir(destination)?.next().is_some() {
                return Err(non_empty_destination_error(reported_destination));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn non_empty_destination_error(destination: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("output directory is not empty: {}", destination.display()),
    )
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
    use image::{ImageBuffer, Rgba};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn default_slices_have_the_expected_names_and_linear_sigma() {
        let frames = frame_specs(1920, 1080, DEFAULT_SLICES);

        assert_eq!(frames.len(), 9);
        assert_eq!(frames[0].file_name, "lsbp_0000.webp");
        assert_eq!(frames[1].file_name, "lsbp_0125.webp");
        assert_eq!(frames[8].file_name, "lsbp_1000.webp");
        assert!(frames.windows(2).all(|pair| pair[0].sigma > pair[1].sigma));
        let delta = frames[0].sigma - frames[1].sigma;
        assert!(
            frames.windows(2).all(|pair| {
                ((pair[0].sigma - pair[1].sigma) - delta).abs() < f32::EPSILON * 16.0
            })
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
    fn rejects_invalid_slice_counts() {
        assert_eq!(
            validate_slices(0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            validate_slices(MAX_SLICES + 1).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
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
        let output = root.join("output");
        let image = ImageBuffer::from_fn(32, 18, |x, y| {
            Rgba([(x * 7) as u8, (y * 11) as u8, ((x + y) * 3) as u8, 255])
        });
        image.save(&source).unwrap();
        let events = Mutex::new(Vec::new());

        let result = bake(&source, &output, DEFAULT_SLICES, None, &|event| {
            events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
        })
        .unwrap();

        assert_eq!((result.width, result.height), (32, 18));
        assert_eq!(result.files.len(), 9);
        for path in &result.files {
            let baked = image::open(path).unwrap();
            assert_eq!((baked.width(), baked.height()), (32, 18));
        }
        assert_eq!(
            image::open(output.join("lsbp_1000.webp"))
                .unwrap()
                .to_rgba8(),
            image
        );
        let events = events.into_inner().unwrap();
        assert!(matches!(
            events.first(),
            Some(BakeEvent::Started { total: 9, .. })
        ));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, BakeEvent::JobFinished { .. }))
                .count(),
            9
        );
        let completed = events
            .iter()
            .filter_map(|event| match event {
                BakeEvent::JobFinished { completed, .. } => Some(*completed),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(completed, (1..=9).collect::<Vec<_>>());
        assert!(matches!(events.last(), Some(BakeEvent::Finished { .. })));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_modify_a_non_empty_output_directory() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("keep.txt"), "keep").unwrap();
        ImageBuffer::from_pixel(2, 2, Rgba([1_u8, 2, 3, 255]))
            .save(&source)
            .unwrap();

        let error = bake(&source, &output, 1, None, &|_| {}).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(output.join("keep.txt")).unwrap(), "keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_bakes_to_one_directory_publish_only_one_result() {
        let root = temporary_directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        let output = root.join("output");
        ImageBuffer::from_pixel(8, 8, Rgba([1_u8, 2, 3, 255]))
            .save(&source)
            .unwrap();

        let results = thread::scope(|scope| {
            let first = scope.spawn(|| bake(&source, &output, 1, None, &|_| {}));
            let second = scope.spawn(|| bake(&source, &output, 1, None, &|_| {}));
            [first.join().unwrap(), second.join().unwrap()]
        });

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .next()
                .unwrap()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read_dir(&output).unwrap().count(), 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_jobs_override_the_automatic_worker_limit() {
        let requested = NonZeroUsize::new(6).unwrap();

        assert_eq!(worker_count(1, 1, 9, Some(requested)), 6);
        assert_eq!(worker_count(1, 1, 3, Some(requested)), 3);
    }

    #[test]
    fn explicit_jobs_still_respect_the_memory_budget() {
        let requested = NonZeroUsize::new(8).unwrap();

        assert_eq!(worker_count(3840, 2160, 9, Some(requested)), 4);
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
            "eli-loadscreen-bake-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
}
