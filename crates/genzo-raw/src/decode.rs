//! RAW の展開・メタデータ・埋め込みサムネイルの公開 API（LibRaw。機能フラグ `libraw`）。
//!
//! 機能フラグ `libraw` が無効のときも同じ関数があり、[`RawError::Unsupported`] を返す
//! （呼び出す側で `cfg` を書き分けなくてよいようにするため）。
//!
//! # 使い方の前提（04 の 1.2 節、AR-7）
//!
//! - これらの関数は **ワーカープロセスの中だけ** で呼ぶ（信頼できない入力のデコードを本体で
//!   行わない。SEC-05）。本体は、受け取ったバッファを [`RawImage::validate`] で検証してから使う。
//! - LibRaw のインスタンスは関数の呼び出しごとに作って破棄する。LibRaw のドキュメント
//!   （API-notes の「Thread safety」）では、1 つのインスタンスを 1 つのスレッドで使う限り、
//!   スレッドごとに別のインスタンスを並行して使えるとされている。そのため、これらの関数は
//!   複数のスレッドから同時に呼んでよい。ただし、スレッドセーフでない `libraw`
//!   （`LIBRAW_NOTHREADS` 付きのビルド。展開の関数が静的変数を使う）にリンクした build では、
//!   LibRaw の使用をプロセスの中で 1 つずつに制限する（並行しても速くならない。build.rs の
//!   説明）。並行して展開した結果が一致することはテストで確かめているが、実機の ARW での
//!   確認と性能の計測は PoC-2 で行う（AR-7）。
//! - ファイルは読み取り専用で開く（DATA-01）。LibRaw 0.21 の `open_file` は
//!   `std::filebuf::open(.., in | binary)` または `fopen(.., "rb")` で開く（LibRaw の
//!   `src/libraw_datastream.cpp`）。
//! - Windows のパス: ワイド文字（UTF-16）版の `open_file` を使う。LibRaw が Unicode のパスに
//!   対応せずにビルドされている場合（`LIBRAW_NOT_IMPLEMENTED`）は、ファイルを Rust で読み込んで
//!   メモリ上のデータとして LibRaw に渡す。MAX_PATH を超える長いパス（絶対パスが 248 単位以上）は
//!   拡張長パス（`\\?\`）にして渡し、それでも開けなければ同じく Rust で読み込んで渡す（指摘 F20。
//!   LibRaw の中の C の実行時ライブラリは接頭辞のない長いパスを開けないため）。
//!   **Windows でのビルドと動作は未確認**（PoC-2）。
//!
//! # 取り出す値と、LibRaw の値の意味（04 の 2.6 節）
//!
//! LibRaw 0.21 のドキュメント（API-datastruct）とソースで確かめた範囲で書く。実機の ARW
//! （α7 IV / α7C）での確認は PoC-2 で行う。
//!
//! | 値 | 取り出し方 |
//! |---|---|
//! | CFA の値 | `rawdata.raw_image` の有効画素の範囲（`left_margin`・`top_margin` から `width` × `height`）。LibRaw の処理（黒レベルの減算・スケーリングなど）はしない |
//! | CFA の配列 | 有効画素の (0, 0)〜(1, 1) の `COLOR(row, col)`。2 つの緑は位置で区別する（R と同じ行が G1）。8 行 × 2 列の範囲で 2 × 2 の周期であることを確かめる |
//! | 黒レベル | `black` ＋ `cblack[色の番号]` ＋ 繰り返しのパターン `cblack[6..]`（大きさ `cblack[4]` × `cblack[5]`）を CFA の 4 つの位置ごとに足す。パターンが 2 × 2 の周期で表せなければ対応外 |
//! | 白レベル | `maximum`（黒レベルを含む生の値）。`linear_max` は [`RawDetails::linear_max`] に記録するだけ |
//! | 撮影時の WB | `cam_mul` を CFA の位置の順に並べ、G1 を 1 にする（[`WbSource`]）。`pre_mul` は記録だけ |
//! | カメラ行列 | 下の「カメラ行列」 |
//! | 撮影情報 | `idata`・`lens`・`other`（ISO・シャッター速度・絞り・焦点距離・GPS）と `sizes.flip`。撮影日時は EXIF のコールバックで読んだ元の文字列（[`CaptureTimeSource`]） |
//!
//! **カメラ行列（[`RawImage::cam_xyz`]）**
//! - LibRaw の `cam_xyz[4][3]` は、ドキュメントでは「Camera RGB - XYZ conversion matrix」
//!   （機種ごとに一定）。ソース（`utils_dcraw.cpp` の `cam_xyz_coeff`）では XYZ → カメラ RGB の
//!   行列として使われる: `cam_rgb = cam_xyz × xyz_rgb`（`xyz_rgb` は sRGB（D65）→ XYZ）の各行を
//!   和が 1 になるように正規化し、その逆数を昼光の WB（`pre_mul`）、擬似逆行列を
//!   `rgb_cam`（WB を掛けたカメラ RGB → 線形の sRGB）とする。3 色の RAW では 4 行目は使わない。
//! - `cam_xyz` は LibRaw の内蔵の表（`adobe_coeff`。値は 10000 倍の整数）からだけ設定される。
//!   表の注記は「All matrices are from Adobe DNG Converter unless otherwise noted」で、
//!   dcraw 以来の慣習では D65 の行列（Adobe の ColorMatrix2）とされるが、**どの光源の行列かは
//!   LibRaw のドキュメントに明記がない。PoC-2 で、Adobe DNG Converter で変換した DNG の
//!   ColorMatrix2 と比べて確認する。**
//! - DNG では、LibRaw は ColorMatrix から `cam_xyz` を設定しない（`cmatrix`・`rgb_cam` の
//!   計算にだけ使う）。そのため、この crate は `dng_color` の CalibrationIlluminant が D65（21）の
//!   行列に CameraCalibration と AnalogBalance を掛けたもの（DNG の仕様の XYZ → カメラ）を使う
//!   （[`CamXyzSource`]）。D65 の行列がなければ LibRaw の表、それもなければ `None`。
//! - 04 の 2.5 節のとおり、パイプラインが使う行列は最終的にアプリのデータファイルに固定する。
//!   ここで取り出す行列は、その候補と確認用の値。
//!
//! # 制限・注意
//!
//! - LibRaw は幅か高さが 22 画素未満の RAW を RAW として扱わない（dcraw 以来の `identify` の
//!   判定）。そのような入力は [`RawError::Unsupported`]。
//! - 展開中に LibRaw が致命的でないデータの誤り（`LibRaw::derror()`。圧縮データの符号の誤り、
//!   記録方式のビット数を超える値など）を検出した場合、LibRaw は数えるだけで展開を続ける。
//!   [`decode_file`] はそれを [`RawError::Decode`] にする（[`DecodedRaw::into_verified_image`]）。
//!   [`decode_file_with_details`]・[`decode_bytes`] は PoC-2 の記録のため `Ok` を返し、数を
//!   [`RawDetails::data_error_count`] に記録する。ファイルの終わりを超える読み込みは LibRaw が
//!   中断し、[`RawError::Decode`] になる。
//! - LibRaw 0.21 のロスレス JPEG の展開（`lossless_dng_load_raw`、α7 IV などのロスレス圧縮の
//!   ARW に使う `sony_ljpeg_load_raw`）は、タイルの JPEG のヘッダが読めないとそこで展開を
//!   打ち切り、エラーもデータの誤りも返さない（`src/decoders/dng.cpp`・
//!   `src/decoders/decoders_libraw_dcrdefs.cpp`。残りの画素は展開されないまま）。この crate は
//!   それを検出できない（Compression を 7 に書き換えた合成 DNG で、`data_error_count` が 0 の
//!   まま `Ok` になることを確認した）。壊れたロスレス圧縮の ARW の扱いは PoC-2 で確認する。
//! - 埋め込みサムネイルのデータがファイルの終わりを超えている場合（LibRaw 0.21 は 16 KiB まで
//!   読み越して、足りない部分を初期化しないまま返す）は [`RawError::Decode`] にする
//!   （JPEG と 8bit のビットマップ。シムで確かめる）。
//! - メモリ上のデータ（[`decode_bytes`] など）では、LibRaw 0.21 の `LibRaw_buffer_datastream::read`
//!   が途中で切れた最後の値も読めた数に数えるため、最後の数バイトだけが欠けたデータを
//!   検出できない場合がある。ワーカーはファイルの API を使う。
//! - 余白（`left_margin`・`top_margin`）が奇数の場合、LibRaw は余白を偶数にそろえて CFA の配列を
//!   ずらすが、黒レベルの繰り返しのパターンはずらさない（LibRaw 0.21 の `src/utils/open.cpp`
//!   の `open_datastream`）。この crate は LibRaw と同じ添字でパターンを読むため、そのような RAW で
//!   CFA の位置ごとに黒レベルが違うと、位置がずれる（合成 DNG のテストで確認）。LibRaw 0.21 の
//!   ソースでは、繰り返しのパターン（`cblack[4]`・`cblack[5]`）を設定するのは DNG の
//!   BlackLevelRepeatDim と一部のタグ（0xf00a）だけで、Sony の ARW の黒レベルは `black` と
//!   `cblack[0..4]` で表されるため、対象機種の ARW には影響しない見込み（PoC-2 で確認する）。
//!   元の余白が奇数だったかは LibRaw の公開の値から分からないため、検出はしない。

use std::path::Path;

use genzo_model::PhotoMetadata;
use serde::{Deserialize, Serialize};

use crate::thumbnail::EmbeddedThumbnail;
use crate::{RawError, RawImage};

/// この build で LibRaw が使えるか（機能フラグ `libraw`）。
pub const LIBRAW_ENABLED: bool = cfg!(feature = "libraw");

/// LibRaw が無効な build で返すエラーのメッセージ。
#[cfg(not(feature = "libraw"))]
const LIBRAW_DISABLED: &str =
    "この build では LibRaw が無効です（genzo-raw の機能フラグ libraw を有効にしてください）";

/// 展開した RAW と、展開の詳細。
///
/// [`RawDetails::data_error_count`] が 0 より大きい場合、[`image`](Self::image) は
/// 壊れた画素（LibRaw が読み飛ばした・範囲外の値・初期化されていない値）を含みうる。
/// 本番の展開では [`into_verified_image`](Self::into_verified_image)（[`decode_file`] と同じ
/// 判定）で取り出す。
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedRaw {
    /// 展開した RAW（[`RawImage::validate`] 済み。データの誤りの有無は問わない）。
    pub image: RawImage,
    /// 展開の詳細（PoC-2 の記録用）。
    pub details: RawDetails,
}

impl DecodedRaw {
    /// LibRaw が展開中にデータの誤りを検出していなければ [`RawImage`] を返す。
    ///
    /// LibRaw 0.21 は、圧縮データの符号の誤りや、記録方式のビット数を超える値などを
    /// 「致命的でないデータの誤り」として数えるだけで展開を続ける（`LibRaw::derror()`。
    /// ファイルの終わりに達した場合だけ中断する）。そのような結果は壊れた画素を含みうるため、
    /// 1 件でもあれば [`RawError::Decode`] にする（SEC-05: 壊れたファイルはそのファイルだけを
    /// エラーにする）。正常な ARW で誤りが数えられないことは PoC-2 で確認する。
    pub fn into_verified_image(self) -> Result<RawImage, RawError> {
        let count = self.details.data_error_count;
        if count > 0 {
            return Err(RawError::Decode(format!(
                "RAW のデータに誤りがあります（LibRaw が {count} 件の誤りを検出。展開の関数 {}）",
                self.details.unpack_function
            )));
        }
        Ok(self.image)
    }
}

/// XYZ → カメラ RGB の行列（[`RawImage::cam_xyz`]）の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CamXyzSource {
    /// DNG の ColorMatrix1 / ColorMatrix2（CalibrationIlluminant が D65 のもの）に
    /// CameraCalibration と AnalogBalance を掛けたもの。
    DngColorMatrix {
        /// 1 なら ColorMatrix1、2 なら ColorMatrix2。
        index: u8,
        /// CalibrationIlluminant（EXIF の LightSource の値。21 = D65）。
        illuminant: u16,
    },
    /// LibRaw の内蔵の表（`imgdata.color.cam_xyz`）。どの光源の行列かは PoC-2 で確認する。
    #[serde(rename = "libraw_table")]
    LibRawTable,
    /// 使える行列がない（[`RawImage::cam_xyz`] は `None`）。
    None,
}

/// 撮影時の WB（[`RawImage::as_shot_wb`]）の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbSource {
    /// LibRaw の `cam_mul`（撮影時の WB）。
    AsShot,
    /// `cam_mul` が使えないため、LibRaw の `pre_mul`（昼光の WB）を使った。
    Daylight,
    /// どちらも使えないため、(1, 1, 1, 1) にした。
    Unity,
}

/// 撮影日時（[`PhotoMetadata::capture`]）の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTimeSource {
    /// EXIF の DateTimeOriginal（と SubSecTimeOriginal・OffsetTimeOriginal）の元の文字列。
    ExifDateTimeOriginal,
    /// LibRaw の `timestamp` をローカル時刻に戻した文字列（オフセットなし）。
    LibRawTimestamp,
    /// 撮影日時がない。
    None,
}

/// DNG の ColorMatrix（記録用）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DngColorMatrixInfo {
    /// 1 なら ColorMatrix1、2 なら ColorMatrix2。
    pub index: u8,
    /// CalibrationIlluminant（EXIF の LightSource の値。記録がなければ LibRaw の既定値）。
    pub illuminant: u16,
    /// XYZ → カメラの行列（CameraCalibration などを掛ける前の値）。
    pub matrix: [[f32; 3]; 3],
}

/// 展開の詳細（PoC-2 で記録する値。04 の 2.2 節の「実際のバッファの寸法・余白」、2.6 節の
/// 「LibRaw の各行列の意味」の確認用）。
///
/// LibRaw の値をそのまま記録する。パイプラインはこれを使わず、[`RawImage`] だけを使う。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawDetails {
    /// デコーダの識別子（[`decoder_id`]。例: `"libraw-0.21.2"`）。
    pub decoder_id: String,
    /// LibRaw が展開に使った関数の名前（例: `"sony_arw2_load_raw"`。記録方式の確認用）。
    pub unpack_function: String,
    /// 展開中に LibRaw が検出した、致命的でないデータの誤りの数（0 より大きければ、画素は
    /// 壊れている可能性が高い。[`DecodedRaw::into_verified_image`]）。
    pub data_error_count: i32,
    /// DNG か。
    pub is_dng: bool,
    /// RAW のバッファの幅（余白を含む）。
    pub raw_width: u32,
    /// RAW のバッファの高さ（余白を含む）。
    pub raw_height: u32,
    /// 有効画素の左の余白。
    pub left_margin: u32,
    /// 有効画素の上の余白。
    pub top_margin: u32,
    /// RAW のバッファの 1 行のバイト数。
    pub raw_pitch_bytes: u32,
    /// RAW のデータのビット数（LibRaw の `raw_bps`）。
    pub raw_bits_per_sample: u32,
    /// LibRaw の `black`。
    pub libraw_black: u32,
    /// LibRaw の `cblack[0..4]`（LibRaw の色の番号の順）。
    pub libraw_cblack: [u32; 4],
    /// 黒レベルの繰り返しのパターンの大きさ（`cblack[4]`、`cblack[5]`）。
    pub black_pattern_size: [u32; 2],
    /// LibRaw の `linear_max`（チャンネルごとの線形の上限。記録がなければ 0）。
    pub linear_max: [i64; 4],
    /// LibRaw の `cam_mul`（撮影時の WB。正規化する前。LibRaw の色の番号の順）。
    pub cam_mul: [f32; 4],
    /// LibRaw の `pre_mul`（昼光の WB。LibRaw の色の番号の順）。
    pub pre_mul: [f32; 4],
    /// LibRaw の `cam_xyz`（4 × 3。内蔵の表から設定される）。
    pub libraw_cam_xyz: [[f32; 3]; 4],
    /// LibRaw の `rgb_cam`（カメラ RGB → sRGB。dcraw の `cam_xyz_coeff` で、WB を掛けた
    /// カメラ RGB から線形の sRGB への行列として計算される）。
    pub rgb_cam: [[f32; 4]; 3],
    /// DNG の ColorMatrix（DNG のときだけ）。
    pub dng_color_matrices: Vec<DngColorMatrixInfo>,
    /// [`RawImage::cam_xyz`] の出どころ。
    pub cam_xyz_source: CamXyzSource,
    /// [`RawImage::as_shot_wb`] の出どころ。
    pub wb_source: WbSource,
    /// LibRaw の `timestamp`（UNIX 時刻。LibRaw が日時をプロセスのローカルタイムゾーンで
    /// 解釈した値なので、UTC として使ってはいけない）。
    pub timestamp: Option<i64>,
    /// 撮影日時の出どころ。
    pub capture_time_source: CaptureTimeSource,
    /// LibRaw が正規化したメーカー名。
    pub libraw_make: Option<String>,
    /// LibRaw が正規化した機種名。
    pub libraw_model: Option<String>,
}

/// 読み込み元。
#[cfg_attr(not(feature = "libraw"), allow(dead_code))]
pub(crate) enum Source<'a> {
    /// ファイルのパス。
    Path(&'a Path),
    /// メモリ上のデータ。
    Bytes(&'a [u8]),
}

/// 実行時に読み込んだ LibRaw の版（`LibRaw::version()`。例: `"0.21.2-Release"`）。
///
/// LibRaw が無効な build では `None`。
pub fn libraw_version() -> Option<&'static str> {
    #[cfg(feature = "libraw")]
    {
        Some(crate::libraw::version())
    }
    #[cfg(not(feature = "libraw"))]
    {
        None
    }
}

/// RAW のデコーダの識別子（04 の 2.5 節の `render_deps.raw_decoder`。例: `"libraw-0.21.2"`）。
///
/// 実行時に読み込んだ LibRaw の版（major.minor.patch）から作る。展開には LibRaw の処理の
/// 設定（デモザイクなど）を使わず、RAW のデータと黒レベルなどをそのまま取り出すだけなので、
/// 設定の項目は含めない。シムの取り出し方を変えて展開の結果（段階 A0 の出力）が変わる場合は、
/// この識別子に印を加える（未定。変えるときに決める）。
///
/// LibRaw が無効な build では `None`。
pub fn decoder_id() -> Option<String> {
    #[cfg(feature = "libraw")]
    {
        Some(crate::libraw::decoder_id())
    }
    #[cfg(not(feature = "libraw"))]
    {
        None
    }
}

/// RAW のファイルを展開する（段階 A0。04 の 2.2 節）。
///
/// 有効画素の範囲の CFA の値・黒レベル・白レベル・撮影時の WB・カメラ行列・撮影情報を取り出し、
/// [`RawImage::validate`] で検証して返す。3 色・2 × 2 のベイヤー配列以外は
/// [`RawError::Unsupported`]。LibRaw が展開中にデータの誤りを検出した場合は
/// [`RawError::Decode`]（[`DecodedRaw::into_verified_image`]）。
pub fn decode_file(path: impl AsRef<Path>) -> Result<RawImage, RawError> {
    decode_file_with_details(path)?.into_verified_image()
}

/// [`decode_file`] と同じだが、展開の詳細（[`RawDetails`]）も返す。
///
/// PoC-2 の記録のため、LibRaw がデータの誤りを検出しても `Ok` を返し、その数を
/// [`RawDetails::data_error_count`] に入れる。結果を使う場合は
/// [`DecodedRaw::into_verified_image`] で確かめる。
pub fn decode_file_with_details(path: impl AsRef<Path>) -> Result<DecodedRaw, RawError> {
    decode_source(Source::Path(path.as_ref()))
}

/// メモリ上の RAW のデータを展開する（[`decode_file_with_details`] と同じ処理。データの誤りの
/// 扱いも同じ）。
pub fn decode_bytes(data: &[u8]) -> Result<DecodedRaw, RawError> {
    decode_source(Source::Bytes(data))
}

/// RAW のファイルの撮影情報を読む（RAW のデータは展開しないため速い）。
pub fn read_metadata(path: impl AsRef<Path>) -> Result<PhotoMetadata, RawError> {
    metadata_source(Source::Path(path.as_ref()))
}

/// メモリ上の RAW のデータの撮影情報を読む。
pub fn read_metadata_bytes(data: &[u8]) -> Result<PhotoMetadata, RawError> {
    metadata_source(Source::Bytes(data))
}

/// RAW のファイルの埋め込みサムネイル（プレビュー）を取り出す。
///
/// 埋め込みサムネイルがない、または対応していない形式（16bit のビットマップなど）の場合は
/// [`RawError::Unsupported`]。データがファイルの終わりを超えている（途中で切れている）場合は
/// [`RawError::Decode`]。複数ある場合は LibRaw が選んだもの（通常は最も大きいもの）。
pub fn extract_thumbnail(path: impl AsRef<Path>) -> Result<EmbeddedThumbnail, RawError> {
    thumbnail_source(Source::Path(path.as_ref()))
}

/// メモリ上の RAW のデータの埋め込みサムネイルを取り出す。
pub fn extract_thumbnail_bytes(data: &[u8]) -> Result<EmbeddedThumbnail, RawError> {
    thumbnail_source(Source::Bytes(data))
}

#[cfg(feature = "libraw")]
fn decode_source(source: Source<'_>) -> Result<DecodedRaw, RawError> {
    crate::libraw::decode(source)
}

#[cfg(feature = "libraw")]
fn metadata_source(source: Source<'_>) -> Result<PhotoMetadata, RawError> {
    crate::libraw::metadata(source)
}

#[cfg(feature = "libraw")]
fn thumbnail_source(source: Source<'_>) -> Result<EmbeddedThumbnail, RawError> {
    crate::libraw::thumbnail(source)
}

#[cfg(not(feature = "libraw"))]
fn decode_source(_source: Source<'_>) -> Result<DecodedRaw, RawError> {
    Err(RawError::Unsupported(LIBRAW_DISABLED.to_owned()))
}

#[cfg(not(feature = "libraw"))]
fn metadata_source(_source: Source<'_>) -> Result<PhotoMetadata, RawError> {
    Err(RawError::Unsupported(LIBRAW_DISABLED.to_owned()))
}

#[cfg(not(feature = "libraw"))]
fn thumbnail_source(_source: Source<'_>) -> Result<EmbeddedThumbnail, RawError> {
    Err(RawError::Unsupported(LIBRAW_DISABLED.to_owned()))
}

#[cfg(all(test, not(feature = "libraw")))]
mod tests_without_libraw {
    use super::*;

    #[test]
    fn functions_report_that_libraw_is_disabled() {
        const { assert!(!LIBRAW_ENABLED) };
        assert_eq!(libraw_version(), None);
        assert_eq!(decoder_id(), None);
        assert!(matches!(
            decode_file("/nonexistent.arw"),
            Err(RawError::Unsupported(_))
        ));
        assert!(matches!(decode_bytes(&[]), Err(RawError::Unsupported(_))));
        assert!(matches!(
            read_metadata("/nonexistent.arw"),
            Err(RawError::Unsupported(_))
        ));
        assert!(matches!(
            extract_thumbnail_bytes(&[1, 2, 3]),
            Err(RawError::Unsupported(_))
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CfaPattern;

    fn decoded(data_error_count: i32) -> DecodedRaw {
        let image = RawImage {
            width: 2,
            height: 2,
            cfa: CfaPattern::RGGB,
            data: vec![600; 4],
            black_level: [512.0; 4],
            white_level: 16383.0,
            as_shot_wb: [2.0, 1.0, 1.5, 1.0],
            cam_xyz: None,
            metadata: PhotoMetadata::default(),
        };
        let details = RawDetails {
            decoder_id: "libraw-0.21.2".to_owned(),
            unpack_function: "sony_arw_load_raw".to_owned(),
            data_error_count,
            is_dng: false,
            raw_width: 2,
            raw_height: 2,
            left_margin: 0,
            top_margin: 0,
            raw_pitch_bytes: 4,
            raw_bits_per_sample: 14,
            libraw_black: 512,
            libraw_cblack: [0; 4],
            black_pattern_size: [0; 2],
            linear_max: [0; 4],
            cam_mul: [2.0, 1.0, 1.5, 0.0],
            pre_mul: [2.0, 1.0, 1.5, 0.0],
            libraw_cam_xyz: [[0.0; 3]; 4],
            rgb_cam: [[0.0; 4]; 3],
            dng_color_matrices: Vec::new(),
            cam_xyz_source: CamXyzSource::None,
            wb_source: WbSource::AsShot,
            timestamp: None,
            capture_time_source: CaptureTimeSource::None,
            libraw_make: None,
            libraw_model: None,
        };
        DecodedRaw { image, details }
    }

    /// LibRaw が展開中にデータの誤りを検出した結果（壊れた画素を含みうる）は、
    /// [`decode_file`] の経路（[`DecodedRaw::into_verified_image`]）でエラーにする。
    /// レビューで見つけた不具合（誤りの数を捨てて Ok を返していた）の再現テスト。
    #[test]
    fn data_errors_are_rejected_by_verified_image() {
        let ok = decoded(0);
        assert_eq!(ok.clone().into_verified_image().unwrap(), ok.image);
        for count in [1, 7, i32::MAX] {
            let r = decoded(count).into_verified_image();
            match r {
                Err(RawError::Decode(msg)) => {
                    assert!(msg.contains(&count.to_string()), "{msg}");
                    assert!(msg.contains("sony_arw_load_raw"), "{msg}");
                }
                other => panic!("{count}: {other:?}"),
            }
        }
    }

    #[test]
    fn sources_serialize_with_stable_names() {
        let s = serde_json::to_string(&CamXyzSource::DngColorMatrix {
            index: 1,
            illuminant: 21,
        })
        .unwrap();
        assert_eq!(
            s,
            r#"{"kind":"dng_color_matrix","index":1,"illuminant":21}"#
        );
        assert_eq!(
            serde_json::to_string(&CamXyzSource::LibRawTable).unwrap(),
            r#"{"kind":"libraw_table"}"#
        );
        assert_eq!(
            serde_json::to_string(&WbSource::Daylight).unwrap(),
            "\"daylight\""
        );
        assert_eq!(
            serde_json::to_string(&CaptureTimeSource::ExifDateTimeOriginal).unwrap(),
            "\"exif_date_time_original\""
        );
    }
}
