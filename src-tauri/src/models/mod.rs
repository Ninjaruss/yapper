//! ModelManager: first-run Moonshine model download with progress events.
//!
//! Zero-setup principle: one progress bar, resilient errors, no accounts.
//! `ensure_models` is BLOCKING (uses `reqwest::blocking` + synchronous tar/bzip2
//! unpacking) — the caller (Task 8) must wrap it in `spawn_blocking` /
//! `std::thread::spawn` so it doesn't stall Tauri's async runtime.

use std::io::Read;
use std::path::{Path, PathBuf};

use tauri::{Emitter, Manager};

use crate::error::YapperError;
use crate::stt::moonshine::MODEL_FILES;

/// Release archive pinned by the Task 1 spike (250,807,309 bytes; single
/// top-level dir `sherpa-onnx-moonshine-base-en-int8/` to strip on unpack).
pub const MODEL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-base-en-int8.tar.bz2";

/// True only if every file in [`MODEL_FILES`] exists directly under `dir`
/// and is non-empty. A partial set (e.g. an interrupted previous download)
/// counts as missing so `ensure_models` re-downloads rather than trusting it.
pub fn models_present(dir: &Path) -> bool {
    MODEL_FILES.iter().all(|f| {
        let path = dir.join(f);
        path.metadata().map(|m| m.is_file() && m.len() > 0).unwrap_or(false)
    })
}

/// Where the Moonshine model files live for this install:
/// `<app_data_dir>/models/moonshine-base-en-int8`.
pub fn model_dir(app: &tauri::AppHandle) -> Result<PathBuf, YapperError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| YapperError::Audio(format!("model dir: {e}")))?;
    Ok(data_dir.join("models").join("moonshine-base-en-int8"))
}

/// Ensure the Moonshine model files are present, downloading + unpacking the
/// pinned release archive if not. Short-circuits immediately if a complete
/// set already exists (e.g. from a previous run). Emits `model:progress`
/// ({downloaded, total} bytes; `total` is 0 if the server didn't report a
/// Content-Length) during download, and `model:ready` (payload `true`) once
/// the extracted files are verified complete.
///
/// BLOCKING — do not call from an async context without `spawn_blocking`.
pub fn ensure_models(app: &tauri::AppHandle) -> Result<PathBuf, YapperError> {
    let dir = model_dir(app)?;
    if models_present(&dir) {
        return Ok(dir);
    }

    let parent = dir.parent().ok_or_else(|| {
        YapperError::Audio("model download failed: model dir has no parent".into())
    })?;
    std::fs::create_dir_all(parent)?;
    let part_path = {
        let mut p = dir.clone();
        p.set_extension("part");
        p
    };

    let emit_app = app.clone();
    download_to_file(MODEL_URL, &part_path, |downloaded, total| {
        let _ = emit_app.emit(
            "model:progress",
            serde_json::json!({"downloaded": downloaded, "total": total}),
        );
    })?;
    unpack_archive(&part_path, &dir)?;
    let _ = std::fs::remove_file(&part_path);

    if !models_present(&dir) {
        return Err(YapperError::Audio(
            "model download failed: model archive missing expected files".into(),
        ));
    }

    let _ = app.emit("model:ready", true);
    Ok(dir)
}

/// Stream `url` to `dest`, calling `on_progress(downloaded, total)` roughly
/// every ≥1 MB (plus once at the end). `total` is 0 if the server didn't
/// report a Content-Length. Decoupled from `tauri::AppHandle` so this
/// function (and thus the real network path) can be exercised directly in a
/// test without a running Tauri app.
fn download_to_file(
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), YapperError> {
    let response = reqwest::blocking::get(url)
        .map_err(|e| YapperError::Audio(format!("model download failed: request error: {e}")))?;
    let response = response.error_for_status().map_err(|e| {
        YapperError::Audio(format!("model download failed: server returned an error: {e}"))
    })?;
    let total = response.content_length().unwrap_or(0);

    let mut file = std::fs::File::create(dest).map_err(|e| {
        YapperError::Audio(format!("model download failed: could not create file: {e}"))
    })?;

    let mut reader = response;
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut since_last_emit: u64 = 0;
    const EMIT_THRESHOLD: u64 = 1024 * 1024;

    loop {
        let n = reader.read(&mut buf).map_err(|e| {
            YapperError::Audio(format!("model download failed: connection error: {e}"))
        })?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| {
            YapperError::Audio(format!("model download failed: could not write file: {e}"))
        })?;
        downloaded += n as u64;
        since_last_emit += n as u64;
        if since_last_emit >= EMIT_THRESHOLD {
            since_last_emit = 0;
            on_progress(downloaded, total);
        }
    }
    on_progress(downloaded, total);
    Ok(())
}

/// Unpack a `.tar.bz2` archive into `dest_dir`, stripping the single
/// top-level directory component the sherpa-onnx release archives use.
fn unpack_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), YapperError> {
    std::fs::create_dir_all(dest_dir)?;
    let file = std::fs::File::open(archive_path).map_err(|e| {
        YapperError::Audio(format!("model unpack failed: could not open archive: {e}"))
    })?;
    let decoder = bzip2::read::BzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|e| {
        YapperError::Audio(format!("model unpack failed: could not read archive entries: {e}"))
    })?;

    for entry in entries {
        let mut entry = entry.map_err(|e| {
            YapperError::Audio(format!("model unpack failed: bad archive entry: {e}"))
        })?;
        let path = entry
            .path()
            .map_err(|e| YapperError::Audio(format!("model unpack failed: bad entry path: {e}")))?
            .into_owned();
        // Strip the single top-level dir (e.g. sherpa-onnx-moonshine-base-en-int8/).
        let stripped: PathBuf = path.components().skip(1).collect();
        if stripped.as_os_str().is_empty() {
            continue; // the top-level dir entry itself
        }
        let dest = dest_dir.join(&stripped);
        entry.unpack(&dest).map_err(|e| {
            YapperError::Audio(format!("model unpack failed: could not extract {stripped:?}: {e}"))
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_missing_when_dir_absent_or_incomplete() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("moonshine");
        assert!(!models_present(&root));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("tokens.txt"), "x").unwrap();
        assert!(!models_present(&root), "partial set must count as missing");
        for f in crate::stt::moonshine::MODEL_FILES {
            std::fs::write(root.join(f), "x").unwrap();
        }
        assert!(models_present(&root));
    }
}
