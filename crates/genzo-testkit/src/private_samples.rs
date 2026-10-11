//! 実機のサンプル（利用者の私的な写真の RAW）を使う回帰テストの共通の部品（02 の MAINT-03）。
//!
//! サンプルそのものと、サンプルから作った画像は **リポジトリに入れない**。リポジトリに置くのは、
//! 写真の内容を明かさない期待値（寸法・レベル・閾値・粗い統計・振る舞い・ハッシュ）だけで、その形は
//! [`ExpectFile`] の構造体で決める（決まった項目だけ。[`check_privacy`] で検査する）。
//!
//! | 環境変数 | 意味 | 未設定のとき |
//! |---|---|---|
//! | [`SAMPLES_ENV`]（`GENZO_PRIVATE_SAMPLES`） | サンプルのフォルダ（読み取りだけ。サブフォルダは見ない） | テストは何もせずに成功する（標準エラーに「スキップ」と出す） |
//! | [`REQUIRE_ENV`]（`GENZO_REQUIRE_PRIVATE_SAMPLES=1`） | フォルダの指定がない・LibRaw が無効・期待値のあるサンプルが 1 つもない場合に失敗にする | スキップを許す |
//! | [`UPDATE_ENV`]（`GENZO_UPDATE_PRIVATE_EXPECT=1`） | 期待値のファイルを、計測した値で書き直す（git の差分を確認してからコミットする） | 書き直さない |
//!
//! # プライバシーのガード
//!
//! 1. サンプルはファイルの SHA-256 で識別する。ファイル名・パスは期待値に書かず、テストの出力にも
//!    出さない（ID とハッシュの先頭 12 桁だけ。[`short_hash`]）。
//! 2. 期待値のファイルは決まった項目だけを持ち（`deny_unknown_fields`）、文字列は許可した形だけにする
//!    （[`check_privacy`]。日付の形・パスの区切り・長い文字列を拒む）。普通の `cargo test` で検査する。
//! 3. サンプルのフォルダがリポジトリ（ワークスペース）の中を指していたらエラーにする
//!    （[`sample_dir`]。サンプルを誤ってリポジトリに置くのを防ぐ）。
//! 4. 壊したファイルは OS の一時ディレクトリ（`tempfile`）に作り、テストの終わりに消す。
//! 5. テストの前後で、サンプルの SHA-256 が変わらないことを確かめる（DATA-01）。

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// サンプルのフォルダを指定する環境変数。
pub const SAMPLES_ENV: &str = "GENZO_PRIVATE_SAMPLES";
/// 期待値のあるサンプルがない場合などを失敗にする環境変数（`1`）。
pub const REQUIRE_ENV: &str = "GENZO_REQUIRE_PRIVATE_SAMPLES";
/// 期待値のファイルを書き直す環境変数（`1`）。
pub const UPDATE_ENV: &str = "GENZO_UPDATE_PRIVATE_EXPECT";

/// 期待値のファイルの形式の版。
pub const SCHEMA: u32 = 1;

/// 対象のファイルの拡張子（大文字・小文字を区別しない）。
pub const RAW_EXTENSIONS: [&str; 2] = ["arw", "dng"];

/// 期待値のファイル（JSON）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectFile {
    /// 形式の版（[`SCHEMA`]）。
    pub schema: u32,
    /// サンプルごとの期待値。
    pub samples: Vec<SampleExpect>,
}

/// 1 つのサンプルの期待値。`id` と `sha256` 以外は省略でき、[`UPDATE_ENV`] で計測した値が入る。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleExpect {
    /// サンプルの ID（英小文字・数字・ハイフン。例: `"a7c-cmp-01"`）。
    pub id: String,
    /// ファイルの SHA-256（16 進の小文字 64 桁）。
    pub sha256: String,
    /// 機種（EXIF の Model。`ILCE-` で始まる形などの許可した形だけ）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_model: Option<String>,
    /// 展開とメタデータ（T1）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode: Option<DecodeExpect>,
    /// 飽和（T3）。張り付いた値のないサンプルでは `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<SaturationExpect>,
    /// 既定の現像の統計（T6）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render: Option<RenderExpect>,
}

/// 展開とメタデータの期待値（実数は [`DECODE_TOLERANCE`] 以内で比べる）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecodeExpect {
    /// LibRaw の展開の関数（例: `"sony_arw2_load_raw()"`）。
    pub unpack_function: String,
    /// RAW のバッファの寸法（幅, 高さ）。
    pub raw_size: [u32; 2],
    /// 有効画素の寸法（幅, 高さ）。
    pub active_size: [u32; 2],
    /// 既定の切り抜き（有効画素の座標。左, 上, 幅, 高さ）。
    pub default_crop: Option<[u32; 4]>,
    /// EXIF の向き（1〜8）。
    pub orientation: u8,
    /// CFA の配列（`"RGGB"` など）。
    pub cfa: String,
    /// 黒レベル（R, G1, B, G2）。
    pub black_level: [f32; 4],
    /// 白レベル。
    pub white_level: f32,
    /// 飽和の閾値。
    pub saturation_level: f32,
    /// Sony の SonyRawFileType。
    pub sony_raw_file_type: Option<u32>,
    /// RAW のデータの終わり（`data_offset + data_size`。壊したファイルの期待を計算するのに使う）。
    pub data_end: u64,
}

/// 実数の期待値を比べるときの許容差（相対・絶対の大きい方）。
pub const DECODE_TOLERANCE: f32 = 1e-5;

/// 飽和の期待値（T3）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaturationExpect {
    /// 緑が張り付く値（生の値。この値以上の緑の画素を「張り付いた画素」とする）。
    pub plateau_dn: u16,
    /// 張り付いた緑の画素のうち、飽和の印が付く（閾値以上の）割合の下限。
    pub min_marked_fraction: f64,
    /// 飽和の印が付く画素（全色。閾値以上）の数の範囲（下限, 上限）。
    pub marked_pixels: [u64; 2],
}

/// 既定の現像（`DevelopSettings::default()`、CPU 版）の統計の期待値（T6）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderExpect {
    /// 統計を取る出力の長辺（px。フル解像度で処理してから縮小）。
    pub long_edge: u32,
    /// L\* の平均。
    pub lstar_mean: f64,
    /// L\* の幅（95 パーセンタイル − 5 パーセンタイル）。
    pub lstar_width: f64,
    /// C\*ab の平均。
    pub chroma_mean: f64,
    /// カメラの埋め込み JPEG の L\* の平均（比較の参考）。
    pub camera_lstar_mean: f64,
    /// カメラの埋め込み JPEG の L\* の幅。
    pub camera_lstar_width: f64,
    /// 現像とカメラの JPEG の L\* の平均の差（の絶対値）の上限。
    pub max_lstar_mean_gap: f64,
    /// 統計の許容差（L\*・C\* の単位）。
    pub tolerance: f64,
}

/// 実機のサンプルのテストのエラー。
#[derive(Debug)]
pub struct PrivateSampleError(pub String);

impl fmt::Display for PrivateSampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PrivateSampleError {}

fn err(msg: impl Into<String>) -> PrivateSampleError {
    PrivateSampleError(msg.into())
}

/// 環境変数が `1` か。
pub fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1")
}

/// サンプルのフォルダ（[`SAMPLES_ENV`]）。未設定・空なら `Ok(None)`。
///
/// フォルダが存在しない、またはワークスペース（`workspace_root`）の中を指していたらエラー
/// （サンプルを誤ってリポジトリに置くのを防ぐ）。パスはエラーの文にも出さない。
pub fn sample_dir(workspace_root: &Path) -> Result<Option<PathBuf>, PrivateSampleError> {
    sample_dir_from(std::env::var_os(SAMPLES_ENV), workspace_root)
}

/// [`sample_dir`] の判定（環境変数の値を引数で受け取る）。
pub fn sample_dir_from(
    value: Option<std::ffi::OsString>,
    workspace_root: &Path,
) -> Result<Option<PathBuf>, PrivateSampleError> {
    let Some(raw) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let dir = fs::canonicalize(PathBuf::from(raw)).map_err(|e| {
        err(format!(
            "{SAMPLES_ENV} のフォルダを開けません（{}）",
            e.kind()
        ))
    })?;
    if !dir.is_dir() {
        return Err(err(format!("{SAMPLES_ENV} がフォルダではありません")));
    }
    let root = fs::canonicalize(workspace_root).map_err(|e| {
        err(format!(
            "ワークスペースの場所が分かりません（{}）",
            e.kind()
        ))
    })?;
    if dir.starts_with(&root) {
        return Err(err(format!(
            "{SAMPLES_ENV} がリポジトリの中を指しています。実機のサンプルはリポジトリの外に置いてください"
        )));
    }
    Ok(Some(dir))
}

/// フォルダの中の RAW（[`RAW_EXTENSIONS`]）。サブフォルダは見ない。並びは SHA-256 の順にするので、
/// ここではパスの順に返す（呼び出し側はパスを出力しない）。
pub fn list_raw_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let ok = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| RAW_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)));
        if ok {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

/// ファイルの SHA-256（16 進の小文字）。
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// バイト列の SHA-256（16 進の小文字）。
pub fn sha256_bytes(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// テストの出力に出すハッシュの先頭（12 桁）。
pub fn short_hash(sha256: &str) -> &str {
    &sha256[..sha256.len().min(12)]
}

/// 期待値のファイルを読む（ない場合は空の期待値）。読んだ後に [`check_privacy`] で検査する。
pub fn load_expect(path: &Path) -> Result<ExpectFile, PrivateSampleError> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(ExpectFile {
                schema: SCHEMA,
                samples: Vec::new(),
            });
        }
        Err(e) => return Err(err(format!("期待値のファイルを読めません: {e}"))),
    };
    check_privacy_text(&text)?;
    let file: ExpectFile = serde_json::from_str(&text)
        .map_err(|e| err(format!("期待値のファイルの形が不正です: {e}")))?;
    check_privacy(&file)?;
    Ok(file)
}

/// 期待値のファイルを書く（[`check_privacy`] で検査してから。末尾に改行）。
pub fn save_expect(path: &Path, file: &ExpectFile) -> Result<(), PrivateSampleError> {
    check_privacy(file)?;
    let mut text = serde_json::to_string_pretty(file)
        .map_err(|e| err(format!("期待値を JSON にできません: {e}")))?;
    text.push('\n');
    check_privacy_text(&text)?;
    fs::write(path, text).map_err(|e| err(format!("期待値のファイルを書けません: {e}")))
}

/// 機種の名前として許可する形か（英大文字・数字・ハイフン・空白、1〜24 文字）。
///
/// シリアル番号などを書かないよう、機種名（EXIF の Model）として普通の形だけを許す。
pub fn is_allowed_model(s: &str) -> bool {
    (1..=24).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b' ')
        && s.bytes().any(|b| b.is_ascii_uppercase())
}

/// サンプルの ID として許可する形か（英小文字・数字・ハイフン、1〜32 文字）。
pub fn is_allowed_id(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// 期待値に写真の内容・個人の情報になりうる値がないかを検査する。
///
/// - ID・SHA-256・機種・展開の関数・CFA は許可した形だけ
/// - 同じ SHA-256・同じ ID の重複がない
pub fn check_privacy(file: &ExpectFile) -> Result<(), PrivateSampleError> {
    if file.schema != SCHEMA {
        return Err(err(format!(
            "期待値の形式の版 {} は対応外です",
            file.schema
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    for s in &file.samples {
        if !is_allowed_id(&s.id) {
            return Err(err(format!("サンプルの ID の形が不正です: {:?}", s.id)));
        }
        let hex_ok = s.sha256.len() == 64
            && s.sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex_ok {
            return Err(err(format!("{}: SHA-256 の形が不正です", s.id)));
        }
        if !seen.insert(s.sha256.clone()) || !seen.insert(format!("id:{}", s.id)) {
            return Err(err(format!("{}: ID か SHA-256 が重複しています", s.id)));
        }
        if let Some(m) = &s.camera_model
            && !is_allowed_model(m)
        {
            return Err(err(format!("{}: 機種の名前の形が許可されていません", s.id)));
        }
        if let Some(d) = &s.decode {
            let f = &d.unpack_function;
            let f_ok = (1..=48).contains(&f.len())
                && f.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_()".contains(&b));
            if !f_ok {
                return Err(err(format!("{}: 展開の関数の名前の形が不正です", s.id)));
            }
            if !["RGGB", "BGGR", "GRBG", "GBRG"].contains(&d.cfa.as_str()) {
                return Err(err(format!("{}: CFA の値が不正です", s.id)));
            }
            if !(1..=8).contains(&d.orientation) {
                return Err(err(format!("{}: 向きの値が不正です", s.id)));
            }
        }
    }
    Ok(())
}

/// 期待値のファイルの文字列に、日付の形（`2024:05:01`・`2024-05-01`）・パスの区切り（`/`・`\`）・
/// 長い文字列がないかを検査する（構造体の検査と別に、文字列のまま確かめる）。
pub fn check_privacy_text(text: &str) -> Result<(), PrivateSampleError> {
    let b = text.as_bytes();
    for i in 0..b.len() {
        if i + 10 <= b.len() {
            let w = &b[i..i + 10];
            let date = w[..4].iter().all(u8::is_ascii_digit)
                && (w[4] == b':' || w[4] == b'-')
                && w[5..7].iter().all(u8::is_ascii_digit)
                && w[7] == w[4]
                && w[8..10].iter().all(u8::is_ascii_digit);
            if date {
                return Err(err("期待値のファイルに日付の形の文字列があります"));
            }
        }
    }
    if text.contains('/') || text.contains('\\') {
        return Err(err("期待値のファイルにパスの区切り文字があります"));
    }
    let mut in_str = false;
    let mut len = 0usize;
    for c in text.chars() {
        if c == '"' {
            in_str = !in_str;
            len = 0;
        } else if in_str {
            len += 1;
            if len > 72 {
                return Err(err("期待値のファイルに長い文字列があります"));
            }
        }
    }
    Ok(())
}

/// 実数の比較（[`DECODE_TOLERANCE`]）。
pub fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= DECODE_TOLERANCE * b.abs().max(1.0)
}

/// sRGB（IEC 61966-2-1 で符号化した 0〜1 の値）の画像の L\*・C\*ab の統計（D65）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LabStats {
    /// L\* の平均。
    pub lstar_mean: f64,
    /// L\* の 5 パーセンタイル（最近接順位法）。
    pub lstar_p5: f64,
    /// L\* の 95 パーセンタイル。
    pub lstar_p95: f64,
    /// C\*ab の平均。
    pub chroma_mean: f64,
}

impl LabStats {
    /// L\* の幅（95 − 5 パーセンタイル）。
    pub fn lstar_width(&self) -> f64 {
        self.lstar_p95 - self.lstar_p5
    }
}

/// sRGB の画素（0〜1）の L\*・C\*ab の統計。画素がなければ `None`。
pub fn srgb_lab_stats(rgb: &[[f32; 3]]) -> Option<LabStats> {
    if rgb.is_empty() {
        return None;
    }
    let conv = crate::diff::LabConversion::encoded(genzo_color::RgbColorSpace::Srgb);
    let mut l = Vec::with_capacity(rgb.len());
    let mut chroma = 0.0f64;
    for &p in rgb {
        let lab = conv.to_lab(p);
        l.push(lab.l);
        chroma += lab.a.hypot(lab.b);
    }
    let mean = l.iter().sum::<f64>() / l.len() as f64;
    let p5 = crate::stats::percentile_nearest_rank(&mut l, 5.0)?;
    let p95 = crate::stats::percentile_nearest_rank(&mut l, 95.0)?;
    Some(LabStats {
        lstar_mean: mean,
        lstar_p5: p5,
        lstar_p95: p95,
        chroma_mean: chroma / rgb.len() as f64,
    })
}

/// JPEG のバイト列を 8bit の RGB の画素（0〜1）にする（カメラの埋め込み JPEG の比較用）。
pub fn decode_jpeg_srgb(bytes: &[u8]) -> Result<(u32, u32, Vec<[f32; 3]>), PrivateSampleError> {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg)
        .map_err(|e| err(format!("JPEG を読めません: {e}")))?
        .into_rgb8();
    let (w, h) = img.dimensions();
    let px = img
        .pixels()
        .map(|p| p.0.map(|v| f32::from(v) / 255.0))
        .collect();
    Ok((w, h, px))
}

/// 端の帯の指標（1 辺）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeBand {
    /// 外側 [`EDGE_OUTER_LINES`] 本の「隣の線との平均絶対差」の平均を、内側（[`EDGE_INNER_LINES`]）の
    /// 平均で割った比。詰め物の帯（値の繰り返し）があると小さくなる。
    pub flatness_ratio: f64,
    /// 外側 [`EDGE_OUTER_LINES`] 本の、隣の線との (R−G, B−G) の平均の差の最大を、内側の最大で割った比。
    /// 異常な線（色のずれた 1 本）があると大きくなる。
    pub color_step_ratio: f64,
}

/// 端の帯の指標で見る外側の線の数。
pub const EDGE_OUTER_LINES: usize = 8;
/// 端の帯の指標の基準にする内側の線（端からの番号の範囲）。
pub const EDGE_INNER_LINES: std::ops::Range<usize> = 100..200;

/// RGB（0〜1。行優先）の画像の 4 辺（上・下・左・右の順）の端の帯の指標。どちらかの辺が
/// [`EDGE_INNER_LINES`] の終わりより短ければ `None`。
pub fn edge_bands(width: usize, height: usize, rgb: &[[f32; 3]]) -> Option<[EdgeBand; 4]> {
    if width < EDGE_INNER_LINES.end + 1 || height < EDGE_INNER_LINES.end + 1 {
        return None;
    }
    let px = |x: usize, y: usize| rgb[y * width + x];
    // 辺 side の k 本目の線の i 番目の画素。
    let line_px = |side: usize, k: usize, i: usize| match side {
        0 => px(i, k),
        1 => px(i, height - 1 - k),
        2 => px(k, i),
        _ => px(width - 1 - k, i),
    };
    let line_len = |side: usize| if side < 2 { width } else { height };
    let mut out = [EdgeBand {
        flatness_ratio: 0.0,
        color_step_ratio: 0.0,
    }; 4];
    for (side, band) in out.iter_mut().enumerate() {
        let n = line_len(side);
        // 線 k と k + 1 の平均絶対差（8bit 換算）と、(R−G, B−G) の平均の差。
        let diff = |k: usize| -> (f64, f64) {
            let mut sad = 0.0f64;
            let (mut rg0, mut bg0, mut rg1, mut bg1) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for i in 0..n {
                let a = line_px(side, k, i);
                let b = line_px(side, k + 1, i);
                for c in 0..3 {
                    sad += f64::from((a[c] - b[c]).abs());
                }
                rg0 += f64::from(a[0] - a[1]);
                bg0 += f64::from(a[2] - a[1]);
                rg1 += f64::from(b[0] - b[1]);
                bg1 += f64::from(b[2] - b[1]);
            }
            let nn = n as f64;
            let step = ((rg0 - rg1) / nn).abs().max(((bg0 - bg1) / nn).abs());
            (sad / (nn * 3.0) * 255.0, step * 255.0)
        };
        let outer: Vec<(f64, f64)> = (0..EDGE_OUTER_LINES).map(diff).collect();
        let inner: Vec<(f64, f64)> = EDGE_INNER_LINES.map(diff).collect();
        let mean = |v: &[(f64, f64)]| v.iter().map(|d| d.0).sum::<f64>() / v.len() as f64;
        let max_step = |v: &[(f64, f64)]| v.iter().map(|d| d.1).fold(0.0, f64::max);
        band.flatness_ratio = mean(&outer) / mean(&inner).max(1e-12);
        band.color_step_ratio = max_step(&outer) / max_step(&inner).max(1e-6);
    }
    Some(out)
}

/// 端の帯がないとみなす平らさの比の下限（**仮置き**）。
///
/// 根拠（実機の 2 枚。既定の現像を長辺 1616 に縮小した出力。2026-10-10 に計測）: α7 IV の APS-C の
/// ARW を既定の切り抜きを外して現像すると、詰め物の帯のある辺（向き 8 の上端）の比は 0.012。帯の
/// ない辺は、既定の切り抜きを使った現像の 8 辺で 0.630〜1.156。その間の値にした。
pub const MIN_EDGE_FLATNESS_RATIO: f64 = 0.25;
/// 端の異常な線がないとみなす色の段差の比の上限（**仮置き**）。
///
/// 根拠（[`MIN_EDGE_FLATNESS_RATIO`] と同じ計測）: α7 IV の APS-C の ARW を既定の切り抜きを外して
/// 現像すると、異常な行（B が高い）のある辺（向き 8 の右端）の比は 2.85。既定の切り抜きを使った
/// 現像の 8 辺では 0.27〜1.36。その間の値にした（長辺 1616 では線が縮小で薄まるので、差は小さい）。
pub const MAX_EDGE_COLOR_STEP_RATIO: f64 = 2.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ExpectFile {
        ExpectFile {
            schema: SCHEMA,
            samples: vec![SampleExpect {
                id: "a7c-cmp-01".to_owned(),
                sha256: "3d7f703cc1114eb994834b8d837f5b9eeb39d35a845cff375da177afe7c1504d"
                    .to_owned(),
                camera_model: Some("ILCE-7C".to_owned()),
                decode: Some(DecodeExpect {
                    unpack_function: "sony_arw2_load_raw()".to_owned(),
                    raw_size: [6048, 4024],
                    active_size: [6024, 4024],
                    default_crop: Some([12, 12, 6000, 4000]),
                    orientation: 1,
                    cfa: "RGGB".to_owned(),
                    black_level: [512.0; 4],
                    white_level: 16383.0,
                    saturation_level: 16303.645,
                    sony_raw_file_type: Some(2),
                    data_end: 24_955_648,
                }),
                saturation: None,
                render: None,
            }],
        }
    }

    #[test]
    fn allowed_values_pass_and_others_are_rejected() {
        let f = sample();
        check_privacy(&f).unwrap();
        let text = serde_json::to_string_pretty(&f).unwrap();
        check_privacy_text(&text).unwrap();
        assert_eq!(serde_json::from_str::<ExpectFile>(&text).unwrap(), f);

        let mut bad = f.clone();
        bad.samples[0].camera_model = Some("ILCE-7C SN 1234567890123456789".to_owned());
        assert!(check_privacy(&bad).is_err());
        let mut bad = f.clone();
        bad.samples[0].id = "Holiday Photo".to_owned();
        assert!(check_privacy(&bad).is_err());
        let mut bad = f.clone();
        bad.samples[0].sha256 = "xyz".to_owned();
        assert!(check_privacy(&bad).is_err());
        let mut bad = f.clone();
        bad.samples.push(f.samples[0].clone());
        assert!(check_privacy(&bad).is_err());
        // 日付・パス・長い文字列は文字列のまま拒む。
        assert!(check_privacy_text(r#"{"x":"2024:05:01 12:00:00"}"#).is_err());
        assert!(check_privacy_text(r#"{"x":"2024-05-01"}"#).is_err());
        assert!(check_privacy_text(r#"{"x":"C:\\photos"}"#).is_err());
        assert!(check_privacy_text(r#"{"x":"/home/user"}"#).is_err());
        assert!(check_privacy_text(&format!(r#"{{"x":"{}"}}"#, "a".repeat(80))).is_err());
        // 決まった項目以外は読まない。
        let extra = text.replacen("\"schema\"", "\"gps\": 1, \"schema\"", 1);
        assert!(serde_json::from_str::<ExpectFile>(&extra).is_err());
    }

    #[test]
    fn models_and_ids() {
        assert!(is_allowed_model("ILCE-7M4"));
        assert!(is_allowed_model("ILCE-7C"));
        assert!(!is_allowed_model("ilce-7m4"));
        assert!(!is_allowed_model(""));
        assert!(!is_allowed_model("12345"));
        assert!(is_allowed_id("a7iv-apsc-cmp-01"));
        assert!(!is_allowed_id("a7iv_apsc"));
    }

    #[test]
    fn edge_band_metrics() {
        // 滑らかでない（縞のある）画像の端の 8 本を平らにすると、平らさの比が小さくなる。
        let (w, h) = (240usize, 230usize);
        let mut rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let v = ((x * 7 + y * 13) % 17) as f32 / 17.0;
                [v, v * 0.9, v * 0.8]
            })
            .collect();
        let b = edge_bands(w, h, &rgb).unwrap();
        for e in b {
            assert!(e.flatness_ratio > 0.5, "{e:?}");
        }
        for y in 0..EDGE_OUTER_LINES + 1 {
            for x in 0..w {
                rgb[y * w + x] = rgb[(EDGE_OUTER_LINES + 1) * w + x];
            }
        }
        let b = edge_bands(w, h, &rgb).unwrap();
        assert!(b[0].flatness_ratio < 0.01, "{:?}", b[0]);
        // 右端の 1 列の B を上げると、色の段差の比が大きくなる。
        for y in 0..h {
            rgb[y * w + w - 1][2] += 0.2;
        }
        let b = edge_bands(w, h, &rgb).unwrap();
        assert!(
            b[3].color_step_ratio > MAX_EDGE_COLOR_STEP_RATIO,
            "{:?}",
            b[3]
        );
        assert!(edge_bands(100, 100, &rgb[..10_000]).is_none());
    }

    #[test]
    fn sample_dir_inside_the_workspace_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("samples");
        fs::create_dir(&inside).unwrap();
        let outside = tempfile::tempdir().unwrap();
        assert!(sample_dir_from(Some(inside.into_os_string()), root.path()).is_err());
        assert!(sample_dir_from(Some(root.path().into()), root.path()).is_err());
        let ok = sample_dir_from(Some(outside.path().into()), root.path()).unwrap();
        assert_eq!(ok, Some(fs::canonicalize(outside.path()).unwrap()));
        assert_eq!(sample_dir_from(None, root.path()).unwrap(), None);
        assert_eq!(sample_dir_from(Some("".into()), root.path()).unwrap(), None);
        assert!(sample_dir_from(Some(root.path().join("none").into()), root.path()).is_err());
    }

    #[test]
    fn hashes() {
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.arw");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), sha256_bytes(b"abc"));
        assert_eq!(list_raw_files(dir.path()).unwrap(), vec![p]);
        assert_eq!(short_hash(&sha256_bytes(b"abc")), "ba7816bf8f01");
    }
}
