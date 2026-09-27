use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use rust_platform_windows::screen_capture::{CaptureError, RgbaFrame};
use rust_protocol::{
    PhysicalRect, RequestId, ScreenTextBlock, SourceContext, TranslatedScreenBlock,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const DEFAULT_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_MAX_BYTES: u64 = 500 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ScreenshotRetention {
    pub(crate) enabled: bool,
    pub(crate) ttl: Duration,
    pub(crate) max_bytes: u64,
}

impl Default for ScreenshotRetention {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl: DEFAULT_TTL,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScreenshotMetadata {
    pub(crate) request_id: RequestId,
    pub(crate) crop_x: i32,
    pub(crate) crop_y: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) blocks: Vec<ScreenTextBlock>,
    pub(crate) translations: Vec<TranslatedScreenBlock>,
    pub(crate) context: SourceContext,
    pub(crate) created_unix_ms: u64,
    pub(crate) expires_unix_ms: u64,
    pub(crate) crop_filename: String,
}

#[derive(Debug)]
#[allow(dead_code)] // Consumed by the approved screenshot-history UI in Task 14.
pub(crate) struct StoredScreenshot {
    pub(crate) metadata: ScreenshotMetadata,
    pub(crate) crop: RgbaFrame,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CleanupReport {
    pub(crate) removed_entries: usize,
    pub(crate) removed_orphans: usize,
    pub(crate) error_count: usize,
}

#[derive(Debug, Error)]
pub(crate) enum ScreenshotStoreError {
    #[error("screenshot storage is unavailable: {0}")]
    Io(#[from] std::io::Error),
    #[error("screenshot metadata is invalid: {0}")]
    Metadata(#[from] serde_json::Error),
    #[error("screenshot image is invalid: {0}")]
    Png(String),
    #[error("screenshot pixels are invalid: {0}")]
    Pixels(#[from] CaptureError),
    #[error("the stored screenshot dimensions do not match its metadata")]
    #[allow(dead_code)] // Exercised once persisted screenshots are reopened by the UI.
    DimensionMismatch,
}

pub(crate) struct ScreenshotStore {
    root: PathBuf,
    retention: ScreenshotRetention,
    deleted: Mutex<HashSet<RequestId>>,
}

impl ScreenshotStore {
    pub(crate) fn open_default(
        retention: ScreenshotRetention,
    ) -> Result<Self, ScreenshotStoreError> {
        let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
        })?;
        Self::open(
            Path::new(&local)
                .join("SayFlex")
                .join("screen-translations"),
            retention,
        )
    }

    pub(crate) fn open(
        root: impl AsRef<Path>,
        retention: ScreenshotRetention,
    ) -> Result<Self, ScreenshotStoreError> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        rust_platform_windows::protect_private_directory(&root)?;
        Ok(Self {
            root,
            retention,
            deleted: Mutex::new(HashSet::new()),
        })
    }

    #[cfg(test)]
    fn open_at(
        root: impl AsRef<Path>,
        retention: ScreenshotRetention,
    ) -> Result<Self, ScreenshotStoreError> {
        Self::open(root, retention)
    }

    pub(crate) fn persist_crop(
        &self,
        mut metadata: ScreenshotMetadata,
        crop: &RgbaFrame,
    ) -> Result<Option<PathBuf>, ScreenshotStoreError> {
        let deleted = self
            .deleted
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deleted.contains(&metadata.request_id) {
            return Ok(None);
        }
        if !self.retention.enabled {
            return Ok(None);
        }
        metadata.crop_x = crop.rect.x;
        metadata.crop_y = crop.rect.y;
        metadata.width = crop.rect.width;
        metadata.height = crop.rect.height;
        let retention_expiry = metadata.created_unix_ms.saturating_add(
            self.retention
                .ttl
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        );
        metadata.expires_unix_ms = if metadata.expires_unix_ms == 0 {
            retention_expiry
        } else {
            metadata.expires_unix_ms.min(retention_expiry)
        };
        let stem = request_stem(metadata.request_id);
        metadata.crop_filename = format!("{stem}.png");
        let image = self.root.join(&metadata.crop_filename);
        let document = self.root.join(format!("{stem}.json"));
        let image_temporary = self.root.join(format!("{stem}.png.tmp"));
        let document_temporary = self.root.join(format!("{stem}.json.tmp"));
        let image_previous = self.root.join(format!("{stem}.png.previous"));
        let document_previous = self.root.join(format!("{stem}.json.previous"));

        write_png(&image_temporary, crop)?;
        write_json(&document_temporary, &metadata)?;
        rust_platform_windows::replace_file_atomically(&image, &image_temporary, &image_previous)?;
        if let Err(error) = rust_platform_windows::replace_file_atomically(
            &document,
            &document_temporary,
            &document_previous,
        ) {
            let _ = fs::remove_file(&document_temporary);
            if image_previous.is_file() {
                let rollback_backup = self.root.join(format!("{stem}.png.rollback"));
                let _ = rust_platform_windows::replace_file_atomically(
                    &image,
                    &image_previous,
                    &rollback_backup,
                );
                let _ = fs::remove_file(rollback_backup);
            } else {
                let _ = fs::remove_file(&image);
            }
            return Err(error.into());
        }
        let _ = fs::remove_file(image_previous);
        let _ = fs::remove_file(document_previous);
        Ok(Some(document))
    }

    #[allow(dead_code)] // Public module contract; wired to history in Task 14.
    pub(crate) fn load_crop(
        &self,
        request_id: RequestId,
    ) -> Result<Option<StoredScreenshot>, ScreenshotStoreError> {
        let deleted = self
            .deleted
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deleted.contains(&request_id) {
            return Ok(None);
        }
        let stem = request_stem(request_id);
        let document = self.root.join(format!("{stem}.json"));
        if !document.exists() {
            return Ok(None);
        }
        let metadata: ScreenshotMetadata = serde_json::from_slice(&fs::read(document)?)?;
        if metadata.request_id != request_id || metadata.crop_filename != format!("{stem}.png") {
            return Err(ScreenshotStoreError::DimensionMismatch);
        }
        let image = self.root.join(&metadata.crop_filename);
        if !image.is_file() {
            return Ok(None);
        }
        let crop = read_png(
            &image,
            PhysicalRect {
                x: metadata.crop_x,
                y: metadata.crop_y,
                width: metadata.width,
                height: metadata.height,
            },
        )?;
        Ok(Some(StoredScreenshot { metadata, crop }))
    }

    #[allow(dead_code)] // Public module contract; wired to history in Task 14.
    pub(crate) fn delete_now(&self, request_id: RequestId) -> Result<(), ScreenshotStoreError> {
        let mut deleted = self
            .deleted
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        deleted.insert(request_id);
        let stem = request_stem(request_id);
        remove_if_present(&self.root.join(format!("{stem}.json")))?;
        remove_if_present(&self.root.join(format!("{stem}.png")))?;
        remove_if_present(&self.root.join(format!("{stem}.json.previous")))?;
        remove_if_present(&self.root.join(format!("{stem}.png.previous")))?;
        Ok(())
    }

    #[allow(dead_code)] // Public module contract; wired to history in Task 14.
    pub(crate) fn save_permanent(
        &self,
        request_id: RequestId,
        destination: &Path,
    ) -> Result<(), ScreenshotStoreError> {
        let deleted = self
            .deleted
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deleted.contains(&request_id) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "the temporary screenshot was deleted",
            )
            .into());
        }
        let source = self.root.join(format!("{}.png", request_stem(request_id)));
        let temporary = destination.with_extension("png.tmp");
        let previous = destination.with_extension("png.previous");
        fs::copy(source, &temporary)?;
        rust_platform_windows::replace_file_atomically(destination, &temporary, &previous)?;
        let _ = fs::remove_file(previous);
        Ok(())
    }

    pub(crate) fn cleanup(&self, now_unix_ms: u64) -> CleanupReport {
        let _deleted = self
            .deleted
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut report = CleanupReport::default();
        let Ok(directory) = fs::read_dir(&self.root) else {
            report.error_count += 1;
            return report;
        };
        let paths = directory
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        let mut entries = Vec::<StoredEntry>::new();
        let mut referenced_images = std::collections::HashSet::new();
        for path in &paths {
            if path.extension().is_some_and(|extension| extension == "tmp")
                || path
                    .extension()
                    .is_some_and(|extension| extension == "previous")
                || path
                    .extension()
                    .is_some_and(|extension| extension == "rollback")
            {
                match remove_if_present(path) {
                    Ok(()) => report.removed_orphans += 1,
                    Err(_) => report.error_count += 1,
                }
                continue;
            }
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let metadata = fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<ScreenshotMetadata>(&bytes).ok());
            let Some(metadata) = metadata else {
                if remove_if_present(path).is_err() {
                    report.error_count += 1;
                } else {
                    report.removed_orphans += 1;
                }
                continue;
            };
            let stem = request_stem(metadata.request_id);
            let expected_document = self.root.join(format!("{stem}.json"));
            let expected_filename = format!("{stem}.png");
            if path != &expected_document || metadata.crop_filename != expected_filename {
                if remove_if_present(path).is_err() {
                    report.error_count += 1;
                } else {
                    report.removed_orphans += 1;
                }
                continue;
            }
            let image = self.root.join(&metadata.crop_filename);
            referenced_images.insert(image.clone());
            if metadata.expires_unix_ms <= now_unix_ms || !image.is_file() {
                if remove_pair(path, &image).is_err() {
                    report.error_count += 1;
                } else {
                    report.removed_entries += 1;
                }
                continue;
            }
            let bytes = file_len(path).saturating_add(file_len(&image));
            entries.push(StoredEntry {
                document: path.clone(),
                image,
                created_unix_ms: metadata.created_unix_ms,
                bytes,
            });
        }
        for path in paths {
            if path.extension().is_some_and(|extension| extension == "png")
                && !referenced_images.contains(&path)
            {
                match remove_if_present(&path) {
                    Ok(()) => report.removed_orphans += 1,
                    Err(_) => report.error_count += 1,
                }
            }
        }
        entries.sort_by_key(|entry| entry.created_unix_ms);
        let mut total = entries.iter().map(|entry| entry.bytes).sum::<u64>();
        for entry in entries {
            if total <= self.retention.max_bytes {
                break;
            }
            if remove_pair(&entry.document, &entry.image).is_err() {
                report.error_count += 1;
            } else {
                total = total.saturating_sub(entry.bytes);
                report.removed_entries += 1;
            }
        }
        report
    }
}

struct StoredEntry {
    document: PathBuf,
    image: PathBuf,
    created_unix_ms: u64,
    bytes: u64,
}

fn request_stem(request_id: RequestId) -> String {
    request_id.to_string()
}

fn write_png(path: &Path, crop: &RgbaFrame) -> Result<(), ScreenshotStoreError> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, crop.rect.width, crop.rect.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|error| ScreenshotStoreError::Png(error.to_string()))?;
    let packed = tightly_packed_pixels(crop)?;
    writer
        .write_image_data(&packed)
        .map_err(|error| ScreenshotStoreError::Png(error.to_string()))?;
    writer
        .finish()
        .map_err(|error| ScreenshotStoreError::Png(error.to_string()))?;
    Ok(())
}

#[allow(dead_code)] // Used by load_crop once the history surface is connected.
fn read_png(path: &Path, rect: PhysicalRect) -> Result<RgbaFrame, ScreenshotStoreError> {
    let decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    let mut reader = decoder
        .read_info()
        .map_err(|error| ScreenshotStoreError::Png(error.to_string()))?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| ScreenshotStoreError::Png("decoded image is too large".to_owned()))?;
    if size > rust_platform_windows::screen_capture::MAX_CAPTURE_BYTES {
        return Err(CaptureError::AllocationTooLarge {
            requested: size as u64,
            limit: rust_platform_windows::screen_capture::MAX_CAPTURE_BYTES,
        }
        .into());
    }
    let mut pixels = vec![0_u8; size];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|error| ScreenshotStoreError::Png(error.to_string()))?;
    if info.width != rect.width
        || info.height != rect.height
        || info.color_type != png::ColorType::Rgba
        || info.bit_depth != png::BitDepth::Eight
    {
        return Err(ScreenshotStoreError::DimensionMismatch);
    }
    pixels.truncate(info.buffer_size());
    RgbaFrame::try_new(rect, rect.width.saturating_mul(4), pixels).map_err(Into::into)
}

fn tightly_packed_pixels(crop: &RgbaFrame) -> Result<Vec<u8>, ScreenshotStoreError> {
    let row_bytes = crop
        .rect
        .width
        .checked_mul(4)
        .ok_or(CaptureError::StrideOverflow {
            width: crop.rect.width,
        })? as usize;
    if crop.stride as usize == row_bytes {
        return Ok(crop.pixels.clone());
    }
    let mut packed = Vec::with_capacity(row_bytes.saturating_mul(crop.rect.height as usize));
    for row in crop.pixels.chunks_exact(crop.stride as usize) {
        packed.extend_from_slice(row.get(..row_bytes).ok_or(CaptureError::StrideTooSmall {
            minimum: row_bytes as u32,
            actual: crop.stride,
        })?);
    }
    Ok(packed)
}

fn write_json(path: &Path, metadata: &ScreenshotMetadata) -> Result<(), ScreenshotStoreError> {
    let mut file = File::create(path)?;
    serde_json::to_writer(&mut file, metadata)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_pair(document: &Path, image: &Path) -> std::io::Result<()> {
    let document_result = remove_if_present(document);
    let image_result = remove_if_present(image);
    document_result.and(image_result)
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |metadata| metadata.len())
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use rust_platform_windows::screen_capture::RgbaFrame;
    use rust_protocol::{PhysicalRect, RequestId, ScreenBlockId, ScreenTextBlock, SourceContext};
    use tempfile::tempdir;

    use super::{ScreenshotMetadata, ScreenshotRetention, ScreenshotStore};

    fn frame(color: u8) -> RgbaFrame {
        RgbaFrame::try_new(
            PhysicalRect {
                x: -20,
                y: 10,
                width: 2,
                height: 1,
            },
            8,
            vec![color, 2, 3, 255, 4, 5, 6, 255],
        )
        .unwrap()
    }

    fn metadata(request_id: RequestId, created: u64, expires: u64) -> ScreenshotMetadata {
        ScreenshotMetadata {
            request_id,
            crop_x: -20,
            crop_y: 10,
            width: 2,
            height: 1,
            blocks: vec![ScreenTextBlock {
                block_id: ScreenBlockId::from("block-1"),
                source_text: "source".to_owned(),
                bounds: PhysicalRect {
                    x: -20,
                    y: 10,
                    width: 2,
                    height: 1,
                },
                confidence_milli: 900,
            }],
            translations: Vec::new(),
            context: SourceContext::default(),
            created_unix_ms: created,
            expires_unix_ms: expires,
            crop_filename: String::new(),
        }
    }

    #[test]
    fn selected_crop_and_metadata_are_written_atomically_and_load_back() {
        let directory = tempdir().unwrap();
        let store =
            ScreenshotStore::open_at(directory.path(), ScreenshotRetention::default()).unwrap();
        let request_id = RequestId::new();

        store
            .persist_crop(metadata(request_id, 100, 200), &frame(1))
            .unwrap();
        let loaded = store.load_crop(request_id).unwrap().unwrap();

        assert_eq!(loaded.metadata.request_id, request_id);
        assert_eq!(loaded.crop.pixels, frame(1).pixels);
        assert!(!fs::read_dir(directory.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|ext| ext == "tmp")
        }));
        let forbidden = ["Desktop", "Capture"].concat();
        assert!(!include_str!("screenshot_store.rs").contains(&forbidden));
    }

    #[test]
    fn default_retention_caps_expiry_at_twenty_four_hours() {
        let directory = tempdir().unwrap();
        let store =
            ScreenshotStore::open_at(directory.path(), ScreenshotRetention::default()).unwrap();
        let request_id = RequestId::new();
        let created = 10_000;

        store
            .persist_crop(metadata(request_id, created, u64::MAX), &frame(1))
            .unwrap();

        assert_eq!(
            store
                .load_crop(request_id)
                .unwrap()
                .unwrap()
                .metadata
                .expires_unix_ms,
            created + 24 * 60 * 60 * 1_000
        );
    }

    #[test]
    fn disabled_retention_writes_nothing() {
        let directory = tempdir().unwrap();
        let store = ScreenshotStore::open_at(
            directory.path(),
            ScreenshotRetention {
                enabled: false,
                ..ScreenshotRetention::default()
            },
        )
        .unwrap();

        assert!(
            store
                .persist_crop(metadata(RequestId::new(), 100, 200), &frame(1))
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn cleanup_expires_entries_and_removes_orphans_without_failing_the_caller() {
        let directory = tempdir().unwrap();
        let store =
            ScreenshotStore::open_at(directory.path(), ScreenshotRetention::default()).unwrap();
        let request_id = RequestId::new();
        store
            .persist_crop(metadata(request_id, 100, 150), &frame(1))
            .unwrap();
        fs::write(directory.path().join("orphan.png"), b"orphan").unwrap();
        fs::write(directory.path().join("interrupted.tmp"), b"temporary").unwrap();

        let report = store.cleanup(151);

        assert!(store.load_crop(request_id).unwrap().is_none());
        assert!(!directory.path().join("orphan.png").exists());
        assert!(!directory.path().join("interrupted.tmp").exists());
        assert_eq!(report.error_count, 0);
    }

    #[test]
    fn cleanup_failure_is_reported_without_becoming_a_control_flow_error() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("store");
        let store = ScreenshotStore::open_at(&root, ScreenshotRetention::default()).unwrap();
        fs::remove_dir(&root).unwrap();

        let report = store.cleanup(1_000);

        assert_eq!(report.error_count, 1);
    }

    #[test]
    fn size_limit_evicts_the_oldest_entry_first() {
        let directory = tempdir().unwrap();
        let store = ScreenshotStore::open_at(
            directory.path(),
            ScreenshotRetention {
                enabled: true,
                ttl: Duration::from_secs(86_400),
                max_bytes: u64::MAX,
            },
        )
        .unwrap();
        let oldest = RequestId::new();
        let newest = RequestId::new();
        store
            .persist_crop(metadata(oldest, 100, 10_000), &frame(1))
            .unwrap();
        store
            .persist_crop(metadata(newest, 200, 10_000), &frame(2))
            .unwrap();
        let newest_stem = super::request_stem(newest);
        let newest_bytes = fs::metadata(directory.path().join(format!("{newest_stem}.json")))
            .unwrap()
            .len()
            + fs::metadata(directory.path().join(format!("{newest_stem}.png")))
                .unwrap()
                .len();
        let limited = ScreenshotStore::open_at(
            directory.path(),
            ScreenshotRetention {
                enabled: true,
                ttl: Duration::from_secs(86_400),
                max_bytes: newest_bytes,
            },
        )
        .unwrap();

        limited.cleanup(300);

        assert!(limited.load_crop(oldest).unwrap().is_none());
        assert!(limited.load_crop(newest).unwrap().is_some());
    }

    #[test]
    fn delete_now_and_permanent_save_are_independent() {
        let directory = tempdir().unwrap();
        let permanent = directory.path().join("saved.png");
        let store = ScreenshotStore::open_at(
            directory.path().join("store"),
            ScreenshotRetention::default(),
        )
        .unwrap();
        let request_id = RequestId::new();
        store
            .persist_crop(metadata(request_id, 100, 200), &frame(9))
            .unwrap();

        store.save_permanent(request_id, &permanent).unwrap();
        store.delete_now(request_id).unwrap();

        assert!(permanent.exists());
        assert!(store.load_crop(request_id).unwrap().is_none());
    }

    #[test]
    fn delete_now_prevents_a_late_background_write_from_recreating_the_crop() {
        let directory = tempdir().unwrap();
        let store =
            ScreenshotStore::open_at(directory.path(), ScreenshotRetention::default()).unwrap();
        let request_id = RequestId::new();

        store.delete_now(request_id).unwrap();
        assert!(
            store
                .persist_crop(metadata(request_id, 100, 200), &frame(9))
                .unwrap()
                .is_none()
        );
        assert!(store.load_crop(request_id).unwrap().is_none());
    }
}
