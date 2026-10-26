//! モデルのフォルダ（[`ModelLibrary`]）: 台帳（`*.genzo-model.json`）を読み、種類ごとに使うモデルを選び、
//! セッションを使い回す。

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use genzo_model::AiMaskKind;

use crate::backend::{MlBackend, MlError, MlSession, ModelSpec, file_sha256};
use crate::image::Rgb8Image;
use crate::manifest::{MANIFEST_EXTENSION, ModelManifest};
use crate::segment::{Segmentation, Segmenter, SegmenterStatus, segment_with};

/// フォルダの中の 1 つのモデル（台帳と、読めなかった理由）。
#[derive(Debug, Clone)]
pub struct ModelEntry {
    /// 台帳のファイル。
    pub manifest_path: PathBuf,
    /// 台帳（読めたとき）。
    pub manifest: Option<ModelManifest>,
    /// モデルのファイル（台帳から）。
    pub model_path: Option<PathBuf>,
    /// 読めなかった・使えない理由（使えるなら `None`）。
    pub problem: Option<String>,
}

impl ModelEntry {
    /// 使えるか。
    pub fn usable(&self) -> bool {
        self.problem.is_none() && self.manifest.is_some()
    }
}

/// モデルのフォルダを読めない。
#[derive(Debug, thiserror::Error)]
#[error("モデルのフォルダ {dir} を読めません: {source}")]
pub struct ModelLoadError {
    /// フォルダ。
    pub dir: PathBuf,
    /// 理由。
    #[source]
    pub source: std::io::Error,
}

/// モデルのフォルダ（モジュールの doc）。[`Segmenter`] を実装する。
pub struct ModelLibrary {
    dir: PathBuf,
    backend: Arc<dyn MlBackend>,
    entries: Vec<ModelEntry>,
    /// 読み込んだセッション（モデルの ID → セッション）。推論は 1 つずつ（ロックの中で実行する）。
    sessions: Mutex<HashMap<String, Box<dyn MlSession>>>,
}

impl fmt::Debug for ModelLibrary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelLibrary")
            .field("dir", &self.dir)
            .field("backend", &self.backend)
            .field("entries", &self.entries.len())
            .finish()
    }
}

impl ModelLibrary {
    /// フォルダ `dir` の台帳を読む（フォルダがなければ空。読めない台帳は `problem` 付きの項目にする）。モデルの
    /// ファイルの存在と SHA-256（台帳に書いてあれば）をここで確かめる。
    pub fn scan(dir: &Path, backend: Arc<dyn MlBackend>) -> Result<Self, ModelLoadError> {
        let mut entries = Vec::new();
        if dir.is_dir() {
            let read = std::fs::read_dir(dir).map_err(|source| ModelLoadError {
                dir: dir.to_path_buf(),
                source,
            })?;
            let mut paths: Vec<PathBuf> = read
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(MANIFEST_EXTENSION))
                })
                .collect();
            paths.sort();
            for p in paths {
                entries.push(read_entry(&p));
            }
        }
        tracing::info!(
            dir = %dir.display(),
            models = entries.iter().filter(|e| e.usable()).count(),
            problems = entries.iter().filter(|e| !e.usable()).count(),
            "AI マスクのモデルのフォルダを読んだ"
        );
        Ok(Self {
            dir: dir.to_path_buf(),
            backend,
            entries,
            sessions: Mutex::new(HashMap::new()),
        })
    }

    /// フォルダ。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 項目（台帳のファイル名の順）。
    pub fn entries(&self) -> &[ModelEntry] {
        &self.entries
    }

    /// 種類 `kind` に使うモデル（使える台帳のうち、ファイル名の順で最初のもの）。
    pub fn model_for(&self, kind: AiMaskKind) -> Option<(&ModelEntry, &ModelManifest)> {
        self.entries.iter().find_map(|e| {
            let m = e.manifest.as_ref()?;
            (e.usable() && m.supports(kind)).then_some((e, m))
        })
    }

    /// 読み込んだセッションを捨てる（メモリを返す）。
    pub fn unload(&self) {
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

fn read_entry(manifest_path: &Path) -> ModelEntry {
    let mut entry = ModelEntry {
        manifest_path: manifest_path.to_path_buf(),
        manifest: None,
        model_path: None,
        problem: None,
    };
    let manifest = match ModelManifest::read(manifest_path) {
        Ok(m) => m,
        Err(e) => {
            entry.problem = Some(format!("台帳を読めません: {e}"));
            return entry;
        }
    };
    let model_path = manifest.model_path(manifest_path);
    entry.model_path = Some(model_path.clone());
    if !model_path.is_file() {
        entry.problem = Some(format!(
            "モデルのファイル {} がありません",
            model_path.display()
        ));
    } else if !manifest.sha256.is_empty() {
        match file_sha256(&model_path) {
            Ok(actual) if actual == manifest.sha256 => {}
            Ok(actual) => {
                entry.problem = Some(format!(
                    "モデルのファイルの SHA-256 が台帳と違います（台帳 {}… / ファイル {}…）",
                    &manifest.sha256[..12],
                    &actual[..12]
                ));
            }
            Err(e) => entry.problem = Some(format!("モデルのファイルを読めません: {e}")),
        }
    }
    entry.manifest = Some(manifest);
    entry
}

impl Segmenter for ModelLibrary {
    fn status(&self) -> SegmenterStatus {
        let mut status = SegmenterStatus {
            backend: self.backend.name(),
            ..Default::default()
        };
        for kind in AiMaskKind::ALL {
            if let Some((_, m)) = self.model_for(kind) {
                status.kinds.push(kind);
                status.model_ids.push(m.id.clone());
            }
        }
        for e in &self.entries {
            if let Some(p) = &e.problem {
                status.problems.push(format!(
                    "{}: {p}",
                    e.manifest_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                ));
            }
        }
        if status.kinds.is_empty() {
            status.problems.push(format!(
                "使えるモデルがありません（{} に台帳 *{MANIFEST_EXTENSION} とモデルのファイルを置いてください）",
                self.dir.display()
            ));
        }
        status
    }

    fn segment(&self, image: &Rgb8Image, kind: AiMaskKind) -> Result<Segmentation, MlError> {
        let (entry, manifest) = self
            .model_for(kind)
            .ok_or(MlError::NoModel(kind.display_name()))?;
        let model_path = entry.model_path.clone().expect("使える項目にはある");
        let mut sessions = self.sessions.lock().unwrap_or_else(PoisonError::into_inner);
        if !sessions.contains_key(&manifest.id) {
            let spec = ModelSpec {
                path: model_path,
                threads: manifest.threads,
            };
            let started = std::time::Instant::now();
            let session = self.backend.load(&spec)?;
            tracing::info!(
                model = %manifest.id,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "AI マスクのモデルを読み込んだ"
            );
            sessions.insert(manifest.id.clone(), session);
        }
        let session = sessions.get_mut(&manifest.id).expect("入れた");
        let result = segment_with(manifest, session.as_mut(), image, kind);
        if let Err(e) = &result {
            // 失敗したセッションは捨てる（次は読み込み直す）。
            tracing::warn!(model = %manifest.id, error = %e, "AI マスクの推論に失敗した");
            sessions.remove(&manifest.id);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::ThresholdBackend;

    fn write(dir: &Path, name: &str, json: &str) {
        std::fs::write(dir.join(name), json).unwrap();
    }

    #[test]
    fn scans_manifests_and_reports_problems() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.onnx"), b"model-a").unwrap();
        let sha = genzo_model::sha256_hex(b"model-a");
        write(
            dir.path(),
            "a.genzo-model.json",
            &format!(
                r#"{{"id": "a", "file": "a.onnx", "sha256": "{sha}", "input": {{"width": 4, "height": 4}},
                    "kinds": {{"sky": {{"channel": 0}}}}}}"#
            ),
        );
        write(
            dir.path(),
            "b.genzo-model.json",
            r#"{"id": "b", "file": "missing.onnx", "input": {"width": 4, "height": 4}, "kinds": {"person": {"channel": 0}}}"#,
        );
        write(
            dir.path(),
            "c.genzo-model.json",
            &format!(
                r#"{{"id": "c", "file": "a.onnx", "sha256": "{}", "input": {{"width": 4, "height": 4}}, "kinds": {{"subject": {{"channel": 0}}}}}}"#,
                "00".repeat(32)
            ),
        );
        write(dir.path(), "d.genzo-model.json", "{");
        write(dir.path(), "not-a-manifest.json", "{}");
        let lib = ModelLibrary::scan(dir.path(), Arc::new(ThresholdBackend::default())).unwrap();
        assert_eq!(lib.entries().len(), 4);
        assert!(lib.entries()[0].usable());
        assert!(
            lib.entries()[1]
                .problem
                .as_deref()
                .unwrap()
                .contains("ありません")
        );
        assert!(
            lib.entries()[2]
                .problem
                .as_deref()
                .unwrap()
                .contains("SHA-256")
        );
        assert!(
            lib.entries()[3]
                .problem
                .as_deref()
                .unwrap()
                .contains("台帳")
        );
        assert_eq!(lib.model_for(AiMaskKind::Sky).unwrap().1.id, "a");
        assert!(lib.model_for(AiMaskKind::Person).is_none());
        let status = lib.status();
        assert_eq!(status.kinds, [AiMaskKind::Sky]);
        assert_eq!(status.model_ids, ["a"]);
        assert_eq!(status.problems.len(), 3);
        assert!(status.backend.starts_with("threshold"));
        // フォルダがなければ空で、使えるモデルがない旨。
        let none = ModelLibrary::scan(
            &dir.path().join("nothing"),
            Arc::new(ThresholdBackend::default()),
        )
        .unwrap();
        assert!(none.entries().is_empty());
        assert_eq!(none.status().problems.len(), 1);
    }

    #[test]
    fn segments_with_the_backend_and_reuses_the_session() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.onnx"), b"model-a").unwrap();
        write(
            dir.path(),
            "a.genzo-model.json",
            r#"{"id": "a", "file": "a.onnx", "input": {"width": 4, "height": 2, "range": "byte"},
                "output": {"activation": "none"}, "kinds": {"sky": {"channel": 0}}}"#,
        );
        let backend = Arc::new(ThresholdBackend::new(100.0));
        let lib = ModelLibrary::scan(dir.path(), backend.clone()).unwrap();
        // 左半分が暗く、右半分が明るい 8 × 4 の画像。
        let mut data = Vec::new();
        for _ in 0..4 {
            for x in 0..8u8 {
                let v = if x < 4 { 20 } else { 220 };
                data.extend([v, v, v]);
            }
        }
        let img = Rgb8Image::new(8, 4, data).unwrap();
        let r = lib.segment(&img, AiMaskKind::Sky).unwrap();
        assert_eq!((r.width, r.height), (8, 4));
        assert_eq!(r.values[0], 0);
        assert_eq!(r.values[7], 255);
        assert_eq!(r.model_id, "a");
        assert_eq!(backend.loads(), 1);
        lib.segment(&img, AiMaskKind::Sky).unwrap();
        assert_eq!(backend.loads(), 1);
        lib.unload();
        lib.segment(&img, AiMaskKind::Sky).unwrap();
        assert_eq!(backend.loads(), 2);
        assert!(matches!(
            lib.segment(&img, AiMaskKind::Person),
            Err(MlError::NoModel(_))
        ));
    }
}
