//! カメラプロファイル・ルックのライブラリ（DEV-17。docs/04_architecture.md の 2.1 節・2.5 節）。
//!
//! 利用者が用意したカメラプロファイル（DCP）と 3D LUT（.cube）を、カタログのデータのフォルダの `profiles`
//! （[`crate::CoreConfig::data_dir`]。CLI とアプリで同じ）に取り込み、現像設定の ID（`profile.camera`・`profile.look`）から
//! 引けるようにする（genzo-pipeline の `ProfileResolver`。エンジンに渡す）。**Adobe のプロファイルなどは同梱しない**。
//! ネットワークには何も送らない。
//!
//! # フォルダの中身
//!
//! - `index.json`: 取り込んだものの一覧（種類・ID・表示の名前・保存したファイルの名前・内容の SHA-256・大きさ・元の
//!   ファイルの名前・取り込んだ日時・DCP の機種）。一時ファイルに書いてから名前を変えて置き換える。
//! - `<SHA-256>.dcp`・`<SHA-256>.cube`: 取り込んだファイルの写し（内容で名前を決めるので、同じ内容は 1 つ）。
//!
//! # 取り込み（[`ProfileLibrary::import`]）
//!
//! 1. 拡張子で種類を決める（`.dcp`・`.cube`。大文字・小文字は区別しない）。大きさの上限を超える・解析できないファイルは
//!    取り込まない（genzo-color の `dcp`・`cube`。ファイルごとに失敗を返し、ほかのファイルは続ける）。
//! 2. **ID は元のファイルの名前（拡張子を除き、NFC にしたもの）**。表示の名前は DCP の ProfileName（.cube は TITLE）、
//!    なければ ID。同じ種類で ID が大文字・小文字を区別せずに同じものがあれば、内容が同じなら何もせず（`unchanged`）、
//!    違えば `replace` のときだけ置き換える（ID は前のまま。使っている写真は次に開いたときに「内容が違う」と知らせる。
//!    レビュー R-08）。内容が同じでも、**写しが消えた・壊れた（中身が名前の SHA-256 と違う）なら書き直す**（`repaired`。
//!    「読めません。取り込み直してください」の案内のとおり、同じファイルを取り込み直せば直る）。
//! 3. 写しを書く: 同じフォルダの一時ファイルに書いて同期し、上書きしない名前の変更で公開する（genzo-media の
//!    `write_atomically`。6.4 節）。同じ名前の写しがあって中身が違う（壊れた写し。名前は内容の SHA-256 なので、正しい
//!    中身は 1 つ）ときだけ、名前の変更で置き換える。**読み直して SHA-256 を確かめてから**一覧を書き換える（一覧を書けな
//!    ければ、この呼び出しで新しく書いた写しを消す）。置き換えた前の写しは、どの項目も指さなくなってから消す（消せなくても
//!    一覧は正しい）。
//!
//! 一覧のファイルがない（`NotFound`）なら空の一覧から始める。**あるのに読めない**（権限・入出力のエラー）なら、一覧の
//! 操作（一覧・取り込み・除く）はエラーにし、一覧を書き換えない（空の一覧で上書きして登録を失わない）。読めるように
//! なれば次に使うときに読み直す。中身が壊れた一覧（JSON として読めない・版が違う・大きすぎる）は
//! `index.json.broken-<時刻>` に名前を変えて退避し、空の一覧から始める（写しは消さない）。
//!
//! # 引く（[`ProfileResolver`] の実装）
//!
//! 一覧から ID（そのままの綴り）で探し、写しを読んで SHA-256 が一覧と同じことを確かめてから解析する（解析したものは
//! 内容の SHA-256 ごとにメモリに持つ）。写しがない・内容が違う・解析できないなら `None`（描画は標準の色・ルックなしで、
//! 「見つからない」と知らせる）。読めなかった写しは、そのときのファイルの様子（ない・大きさと更新日時）を覚え、様子が
//! 変わるまで読み直さない（描画のたびに読み込み・SHA-256 の計算・ログを繰り返さない）。読み込みと解析はロックの外で
//! 行う。

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use genzo_color::{CubeLut, DcpProfile};
use genzo_media::{MediaError, ProtectedFiles, WriteOutcome, write_atomically};
use genzo_model::{ConflictPolicy, DataRef, is_valid_profile_id};
use genzo_pipeline::ProfileResolver;
use genzo_pipeline::profile::{Resolved, cube_ref_id, dcp_ref_id};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::Core;
use crate::error::ApiError;
use crate::types::{ProfileEntry, ProfileImportFailure, ProfileImportReport, ProfileKind};

/// ライブラリのフォルダの名前（データのフォルダの下）。
pub const PROFILES_DIR: &str = "profiles";

/// 一覧のファイルの名前。
const INDEX_FILE: &str = "index.json";

/// 一覧の形の版。
const INDEX_VERSION: u32 = 1;

/// 一覧の項目の数の上限（**仮置き**。壊れた一覧で大量のメモリを使わないため）。
pub const MAX_PROFILE_ENTRIES: usize = 10_000;

/// 一覧のファイルの大きさの上限（バイト。**仮置き**）。
const MAX_INDEX_BYTES: u64 = 16 << 20;

impl ProfileKind {
    /// 保存する写しの拡張子。
    fn extension(self) -> &'static str {
        match self {
            ProfileKind::Camera => "dcp",
            ProfileKind::Look => "cube",
        }
    }

    /// ファイルの大きさの上限（バイト）。
    fn max_bytes(self) -> usize {
        match self {
            ProfileKind::Camera => genzo_color::dcp::MAX_DCP_BYTES,
            ProfileKind::Look => genzo_color::cube::MAX_CUBE_BYTES,
        }
    }

    /// 拡張子から（大文字・小文字は区別しない）。
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "dcp" => Some(ProfileKind::Camera),
            "cube" => Some(ProfileKind::Look),
            _ => None,
        }
    }

    /// 表示の名前。
    pub fn label(self) -> &'static str {
        match self {
            ProfileKind::Camera => "カメラプロファイル",
            ProfileKind::Look => "ルック",
        }
    }
}

/// 一覧の 1 件（`index.json`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredEntry {
    kind: ProfileKind,
    id: String,
    name: String,
    /// 写しのファイルの名前（`<SHA-256>.<拡張子>`）。
    file: String,
    sha256: String,
    size: u64,
    original_name: String,
    imported_at: String,
    #[serde(default)]
    camera_model: Option<String>,
}

impl StoredEntry {
    /// 一覧から読んだ項目として正しいか（壊れた・書き換えられた一覧で、フォルダの外のファイルを読まないため）。
    fn is_sane(&self) -> bool {
        is_valid_profile_id(&self.id)
            && is_sha256_hex(&self.sha256)
            && self.file == format!("{}.{}", self.sha256, self.kind.extension())
    }

    fn same_id(&self, kind: ProfileKind, id: &str) -> bool {
        self.kind == kind && self.id.to_lowercase() == id.to_lowercase()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoredIndex {
    version: u32,
    entries: Vec<StoredEntry>,
}

/// 解析したもの（内容の SHA-256 ごと）。
#[derive(Debug, Clone)]
enum Parsed {
    Camera(Arc<DcpProfile>),
    Look(Arc<CubeLut>),
}

/// 写しのファイルの様子（読めなかった写しを、変わるまで読み直さないため）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum FileStamp {
    /// ない。
    Missing,
    /// ある（大きさと更新日時）。
    Present {
        len: u64,
        modified: Option<SystemTime>,
    },
    /// 様子を調べられない（エラーの種類）。
    Error(std::io::ErrorKind),
}

impl FileStamp {
    fn of(path: &Path) -> Self {
        match std::fs::metadata(path) {
            Ok(m) => FileStamp::Present {
                len: m.len(),
                modified: m.modified().ok(),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileStamp::Missing,
            Err(e) => FileStamp::Error(e.kind()),
        }
    }
}

#[derive(Debug, Default)]
struct State {
    /// 一覧（初めて使うときに読む。読めなかったら `None` のまま、次に使うときに読み直す）。
    index: Option<Vec<StoredEntry>>,
    /// 解析したもの（キーは内容の SHA-256）。
    parsed: HashMap<String, Parsed>,
    /// 読めなかった写し（キーは写しのファイルの名前、値はそのときのファイルの様子）。
    failed: HashMap<String, FileStamp>,
}

/// 写しを書いた結果（[`ProfileLibrary::store_copy`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyState {
    /// 同じ中身の写しがあった（書かなかった）。
    Existing,
    /// なかったので新しく書いた。
    Created,
    /// 中身の違う写し（壊れたもの）を書き直した。
    Repaired,
}

/// カメラプロファイル・ルックのライブラリ（モジュールの doc）。
#[derive(Debug)]
pub struct ProfileLibrary {
    dir: PathBuf,
    state: Mutex<State>,
    /// 取り込み・削除を 1 つずつにするロック（ほかのロックを持たずに最初に取る）。
    ops: Mutex<()>,
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 解析する（種類ごと）。
fn parse(kind: ProfileKind, bytes: &[u8]) -> Result<Parsed, String> {
    match kind {
        ProfileKind::Camera => DcpProfile::parse(bytes)
            .map(|p| Parsed::Camera(Arc::new(p)))
            .map_err(|e| e.to_string()),
        ProfileKind::Look => {
            let text = std::str::from_utf8(bytes).map_err(|_| {
                "3D LUT（.cube）のファイルが UTF-8 のテキストではありません".to_owned()
            })?;
            CubeLut::parse(text)
                .map(|l| Parsed::Look(Arc::new(l)))
                .map_err(|e| e.to_string())
        }
    }
}

/// ProfileEmbedPolicy の説明。
fn embed_policy_text(v: u32) -> String {
    match v {
        0 => "コピーを許す（0）".to_owned(),
        1 => "使ったときだけ埋め込む（1）".to_owned(),
        2 => "埋め込まない（2）".to_owned(),
        3 => "制限なし（3）".to_owned(),
        other => format!("不明（{other}）"),
    }
}

/// 読んで解析したファイル（取り込みと [`inspect_profile_file`]）。
struct ProfileFile {
    kind: ProfileKind,
    bytes: Vec<u8>,
    parsed: Parsed,
    /// 元のファイルの名前から拡張子を除き、NFC にしたもの。
    id: String,
    name: String,
    camera_model: Option<String>,
    sha256: String,
}

/// ファイルを読んで解析する（種類は拡張子で決める。大きさの上限を超える・解析できない・名前を ID にできないなら、
/// 理由の文）。
fn read_profile_file(path: &Path) -> Result<ProfileFile, String> {
    let kind = ProfileKind::from_path(path).ok_or_else(|| {
        "対応していない形式です（カメラプロファイルは .dcp、ルックは .cube）".to_owned()
    })?;
    let meta = std::fs::metadata(path).map_err(|e| format!("ファイルを読めません: {e}"))?;
    if !meta.is_file() {
        return Err("ファイルではありません".to_owned());
    }
    if meta.len() > kind.max_bytes() as u64 {
        return Err(format!(
            "ファイルが大きすぎます（{} バイト。{} バイトまで）",
            meta.len(),
            kind.max_bytes()
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("ファイルを読めません: {e}"))?;
    let parsed = parse(kind, &bytes)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "ファイルの名前を読めません".to_owned())?;
    let id = genzo_catalog::text::nfc(stem.trim());
    if !is_valid_profile_id(&id) {
        return Err(format!(
            "ファイルの名前を ID にできません（1〜{} 文字で、制御文字を含まない名前にしてください）",
            genzo_model::MAX_PROFILE_ID_CHARS
        ));
    }
    let sha256 = sha256_hex(&bytes);
    let (name, camera_model) = match &parsed {
        Parsed::Camera(p) => (
            p.display_name().map_or_else(|| id.clone(), str::to_owned),
            p.unique_camera_model.clone(),
        ),
        Parsed::Look(l) => (l.title.clone().unwrap_or_else(|| id.clone()), None),
    };
    Ok(ProfileFile {
        kind,
        bytes,
        parsed,
        id,
        name,
        camera_model,
        sha256,
    })
}

/// 取り込まずに、カメラプロファイル（.dcp）・ルック（.cube）のファイルを読んで中身を説明する（CLI の
/// `profile inspect`。カタログは要らない。元のファイルは変えない）。`camera` を渡すと、その機種用かを
/// [`ProfileEntry::matches_camera`] に入れる。取り込んだ日時は空。読めない・解析できないなら
/// [`ApiError::InvalidArgument`]。
pub fn inspect_profile_file(path: &Path, camera: Option<&str>) -> Result<ProfileEntry, ApiError> {
    let f = read_file_arg(path)?;
    Ok(file_entry(path, &f, camera))
}

fn read_file_arg(path: &Path) -> Result<ProfileFile, ApiError> {
    read_profile_file(path)
        .map_err(|m| ApiError::InvalidArgument(format!("{}: {m}", path.display())))
}

/// 取り込んでいないファイルの [`ProfileEntry`]（取り込んだ日時は空）。
fn file_entry(path: &Path, f: &ProfileFile, camera: Option<&str>) -> ProfileEntry {
    let (description, notes, copyright, embed_policy) = describe(&f.parsed);
    let matches_camera = match (&f.parsed, camera) {
        (Parsed::Camera(p), Some(c)) => Some(p.matches_camera(c)),
        _ => None,
    };
    ProfileEntry {
        kind: f.kind,
        id: f.id.clone(),
        name: f.name.clone(),
        camera_model: f.camera_model.clone(),
        matches_camera,
        usable: true,
        description,
        notes,
        copyright,
        embed_policy,
        original_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        sha256: f.sha256.clone(),
        size: f.bytes.len() as u64,
        imported_at: String::new(),
    }
}

/// 取り込まずに、ファイルのカメラプロファイル・ルックをメモリに読む（CLI の `render --dcp`・`--cube`。カタログなしの
/// 現像）。返す一覧の ID（元のファイルの名前から拡張子を除いたもの）を現像設定の `profile.camera`・`profile.look` に
/// 入れ、エンジンに [`genzo_pipeline::MemoryProfiles`] を渡すと使われる。読めない・解析できないなら
/// [`ApiError::InvalidArgument`]。
pub fn load_profile_files(
    paths: &[PathBuf],
) -> Result<(genzo_pipeline::MemoryProfiles, Vec<ProfileEntry>), ApiError> {
    let mut mem = genzo_pipeline::MemoryProfiles::default();
    let mut entries = Vec::with_capacity(paths.len());
    for path in paths {
        let f = read_file_arg(path)?;
        let entry = file_entry(path, &f, None);
        match f.parsed {
            Parsed::Camera(p) => mem.insert_camera(&f.id, (*p).clone(), &f.sha256),
            Parsed::Look(l) => mem.insert_look(&f.id, (*l).clone(), &f.sha256),
        }
        entries.push(entry);
    }
    Ok((mem, entries))
}

/// 解析したものの要約（[`ProfileEntry`] の説明・使わない情報・著作権の表示・埋め込みの方針）。
fn describe(parsed: &Parsed) -> (String, Vec<String>, Option<String>, Option<String>) {
    use genzo_color::dcp::illuminant_name;
    match parsed {
        Parsed::Camera(p) => {
            let mut parts = Vec::new();
            let ills: Vec<String> = p
                .calibration_illuminants
                .iter()
                .zip(&p.color_matrices)
                .filter_map(|(i, m)| {
                    m.as_ref()
                        .map(|_| i.map_or_else(|| "光源の指定なし".to_owned(), illuminant_name))
                })
                .collect();
            if p.is_dual_illuminant() {
                parts.push(format!("光源 2 つ（{}）で補間", ills.join("・")));
            } else {
                parts.push(format!(
                    "光源 1 つ（{}）",
                    ills.first().cloned().unwrap_or_default()
                ));
            }
            if p.forward_matrices[0].is_some() {
                parts.push("ForwardMatrix あり".to_owned());
            }
            if let Some(m) = p.hue_sat_maps.iter().flatten().next() {
                let [h, s, v] = m.dims();
                parts.push(format!("色相彩度のマップ（{h} × {s} × {v}）"));
            }
            if let Some(t) = &p.look_table {
                let [h, s, v] = t.dims();
                let enc = if t.srgb_value_encoding() {
                    "・sRGB"
                } else {
                    ""
                };
                parts.push(format!("ルックテーブル（{h} × {s} × {v}{enc}）"));
            }
            let mut notes = Vec::new();
            if let Some(c) = &p.tone_curve {
                notes.push(format!(
                    "トーンカーブ（{} 点）は使いません（このアプリのシーン → ディスプレイの変換を使います）",
                    c.len()
                ));
            }
            if let Some(o) = p.baseline_exposure_offset
                && o != 0.0
            {
                notes.push(format!(
                    "露出のずれ（BaselineExposureOffset {o:+.2} EV）は使いません"
                ));
            }
            notes.extend(p.ignored.iter().cloned());
            (
                parts.join("・"),
                notes,
                p.copyright.clone(),
                p.embed_policy.map(embed_policy_text),
            )
        }
        Parsed::Look(l) => {
            let n = l.size();
            let domain = if l.has_default_domain() {
                String::new()
            } else {
                format!("・入力の範囲 {:?}〜{:?}", l.domain_min, l.domain_max)
            };
            (
                format!("3D LUT（{n} × {n} × {n}{domain}。sRGB の値の LUT とみなします）"),
                Vec::new(),
                None,
                None,
            )
        }
    }
}

impl ProfileLibrary {
    /// データのフォルダ `data_dir` のライブラリ（フォルダは取り込むときに作る）。
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(PROFILES_DIR),
            state: Mutex::new(State::default()),
            ops: Mutex::new(()),
        }
    }

    /// フォルダ。
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn index_path(&self) -> PathBuf {
        self.dir.join(INDEX_FILE)
    }

    /// 一覧を読む（モジュールの doc）。ない（`NotFound`）なら空。あるのに読めない（権限・入出力のエラー）なら
    /// [`ApiError::Io`]。中身が壊れた一覧は `index.json.broken-<時刻>` に名前を変えて退避して空にする。
    fn read_index(&self) -> Result<Vec<StoredEntry>, ApiError> {
        let path = self.index_path();
        let io = |source: std::io::Error| ApiError::Io {
            path: path.clone(),
            source,
        };
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(e)),
        };
        let parsed = if meta.len() <= MAX_INDEX_BYTES {
            let bytes = std::fs::read(&path).map_err(io)?;
            serde_json::from_slice::<StoredIndex>(&bytes)
                .ok()
                .filter(|i| i.version == INDEX_VERSION && i.entries.len() <= MAX_PROFILE_ENTRIES)
        } else {
            None
        };
        match parsed {
            Some(i) => Ok(i.entries.into_iter().filter(StoredEntry::is_sane).collect()),
            None => {
                let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
                let aside = self.dir.join(format!("{INDEX_FILE}.broken-{stamp}"));
                tracing::warn!(path = %path.display(), aside = %aside.display(), "プロファイルの一覧が壊れているので退避する");
                if let Err(e) = std::fs::rename(&path, &aside) {
                    tracing::warn!(error = %e, "プロファイルの一覧を退避できない");
                }
                Ok(Vec::new())
            }
        }
    }

    /// 一覧（読んでいなければ読む）。読めなければ [`ApiError::Io`]（覚えずに、次に使うときに読み直す）。
    fn entries(&self) -> Result<Vec<StoredEntry>, ApiError> {
        let mut st = self.state.lock();
        if st.index.is_none() {
            st.index = Some(self.read_index()?);
        }
        Ok(st.index.clone().unwrap_or_default())
    }

    /// 一覧を書く（一時ファイルに書いてから名前を変えて置き換える）。
    fn write_index(&self, entries: &[StoredEntry]) -> Result<(), ApiError> {
        let index = StoredIndex {
            version: INDEX_VERSION,
            entries: entries.to_vec(),
        };
        let json = serde_json::to_vec_pretty(&index).map_err(|e| {
            ApiError::Internal(format!("プロファイルの一覧を JSON にできません: {e}"))
        })?;
        write_atomically(
            &self.index_path(),
            ConflictPolicy::Overwrite,
            &ProtectedFiles::new(),
            |f| {
                f.write_all(&json).map_err(|e| MediaError::Io {
                    path: Some(self.index_path()),
                    source: e,
                })
            },
        )?;
        Ok(())
    }

    /// 写しを読んで、SHA-256 が一覧と同じなら解析する（ロックの外で呼ぶ）。読めなかった写しは、ファイルの様子が
    /// 変わるまで読み直さない（モジュールの doc の「引く」）。
    fn load(&self, entry: &StoredEntry) -> Option<Parsed> {
        let path = self.dir.join(&entry.file);
        let stamp = {
            let st = self.state.lock();
            if let Some(p) = st.parsed.get(&entry.sha256) {
                return Some(p.clone());
            }
            let stamp = FileStamp::of(&path);
            if st.failed.get(&entry.file) == Some(&stamp) {
                return None;
            }
            stamp
        };
        let fail = |message: &str, error: Option<String>| {
            tracing::warn!(path = %path.display(), error = error.as_deref().unwrap_or(""), "{message}");
            self.state
                .lock()
                .failed
                .insert(entry.file.clone(), stamp.clone());
            None
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => return fail("プロファイルの写しを読めない", Some(e.to_string())),
        };
        if sha256_hex(&bytes) != entry.sha256 {
            return fail("プロファイルの写しの内容が一覧と違う（使わない）", None);
        }
        match parse(entry.kind, &bytes) {
            Ok(p) => {
                let mut st = self.state.lock();
                st.parsed.insert(entry.sha256.clone(), p.clone());
                st.failed.remove(&entry.file);
                Some(p)
            }
            Err(e) => fail("プロファイルの写しを解析できない", Some(e)),
        }
    }

    fn find(&self, kind: ProfileKind, id: &str) -> Option<StoredEntry> {
        match self.entries() {
            Ok(entries) => entries.into_iter().find(|e| e.kind == kind && e.id == id),
            Err(e) => {
                tracing::debug!(error = %e, "プロファイルの一覧を読めない");
                None
            }
        }
    }

    /// 一覧（`camera` を渡すと、カメラプロファイルがその機種用かを [`ProfileEntry::matches_camera`] に入れる）。
    /// 種類 → 表示の名前の順に並べる。一覧のファイルがあるのに読めなければ [`ApiError::Io`]。
    pub fn list(&self, camera: Option<&str>) -> Result<Vec<ProfileEntry>, ApiError> {
        let mut out: Vec<ProfileEntry> = self
            .entries()?
            .into_iter()
            .map(|e| self.entry_of(&e, camera))
            .collect();
        out.sort_by(|a, b| {
            (a.kind as u8, a.name.to_lowercase(), &a.id).cmp(&(
                b.kind as u8,
                b.name.to_lowercase(),
                &b.id,
            ))
        });
        Ok(out)
    }

    fn entry_of(&self, e: &StoredEntry, camera: Option<&str>) -> ProfileEntry {
        let parsed = self.load(e);
        let (description, notes, copyright, embed_policy) = match &parsed {
            Some(p) => describe(p),
            None => (
                "読めません（写しがない・内容が違う・解析できない）".to_owned(),
                Vec::new(),
                None,
                None,
            ),
        };
        let matches_camera = match (e.kind, camera, &parsed) {
            (ProfileKind::Camera, Some(c), Some(Parsed::Camera(p))) => Some(p.matches_camera(c)),
            (ProfileKind::Camera, Some(_), _) => Some(false),
            _ => None,
        };
        ProfileEntry {
            kind: e.kind,
            id: e.id.clone(),
            name: e.name.clone(),
            camera_model: e.camera_model.clone(),
            matches_camera,
            usable: parsed.is_some(),
            description,
            notes,
            copyright,
            embed_policy,
            original_name: e.original_name.clone(),
            sha256: e.sha256.clone(),
            size: e.size,
            imported_at: e.imported_at.clone(),
        }
    }

    /// ファイルを取り込む（モジュールの doc の「取り込み」）。ファイルごとの失敗は結果に入れ、ほかのファイルは続ける。
    pub fn import(&self, paths: &[PathBuf], replace: bool) -> ProfileImportReport {
        let _op = self.ops.lock();
        let mut report = ProfileImportReport::default();
        for path in paths {
            match self.import_one(path, replace) {
                Ok((entry, outcome)) => {
                    let e = self.entry_of(&entry, None);
                    match outcome {
                        ImportOutcome::Imported => report.imported.push(e),
                        ImportOutcome::Replaced => report.replaced.push(e),
                        ImportOutcome::Repaired => report.repaired.push(e),
                        ImportOutcome::Unchanged => report.unchanged.push(e),
                    }
                }
                Err(message) => report.failed.push(ProfileImportFailure {
                    path: path.clone(),
                    message,
                }),
            }
        }
        report
    }

    fn import_one(
        &self,
        path: &Path,
        replace: bool,
    ) -> Result<(StoredEntry, ImportOutcome), String> {
        let ProfileFile {
            kind,
            bytes,
            parsed,
            id,
            name,
            camera_model,
            sha256,
        } = read_profile_file(path)?;
        let mut entries = self
            .entries()
            .map_err(|e| format!("プロファイルの一覧を読めません（取り込みませんでした）: {e}"))?;
        let existing = entries.iter().position(|e| e.same_id(kind, &id));
        if let Some(i) = existing {
            if entries[i].sha256 == sha256 {
                // 一覧は同じ内容を指す。写しが消えた・壊れたなら書き直す（取り込み直しで直せる）。
                let state = self.store_copy(&entries[i].file, &bytes, &sha256)?;
                if state == CopyState::Existing {
                    return Ok((entries[i].clone(), ImportOutcome::Unchanged));
                }
                let mut st = self.state.lock();
                st.failed.remove(&entries[i].file);
                st.parsed.insert(sha256, parsed);
                return Ok((entries[i].clone(), ImportOutcome::Repaired));
            }
            if !replace {
                return Err(format!(
                    "同じ ID（{}）の{}がすでにあります（内容が違います）。置き換えるには「置き換える」を選んでください",
                    entries[i].id,
                    kind.label()
                ));
            }
        }
        // 写しを書いて、読み直して確かめる。
        let file = format!("{sha256}.{}", kind.extension());
        let dest = self.dir.join(&file);
        let wrote = self.store_copy(&file, &bytes, &sha256)? == CopyState::Created;
        let entry = StoredEntry {
            kind,
            id: existing.map_or_else(|| id.clone(), |i| entries[i].id.clone()),
            name,
            file: file.clone(),
            sha256: sha256.clone(),
            size: bytes.len() as u64,
            original_name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            imported_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            camera_model,
        };
        let old = match existing {
            Some(i) => Some(std::mem::replace(&mut entries[i], entry.clone())),
            None => {
                if entries.len() >= MAX_PROFILE_ENTRIES {
                    if wrote {
                        let _ = std::fs::remove_file(&dest);
                    }
                    return Err(format!("取り込めるのは {MAX_PROFILE_ENTRIES} 件までです"));
                }
                entries.push(entry.clone());
                None
            }
        };
        if let Err(e) = self.write_index(&entries) {
            if wrote {
                let _ = std::fs::remove_file(&dest);
            }
            return Err(format!("プロファイルの一覧を書けません: {e}"));
        }
        {
            let mut st = self.state.lock();
            st.index = Some(entries.clone());
            st.failed.remove(&file);
            st.parsed.insert(sha256, parsed);
        }
        if let Some(old) = old {
            self.remove_unreferenced(&old, &entries);
            return Ok((entry, ImportOutcome::Replaced));
        }
        Ok((entry, ImportOutcome::Imported))
    }

    /// 写し `file`（`<SHA-256>.<拡張子>`）を中身 `bytes`（SHA-256 が `sha256`）にする（モジュールの doc の「取り込み」の 3）。
    /// 同じ中身の写しがあれば書かない。ないなら上書きしない名前の変更で書き、中身の違う写し（壊れたもの）があれば名前の
    /// 変更で置き換える。書いた後に読み直して SHA-256 を確かめる（違えば、新しく書いた写しは消してエラー）。
    fn store_copy(&self, file: &str, bytes: &[u8], sha256: &str) -> Result<CopyState, String> {
        std::fs::create_dir_all(&self.dir).map_err(|e| {
            format!(
                "プロファイルのフォルダを作れません（{}）: {e}",
                self.dir.display()
            )
        })?;
        let dest = self.dir.join(file);
        let policy = match std::fs::read(&dest) {
            Ok(b) if sha256_hex(&b) == sha256 => return Ok(CopyState::Existing),
            Ok(_) => ConflictPolicy::Overwrite,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => ConflictPolicy::Skip,
            Err(e) => return Err(format!("写しを確かめられません（{}）: {e}", dest.display())),
        };
        let state = match write_atomically(&dest, policy, &ProtectedFiles::new(), |f| {
            f.write_all(bytes).map_err(|e| MediaError::Io {
                path: Some(dest.clone()),
                source: e,
            })
        }) {
            Ok(WriteOutcome::Written { replaced: true, .. }) => CopyState::Repaired,
            Ok(WriteOutcome::Written { .. }) => CopyState::Created,
            // 確かめた後にほかから作られた（同じ中身なら、そのまま使う）。
            Ok(WriteOutcome::Skipped { .. }) => CopyState::Existing,
            Err(e) => return Err(format!("写しを書けません: {e}")),
        };
        let check = std::fs::read(&dest).map(|b| sha256_hex(&b)).ok();
        if check.as_deref() != Some(sha256) {
            if state == CopyState::Created {
                let _ = std::fs::remove_file(&dest);
            }
            return Err(
                "写しを書いた後の確認で内容が違いました（取り込みませんでした）".to_owned(),
            );
        }
        Ok(state)
    }

    /// どの項目も指さなくなった写しを消す（消せなくても一覧は正しい。ログだけ）。
    fn remove_unreferenced(&self, old: &StoredEntry, entries: &[StoredEntry]) {
        if entries.iter().any(|e| e.file == old.file) {
            return;
        }
        let path = self.dir.join(&old.file);
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), error = %e, "使わなくなったプロファイルの写しを消せない");
        }
        let mut st = self.state.lock();
        st.parsed.remove(&old.sha256);
        st.failed.remove(&old.file);
    }

    /// 一覧から除く（写しは、どの項目も指さなくなれば消す）。ない ID は [`ApiError::NotFound`]。
    pub fn remove(&self, kind: ProfileKind, id: &str) -> Result<(), ApiError> {
        let _op = self.ops.lock();
        let mut entries = self.entries()?;
        let Some(i) = entries.iter().position(|e| e.kind == kind && e.id == id) else {
            return Err(ApiError::NotFound(format!("{}「{id}」", kind.label())));
        };
        let old = entries.remove(i);
        self.write_index(&entries)?;
        self.state.lock().index = Some(entries.clone());
        self.remove_unreferenced(&old, &entries);
        Ok(())
    }
}

enum ImportOutcome {
    Imported,
    Replaced,
    Repaired,
    Unchanged,
}

impl Core {
    /// 取り込んだカメラプロファイル・ルックの一覧（種類 → 表示の名前の順）。`camera`（機種。[`crate::SourceInfo::camera`]）を
    /// 渡すと、カメラプロファイルがその機種用かを [`ProfileEntry::matches_camera`] に入れる（画面は、その機種用のものを
    /// 先に出し、ほかの機種用のものは「すべて表示」で出す）。
    pub fn list_profiles(&self, camera: Option<&str>) -> Result<Vec<ProfileEntry>, ApiError> {
        self.inner.check_open()?;
        self.inner.profiles.list(camera)
    }

    /// 取り込んだカメラプロファイル・ルックを置くフォルダ（データのフォルダの `profiles`。まだないこともある）。
    pub fn profiles_dir(&self) -> PathBuf {
        self.inner.profiles.dir().to_path_buf()
    }

    /// カメラプロファイル（.dcp）・ルック（.cube）を取り込む（`profiles` の doc。元のファイルは変えない）。
    /// ファイルごとの失敗は結果に入れる。置き換えたものがあり、現像中の写真が使っていれば描き直す。
    pub fn import_profiles(
        &self,
        paths: &[PathBuf],
        replace: bool,
    ) -> Result<ProfileImportReport, ApiError> {
        self.inner.check_open()?;
        if paths.is_empty() {
            return Err(ApiError::InvalidArgument(
                "取り込むファイルを指定してください".to_owned(),
            ));
        }
        let report = self.inner.profiles.import(paths, replace);
        if !report.imported.is_empty() || !report.replaced.is_empty() || !report.repaired.is_empty()
        {
            crate::develop::refresh_profiles(&self.inner);
        }
        Ok(report)
    }

    /// 取り込んだカメラプロファイル・ルックを一覧から除く（写しは、どの項目も指さなくなれば消す）。使っている写真は
    /// 標準の色・ルックなしで描き、「見つからない」と知らせる。ない ID は [`ApiError::NotFound`]。
    pub fn remove_profile(&self, kind: ProfileKind, id: &str) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner.profiles.remove(kind, id)?;
        crate::develop::refresh_profiles(&self.inner);
        Ok(())
    }
}

impl ProfileResolver for ProfileLibrary {
    fn camera_profile(&self, id: &str) -> Option<Resolved<DcpProfile>> {
        let e = self.find(ProfileKind::Camera, id)?;
        match self.load(&e)? {
            Parsed::Camera(p) => Some(Resolved {
                data: p,
                data_ref: DataRef::new(dcp_ref_id(&e.id), e.sha256),
            }),
            Parsed::Look(_) => None,
        }
    }

    fn look(&self, id: &str) -> Option<Resolved<CubeLut>> {
        let e = self.find(ProfileKind::Look, id)?;
        match self.load(&e)? {
            Parsed::Look(l) => Some(Resolved {
                data: l,
                data_ref: DataRef::new(cube_ref_id(&e.id), e.sha256),
            }),
            Parsed::Camera(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use genzo_color::Mat3;

    use super::*;

    fn dcp(name: &str, scale: f64) -> Vec<u8> {
        DcpProfile {
            name: Some(name.into()),
            unique_camera_model: Some("Sony ILCE-7M4".into()),
            calibration_illuminants: [Some(21), None],
            color_matrices: [
                Some(Mat3::from_rows([
                    [0.70 * scale, -0.20, -0.06],
                    [-0.50, 1.30, 0.23],
                    [-0.07, 0.13, 0.65],
                ])),
                None,
            ],
            tone_curve: Some(vec![[0.0, 0.0], [1.0, 1.0]]),
            ..Default::default()
        }
        .to_bytes()
    }

    const CUBE: &str = "TITLE \"フィルム風\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";

    #[test]
    fn import_list_resolve_replace_and_remove() {
        let d = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let a = src.path().join("Sony ILCE-7M4 テスト.dcp");
        std::fs::write(&a, dcp("テスト標準", 1.0)).unwrap();
        let c = src.path().join("film.CUBE");
        std::fs::write(&c, CUBE).unwrap();
        let bad = src.path().join("壊れた.dcp");
        std::fs::write(&bad, b"IIRC").unwrap();
        let other = src.path().join("x.icc");
        std::fs::write(&other, b"x").unwrap();
        let lib = ProfileLibrary::new(d.path());
        let r = lib.import(&[a.clone(), c.clone(), bad.clone(), other.clone()], false);
        assert_eq!(r.imported.len(), 2, "{r:?}");
        assert_eq!(r.failed.len(), 2);
        assert!(r.failed[1].message.contains(".dcp"), "{r:?}");
        let list = lib.list(Some("SONY ILCE-7M4")).unwrap();
        assert_eq!(list.len(), 2);
        let cam = &list[0];
        assert_eq!(cam.kind, ProfileKind::Camera);
        assert_eq!(cam.id, "Sony ILCE-7M4 テスト");
        assert_eq!(cam.name, "テスト標準");
        assert_eq!(cam.matches_camera, Some(true));
        assert!(cam.usable);
        assert!(cam.description.contains("D65"), "{}", cam.description);
        assert!(
            cam.notes.iter().any(|n| n.contains("トーンカーブ")),
            "{:?}",
            cam.notes
        );
        assert_eq!(list[1].kind, ProfileKind::Look);
        assert_eq!(list[1].id, "film");
        assert_eq!(list[1].name, "フィルム風");
        assert_eq!(list[1].matches_camera, None);
        assert_eq!(
            lib.list(Some("Canon EOS R5")).unwrap()[0].matches_camera,
            Some(false)
        );
        // 引ける（参照は種類・ID・内容の SHA-256）。
        let p = lib.camera_profile("Sony ILCE-7M4 テスト").unwrap();
        assert_eq!(p.data_ref.id, "dcp:Sony ILCE-7M4 テスト");
        assert_eq!(p.data_ref.sha256, cam.sha256);
        assert!(lib.look("film").is_some());
        assert!(lib.camera_profile("film").is_none());
        // 同じ内容の取り込みは何もしない。
        let r = lib.import(std::slice::from_ref(&a), false);
        assert_eq!(r.unchanged.len(), 1);
        // 同じ名前で内容が違えば、置き換えるときだけ置き換える（大文字・小文字の違いは同じ ID）。
        let a2 = src.path().join("sony ilce-7m4 テスト.dcp");
        std::fs::write(&a2, dcp("テスト標準 2", 0.9)).unwrap();
        let r = lib.import(std::slice::from_ref(&a2), false);
        assert_eq!(r.failed.len(), 1);
        assert!(r.failed[0].message.contains("すでにあります"));
        let old_sha = cam.sha256.clone();
        let r = lib.import(std::slice::from_ref(&a2), true);
        assert_eq!(r.replaced.len(), 1, "{r:?}");
        assert_eq!(r.replaced[0].id, "Sony ILCE-7M4 テスト", "ID は前のまま");
        let p2 = lib.camera_profile("Sony ILCE-7M4 テスト").unwrap();
        assert_ne!(p2.data_ref.sha256, old_sha);
        assert!(
            !d.path()
                .join(PROFILES_DIR)
                .join(format!("{old_sha}.dcp"))
                .exists()
        );
        // 作り直したライブラリ（次に開いたとき）でも同じ一覧。
        let lib2 = ProfileLibrary::new(d.path());
        assert_eq!(lib2.list(None).unwrap().len(), 2);
        assert!(lib2.camera_profile("Sony ILCE-7M4 テスト").is_some());
        // 除く。
        lib2.remove(ProfileKind::Look, "film").unwrap();
        assert!(lib2.look("film").is_none());
        assert!(matches!(
            lib2.remove(ProfileKind::Look, "film"),
            Err(ApiError::NotFound(_))
        ));
        assert_eq!(lib2.list(None).unwrap().len(), 1);
    }

    #[test]
    fn tampered_copies_and_broken_indexes_are_not_used() {
        let d = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let a = src.path().join("cam.dcp");
        std::fs::write(&a, dcp("x", 1.0)).unwrap();
        let lib = ProfileLibrary::new(d.path());
        assert_eq!(lib.import(&[a], false).imported.len(), 1);
        let entry = &lib.list(None).unwrap()[0];
        // 写しの中身が変わったら使わない（一覧の SHA-256 と違う）。
        let copy = d
            .path()
            .join(PROFILES_DIR)
            .join(format!("{}.dcp", entry.sha256));
        std::fs::write(&copy, dcp("y", 0.8)).unwrap();
        let fresh = ProfileLibrary::new(d.path());
        assert!(fresh.camera_profile("cam").is_none());
        assert!(!fresh.list(None).unwrap()[0].usable);
        // 壊れた一覧は退避して空にする（写しは消さない）。
        std::fs::write(d.path().join(PROFILES_DIR).join(INDEX_FILE), b"{not json").unwrap();
        let broken = ProfileLibrary::new(d.path());
        assert!(broken.list(None).unwrap().is_empty());
        let names: Vec<String> = std::fs::read_dir(d.path().join(PROFILES_DIR))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().any(|n| n.starts_with("index.json.broken-")),
            "{names:?}"
        );
        assert!(copy.exists());
        // フォルダの外を指す一覧の項目は読まない。
        let evil = StoredIndex {
            version: INDEX_VERSION,
            entries: vec![StoredEntry {
                kind: ProfileKind::Camera,
                id: "evil".into(),
                name: "evil".into(),
                file: "../../etc/passwd".into(),
                sha256: "ab".repeat(32),
                size: 1,
                original_name: String::new(),
                imported_at: String::new(),
                camera_model: None,
            }],
        };
        std::fs::write(
            d.path().join(PROFILES_DIR).join(INDEX_FILE),
            serde_json::to_vec(&evil).unwrap(),
        )
        .unwrap();
        let lib = ProfileLibrary::new(d.path());
        assert!(lib.list(None).unwrap().is_empty());
    }

    #[test]
    fn importing_again_repairs_a_missing_or_tampered_copy() {
        let d = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let a = src.path().join("cam.dcp");
        std::fs::write(&a, dcp("x", 1.0)).unwrap();
        let lib = ProfileLibrary::new(d.path());
        assert_eq!(
            lib.import(std::slice::from_ref(&a), false).imported.len(),
            1
        );
        let sha = lib.list(None).unwrap()[0].sha256.clone();
        let copy = d.path().join(PROFILES_DIR).join(format!("{sha}.dcp"));
        // 写しが消えた: 使えない。読めなかったことを覚え、変わるまで読み直さない。
        std::fs::remove_file(&copy).unwrap();
        let fresh = ProfileLibrary::new(d.path());
        assert!(!fresh.list(None).unwrap()[0].usable);
        assert!(fresh.camera_profile("cam").is_none());
        assert_eq!(
            fresh.state.lock().failed.get(&format!("{sha}.dcp")),
            Some(&FileStamp::Missing)
        );
        // 同じファイルを取り込み直すと写しを書き直す（「同じ内容があった」で終わらない）。
        let r = fresh.import(std::slice::from_ref(&a), false);
        assert_eq!(r.repaired.len(), 1, "{r:?}");
        assert!(r.unchanged.is_empty() && r.failed.is_empty(), "{r:?}");
        assert!(r.repaired[0].usable);
        assert!(fresh.camera_profile("cam").is_some());
        assert!(fresh.state.lock().failed.is_empty());
        // 写しの中身が書き換えられた: 置き換えるの指定がなくても、取り込み直しで直す。
        std::fs::write(&copy, dcp("y", 0.8)).unwrap();
        let fresh = ProfileLibrary::new(d.path());
        assert!(fresh.camera_profile("cam").is_none());
        let r = fresh.import(std::slice::from_ref(&a), false);
        assert_eq!(r.repaired.len(), 1, "{r:?}");
        assert_eq!(sha256_hex(&std::fs::read(&copy).unwrap()), sha);
        assert!(fresh.camera_profile("cam").is_some());
        // 写しが正しければ何もしない。
        let r = fresh.import(std::slice::from_ref(&a), true);
        assert_eq!(r.unchanged.len(), 1, "{r:?}");
        // 外で直された写しは、ファイルが変われば読み直す。
        std::fs::remove_file(&copy).unwrap();
        let fresh = ProfileLibrary::new(d.path());
        assert!(fresh.camera_profile("cam").is_none());
        std::fs::write(&copy, dcp("x", 1.0)).unwrap();
        assert!(fresh.camera_profile("cam").is_some());
    }

    #[test]
    fn a_stale_copy_with_the_wrong_content_does_not_block_a_new_import() {
        // 一覧が指さない、名前（SHA-256）と中身の違う写しが残っていても、取り込める（書き直す）。
        let d = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let bytes = dcp("x", 1.0);
        let a = src.path().join("cam.dcp");
        std::fs::write(&a, &bytes).unwrap();
        let dir = d.path().join(PROFILES_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join(format!("{}.dcp", sha256_hex(&bytes)));
        std::fs::write(&copy, b"junk").unwrap();
        let lib = ProfileLibrary::new(d.path());
        let r = lib.import(&[a], false);
        assert_eq!(r.imported.len(), 1, "{r:?}");
        assert_eq!(std::fs::read(&copy).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[test]
    fn an_index_that_cannot_be_read_is_not_overwritten() {
        // 一覧があるのに読めない（ここでは自分を指すシンボリックリンクで、metadata が NotFound 以外で失敗する）
        // ときは、空の一覧とみなして上書きしない。
        let d = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let dir = d.path().join(PROFILES_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let index = dir.join(INDEX_FILE);
        std::os::unix::fs::symlink(INDEX_FILE, &index).unwrap();
        let a = src.path().join("cam.dcp");
        std::fs::write(&a, dcp("x", 1.0)).unwrap();
        let lib = ProfileLibrary::new(d.path());
        assert!(lib.list(None).is_err());
        let r = lib.import(std::slice::from_ref(&a), false);
        assert_eq!(r.failed.len(), 1, "{r:?}");
        assert!(r.failed[0].message.contains("一覧を読めません"), "{r:?}");
        assert!(
            std::fs::symlink_metadata(&index)
                .unwrap()
                .file_type()
                .is_symlink(),
            "一覧は置き換えない"
        );
        assert!(matches!(
            lib.remove(ProfileKind::Camera, "cam"),
            Err(ApiError::Io { .. })
        ));
        assert!(lib.camera_profile("cam").is_none());
        // 読めるようになれば（次に使うときに読み直す）使える。
        std::fs::remove_file(&index).unwrap();
        assert!(lib.list(None).unwrap().is_empty());
        assert_eq!(lib.import(&[a], false).imported.len(), 1);
    }
}
