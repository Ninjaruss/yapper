//! ModelManager: first-run model downloads (STT + LLM) with progress events.
//!
//! Zero-setup principle: one progress bar, resilient errors, no accounts.
//! `ensure_model` is BLOCKING (uses `reqwest::blocking` + synchronous tar/bzip2
//! unpacking for archive models) — the caller (Task 8) must wrap it in
//! `spawn_blocking` / `std::thread::spawn` so it doesn't stall Tauri's async
//! runtime.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use tauri::{Emitter, Manager};

use crate::error::YapperError;
use crate::stt::moonshine::MODEL_FILES;

/// How a model's downloaded artifact is turned into on-disk files under its
/// model dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// A `.tar.bz2` archive whose single top-level directory component is
    /// stripped on unpack (the sherpa-onnx release archive shape).
    TarBz2StripTop,
    /// A single file downloaded directly to `<dir>/<files[0]>`, no archive
    /// involved. The download follows redirects (reqwest's default policy),
    /// which matters for hosts like Hugging Face that redirect through a CDN.
    SingleFile,
}

/// Everything ModelManager needs to know to fetch and verify one model.
pub struct ModelSpec {
    /// Subdirectory name under `<app_data_dir>/models/` this model lives in.
    pub dir_name: &'static str,
    /// Download URL (redirects are followed).
    pub url: &'static str,
    /// Files that must exist (non-empty) directly under the model dir for it
    /// to count as present. For `ArchiveKind::SingleFile`, `files[0]` also
    /// names the downloaded file.
    pub files: &'static [&'static str],
    pub kind: ArchiveKind,
}

/// Moonshine STT model — 250,807,309-byte tar.bz2 release archive; single
/// top-level dir `sherpa-onnx-moonshine-base-en-int8/` to strip on unpack.
/// Pinned by the Plan 2 Task 1 spike.
pub const STT_MODEL: ModelSpec = ModelSpec {
    dir_name: "moonshine-base-en-int8",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-moonshine-base-en-int8.tar.bz2",
    files: &MODEL_FILES,
    kind: ArchiveKind::TarBz2StripTop,
};

/// Qwen2.5-3B-Instruct GGUF, `q4_k_m` quant — 2,104,932,768-byte single-file
/// download (no archive). Pinned by the Plan 4 Task 1 spike; the HF URL
/// redirects through their Xet CDN, so the download must follow redirects.
pub const LLM_MODEL: ModelSpec = ModelSpec {
    dir_name: "llm",
    url: "https://huggingface.co/Qwen/Qwen2.5-3B-Instruct-GGUF/resolve/main/qwen2.5-3b-instruct-q4_k_m.gguf",
    files: &["model.gguf"],
    kind: ArchiveKind::SingleFile,
};

/// True only if every file in `files` exists directly under `dir` and is
/// non-empty. A partial set (e.g. an interrupted previous download) counts
/// as missing so `ensure_model` re-downloads rather than trusting it. Pure
/// and dir-agnostic so it can be driven by any `ModelSpec`'s `files`.
pub fn files_present(dir: &Path, files: &[&str]) -> bool {
    files.iter().all(|f| {
        let path = dir.join(f);
        path.metadata()
            .map(|m| m.is_file() && m.len() > 0)
            .unwrap_or(false)
    })
}

/// Where a model's files live for this install:
/// `<app_data_dir>/models/<spec.dir_name>`.
pub fn model_dir_for(app: &tauri::AppHandle, spec: &ModelSpec) -> Result<PathBuf, YapperError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| YapperError::Audio(format!("model dir: {e}")))?;
    Ok(data_dir.join("models").join(spec.dir_name))
}

/// True if `spec`'s files are already fully downloaded for this install.
/// Never touches the network — safe to call from a synchronous command.
pub fn model_present(app: &tauri::AppHandle, spec: &ModelSpec) -> bool {
    model_dir_for(app, spec)
        .map(|dir| files_present(&dir, spec.files))
        .unwrap_or(false)
}

/// Ensure `spec`'s model files are present, downloading (and unpacking, for
/// archive models) if not. Short-circuits immediately if a complete set
/// already exists. Emits `model:progress` (`{model, downloaded, total}`
/// bytes; `model` is `spec.dir_name`; `total` is 0 if the server didn't
/// report a Content-Length) during download, and `model:ready` (payload
/// `spec.dir_name`) once the files are verified complete.
///
/// BLOCKING — do not call from an async context without `spawn_blocking`.
pub fn ensure_model(app: &tauri::AppHandle, spec: &ModelSpec) -> Result<PathBuf, YapperError> {
    let dir = model_dir_for(app, spec)?;
    if files_present(&dir, spec.files) {
        return Ok(dir);
    }

    match spec.kind {
        ArchiveKind::TarBz2StripTop => ensure_archive_model(app, spec, &dir),
        ArchiveKind::SingleFile => ensure_single_file_model(app, spec, &dir),
    }
}

fn emit_progress(app: &tauri::AppHandle, model: &'static str, downloaded: u64, total: u64) {
    let _ = app.emit(
        "model:progress",
        serde_json::json!({"model": model, "downloaded": downloaded, "total": total}),
    );
}

fn ensure_archive_model(
    app: &tauri::AppHandle,
    spec: &ModelSpec,
    dir: &Path,
) -> Result<PathBuf, YapperError> {
    let parent = dir.parent().ok_or_else(|| {
        YapperError::Audio("model download failed: model dir has no parent".into())
    })?;
    std::fs::create_dir_all(parent).map_err(|e| {
        YapperError::Audio(format!(
            "model download failed: could not create model dir: {e}"
        ))
    })?;
    let part_path = {
        let mut p = dir.to_path_buf();
        p.set_extension("part");
        p
    };

    // A download or unpack failure no longer deletes the .part file (that
    // was the pre-resume behavior) — it's left in place so the next attempt
    // can resume the download (via download_to_file's Range support) instead
    // of starting over from zero. The .part is only ever removed below, once
    // we know the outcome: on full success it's no longer needed (already
    // unpacked into `dir`), and on a post-unpack verification failure it's
    // discarded too, since an archive that unpacked without error but still
    // left expected files missing is unlikely to unpack any better on a
    // second try — better to force a fresh download next time.
    let result = (|| -> Result<(), YapperError> {
        let emit_app = app.clone();
        let model_name = spec.dir_name;
        download_to_file(spec.url, &part_path, |downloaded, total| {
            emit_progress(&emit_app, model_name, downloaded, total);
        })?;
        unpack_archive(&part_path, dir)?;
        Ok(())
    })();
    result?;

    if !files_present(dir, spec.files) {
        let _ = std::fs::remove_file(&part_path);
        return Err(YapperError::Audio(
            "model download failed: model archive missing expected files".into(),
        ));
    }
    let _ = std::fs::remove_file(&part_path);

    let _ = app.emit("model:ready", spec.dir_name);
    Ok(dir.to_path_buf())
}

fn ensure_single_file_model(
    app: &tauri::AppHandle,
    spec: &ModelSpec,
    dir: &Path,
) -> Result<PathBuf, YapperError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        YapperError::Audio(format!(
            "model download failed: could not create model dir: {e}"
        ))
    })?;
    let file_name = spec.files.first().ok_or_else(|| {
        YapperError::Audio("model spec has no files for a SingleFile model".into())
    })?;
    let dest = dir.join(file_name);
    let part_path = dir.join(format!("{file_name}.part"));

    // Same resume-friendly discipline as the archive path: a download
    // failure leaves the .part in place (download_to_file resumes it next
    // time via Range) instead of deleting it. On success, `rename` already
    // consumes the .part by moving it to `dest`, so there's nothing left to
    // clean up here — no unconditional delete needed (or wanted, since that
    // would wipe a legitimately partial file on failure).
    let result = (|| -> Result<(), YapperError> {
        let emit_app = app.clone();
        let model_name = spec.dir_name;
        download_to_file(spec.url, &part_path, |downloaded, total| {
            emit_progress(&emit_app, model_name, downloaded, total);
        })?;
        std::fs::rename(&part_path, &dest).map_err(|e| {
            YapperError::Audio(format!(
                "model download failed: could not finalize downloaded file: {e}"
            ))
        })?;
        Ok(())
    })();
    result?;

    if !files_present(dir, spec.files) {
        return Err(YapperError::Audio(
            "model download failed: downloaded file missing or empty".into(),
        ));
    }

    let _ = app.emit("model:ready", spec.dir_name);
    Ok(dir.to_path_buf())
}

/// What `download_to_file` should do with an existing `.part` file once it
/// has a response status in hand for a ranged request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumeAction {
    /// Server honored the `Range` request (206 Partial Content) — keep the
    /// existing bytes and stream the response onto the end of the file.
    Append,
    /// Server did not honor the `Range` request, most commonly 200 OK (some
    /// servers/CDNs ignore `Range` and just send the whole body again) —
    /// discard whatever bytes exist and start over from zero.
    Restart,
}

/// Decide how to continue a partially-downloaded file given the HTTP status
/// returned for a ranged GET.
///
/// Precondition: only meaningful when the caller already knows
/// `existing_len > 0` and sent a `Range: bytes=<existing_len>-` header for
/// this request — this function doesn't decide *whether* to resume, only
/// what to do given a response status once resuming was attempted. A status
/// outside `{200, 206}` (e.g. 416 Range Not Satisfiable, or any error status)
/// is not a resumable outcome at all and is the caller's job to reject
/// before reaching here — this function treats anything that isn't 206 as
/// "restart", so callers must gate on success statuses first.
pub(crate) fn resume_decision(existing_len: u64, status: u16) -> ResumeAction {
    let _ = existing_len; // documents the precondition above; unused otherwise
    match status {
        206 => ResumeAction::Append,
        _ => ResumeAction::Restart,
    }
}

/// Stream `url` to `dest`, calling `on_progress(downloaded, total)` roughly
/// every ≥1 MB (plus once at the end). `total` is 0 if the server didn't
/// report a Content-Length. Redirects are followed (reqwest's default blocking
/// client policy). Decoupled from `tauri::AppHandle` so this function (and
/// thus the real network path) can be exercised directly in a test without a
/// running Tauri app.
///
/// Resume: if `dest` already exists with non-zero length (a `.part` left
/// over from an interrupted previous attempt), this sends
/// `Range: bytes=<existing_len>-` instead of a plain GET. A 206 response
/// appends onto the existing file, with progress starting at `existing_len`.
/// A 200 (or anything else) means the server didn't honor the range, so the
/// file is truncated and the download restarts from zero. Any other status
/// on the ranged request is an error. With no existing `.part` (or a
/// zero-length one), this is a plain GET — the pre-resume behavior.
fn download_to_file(
    url: &str,
    dest: &Path,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<(), YapperError> {
    let existing_len = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);

    let (response, mut file, mut downloaded) = if existing_len > 0 {
        let client = reqwest::blocking::Client::new();
        let response = client
            .get(url)
            .header(reqwest::header::RANGE, format!("bytes={existing_len}-"))
            .send()
            .map_err(|e| {
                YapperError::Audio(format!("model download failed: request error: {e}"))
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(YapperError::Audio(format!(
                "model download failed: server returned an error resuming download: {status}"
            )));
        }
        match resume_decision(existing_len, status.as_u16()) {
            ResumeAction::Append => {
                let file = std::fs::OpenOptions::new().append(true).open(dest).map_err(|e| {
                    YapperError::Audio(format!(
                        "model download failed: could not reopen file to resume: {e}"
                    ))
                })?;
                (response, file, existing_len)
            }
            ResumeAction::Restart => {
                let file = std::fs::File::create(dest).map_err(|e| {
                    YapperError::Audio(format!(
                        "model download failed: could not create file: {e}"
                    ))
                })?;
                (response, file, 0)
            }
        }
    } else {
        let response = reqwest::blocking::get(url).map_err(|e| {
            YapperError::Audio(format!("model download failed: request error: {e}"))
        })?;
        let response = response.error_for_status().map_err(|e| {
            YapperError::Audio(format!(
                "model download failed: server returned an error: {e}"
            ))
        })?;
        let file = std::fs::File::create(dest).map_err(|e| {
            YapperError::Audio(format!("model download failed: could not create file: {e}"))
        })?;
        (response, file, 0)
    };

    // On a fresh download, content_length() is the whole file. On a resumed
    // (206) response it's just the remaining bytes, so it's added onto
    // `downloaded` (which starts at existing_len in that case) to get the
    // true total. `None` (no Content-Length header) stays the existing
    // "0 means unknown" convention.
    let total = response
        .content_length()
        .map(|remaining| downloaded + remaining)
        .unwrap_or(0);

    let mut reader = response;
    let mut buf = [0u8; 64 * 1024];
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

/// True iff every component of `p` is a plain path segment (`Component::Normal`)
/// — no `..`, no root, no prefix. Used to reject archive entries that would
/// otherwise escape `dest_dir` via `..` traversal once joined onto it.
fn is_safe_relative(p: &Path) -> bool {
    p.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Unpack a `.tar.bz2` archive into `dest_dir`, stripping the single
/// top-level directory component the sherpa-onnx release archives use.
fn unpack_archive(archive_path: &Path, dest_dir: &Path) -> Result<(), YapperError> {
    std::fs::create_dir_all(dest_dir).map_err(|e| {
        YapperError::Audio(format!(
            "model unpack failed: could not create dest dir: {e}"
        ))
    })?;
    let file = std::fs::File::open(archive_path).map_err(|e| {
        YapperError::Audio(format!("model unpack failed: could not open archive: {e}"))
    })?;
    let decoder = bzip2::read::BzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(|e| {
        YapperError::Audio(format!(
            "model unpack failed: could not read archive entries: {e}"
        ))
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
        if !is_safe_relative(&stripped) {
            return Err(YapperError::Audio(format!(
                "model unpack failed: unsafe path in archive: {}",
                path.display()
            )));
        }
        let dest = dest_dir.join(&stripped);
        entry.unpack(&dest).map_err(|e| {
            YapperError::Audio(format!(
                "model unpack failed: could not extract {stripped:?}: {e}"
            ))
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
        assert!(!files_present(&root, STT_MODEL.files));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("tokens.txt"), "x").unwrap();
        assert!(
            !files_present(&root, STT_MODEL.files),
            "partial set must count as missing"
        );
        for f in STT_MODEL.files {
            std::fs::write(root.join(f), "x").unwrap();
        }
        assert!(files_present(&root, STT_MODEL.files));
    }

    /// The SingleFile model (LLM) has a one-entry `files` list, so
    /// `files_present` degenerates to a single existence+non-empty check —
    /// confirms the generalized function handles that shape correctly, not
    /// just the multi-file archive shape above. This is also the short-circuit
    /// check `ensure_model` relies on before ever touching the network: a
    /// complete `model.gguf` on disk must be recognized without a request.
    #[test]
    fn single_file_model_files_present_checks_the_one_named_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!files_present(dir.path(), LLM_MODEL.files));
        std::fs::write(dir.path().join(LLM_MODEL.files[0]), "x").unwrap();
        assert!(files_present(dir.path(), LLM_MODEL.files));
        // An empty file (e.g. `File::create` then a crash before any bytes
        // land) must still count as missing, same as the archive model.
        std::fs::write(dir.path().join(LLM_MODEL.files[0]), "").unwrap();
        assert!(!files_present(dir.path(), LLM_MODEL.files));
    }

    #[test]
    fn stt_and_llm_specs_are_distinct_and_correctly_kinded() {
        assert_eq!(STT_MODEL.dir_name, "moonshine-base-en-int8");
        assert_eq!(STT_MODEL.kind, ArchiveKind::TarBz2StripTop);
        assert_eq!(LLM_MODEL.dir_name, "llm");
        assert_eq!(LLM_MODEL.files, &["model.gguf"]);
        assert_eq!(LLM_MODEL.kind, ArchiveKind::SingleFile);
        assert_ne!(STT_MODEL.dir_name, LLM_MODEL.dir_name);
    }

    #[test]
    fn resume_decision_206_appends() {
        assert_eq!(resume_decision(12_345, 206), ResumeAction::Append);
    }

    #[test]
    fn resume_decision_200_restarts() {
        // Server ignored the Range request and is sending the full body
        // again — the existing partial bytes are stale, so restart.
        assert_eq!(resume_decision(12_345, 200), ResumeAction::Restart);
    }

    #[test]
    fn is_safe_relative_rejects_traversal_and_accepts_normal_paths() {
        assert!(!is_safe_relative(Path::new("../../evil.txt")));
        assert!(!is_safe_relative(Path::new("a/../../evil.txt")));
        assert!(!is_safe_relative(Path::new("/etc/passwd")));
        assert!(is_safe_relative(Path::new("tokens.txt")));
        assert!(is_safe_relative(Path::new("sub/dir/file.onnx")));
    }
}
