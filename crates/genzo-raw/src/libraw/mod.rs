//! LibRaw の FFI（機能フラグ `libraw`）。
//!
//! C++ のシム（`src/shim/genzo_libraw_shim.cpp`）を通して LibRaw を呼ぶ。シムは LibRaw の
//! C++ API の値を平坦な C の構造体にコピーして返すだけで、値の解釈は [`crate::convert`] で行う。
//!
//! - [`Processor`] は LibRaw のインスタンス 1 つを持ち、作ったスレッドの中だけで使う
//!   （生のポインタを持つため `Send` / `Sync` ではない）。破棄するときに LibRaw のインスタンスを
//!   解放する。公開 API（[`crate::decode`]）は呼び出しごとに作って破棄する（AR-7）。
//! - スレッドセーフでない LibRaw（`libraw`。build.rs の cfg `genzo_libraw_nothreads`）に
//!   リンクしている場合は、LibRaw の使用をプロセスの中で 1 つずつに制限する（[`lock_libraw`]）。
//! - シムと Rust の構造体の大きさと、ビルド時の LibRaw のヘッダと実行時のライブラリの版
//!   （major.minor）が一致することを、最初に使うときに確かめる。一致しなければ
//!   [`RawError::Unsupported`]（構造体のレイアウトが違う LibRaw を使うと未定義動作になるため）。

pub(crate) mod ffi;

use std::ffi::{CStr, c_char, c_void};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::OnceLock;

use genzo_model::PhotoMetadata;

use crate::RawError;
use crate::convert::{
    self, ColorSnapshot, DecoderInfo, DngColorSnapshot, GpsSnapshot, MetaSnapshot, SizesSnapshot,
};
use crate::decode::{DecodedRaw, Source};
use crate::thumbnail::{EmbeddedThumbnail, MAX_THUMBNAIL_BYTES};

use ffi::{code, libraw_code};

/// LibRaw を使う間（インスタンスを作ってから破棄するまで）保持するガード。
///
/// スレッドセーフでない LibRaw（`LIBRAW_NOTHREADS` 付きでビルドされた `libraw`）は、展開の
/// 関数が静的変数を使うため、別のインスタンスでも同時に使うとデータが壊れる（build.rs の説明）。
/// その場合だけ、プロセスの中で共通のロックを持つ。
struct LibRawGuard {
    #[cfg(genzo_libraw_nothreads)]
    _lock: std::sync::MutexGuard<'static, ()>,
}

/// [`LibRawGuard`] を得る（スレッドセーフな LibRaw では何もしない）。
fn lock_libraw() -> LibRawGuard {
    #[cfg(genzo_libraw_nothreads)]
    {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        // ロックで守るのは LibRaw の静的変数だけで、Rust の値は持たないため、
        // 以前の保持者がパニックしていても使い続けてよい。
        LibRawGuard {
            _lock: LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        }
    }
    #[cfg(not(genzo_libraw_nothreads))]
    {
        LibRawGuard {}
    }
}

/// `LibRaw::versionNumber()` の値を (major, minor, patch) に分ける
/// （`LIBRAW_MAKE_VERSION(major, minor, patch)` = `(major << 16) | (minor << 8) | patch`）。
const fn split_version(v: i32) -> (i32, i32, i32) {
    ((v >> 16) & 0xff, (v >> 8) & 0xff, v & 0xff)
}

/// 実行時の LibRaw の版の文字列（`LibRaw::version()`）。
pub(crate) fn version() -> &'static str {
    let p = ffi::genzo_lr_version();
    if p.is_null() {
        return "";
    }
    // SAFETY: LibRaw::version() は静的な NUL 終端の文字列（LIBRAW_VERSION_STR）を返す。
    unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
}

/// デコーダの識別子（`"libraw-{major}.{minor}.{patch}"`）。
pub(crate) fn decoder_id() -> String {
    let (major, minor, patch) = split_version(ffi::genzo_lr_version_number());
    format!("libraw-{major}.{minor}.{patch}")
}

/// シムと Rust の構造体の大きさ、ヘッダとライブラリの版を確かめる（結果は 1 回だけ計算する）。
fn check_abi() -> Result<(), RawError> {
    static CHECK: OnceLock<Result<(), String>> = OnceLock::new();
    CHECK
        .get_or_init(|| {
            let rc = ffi::genzo_lr_abi_check(
                size_of::<ffi::Sizes>(),
                size_of::<ffi::Color>(),
                size_of::<ffi::Meta>(),
                size_of::<ffi::Thumb>(),
            );
            if rc != 0 {
                return Err(
                    "LibRaw のシムと Rust の構造体の大きさが一致しません（シムの不具合）".to_owned(),
                );
            }
            let header = split_version(ffi::genzo_lr_header_version_number());
            let runtime = split_version(ffi::genzo_lr_version_number());
            if (header.0, header.1) != (runtime.0, runtime.1) {
                return Err(format!(
                    "ビルド時の LibRaw のヘッダ（{}.{}.{}）と実行時のライブラリ（{}.{}.{}）の版が違います",
                    header.0, header.1, header.2, runtime.0, runtime.1, runtime.2
                ));
            }
            Ok(())
        })
        .clone()
        .map_err(RawError::Unsupported)
}

/// LibRaw のエラーコードの英語の説明。
fn libraw_message(rc: i32) -> String {
    let p = ffi::genzo_lr_strerror(rc);
    if p.is_null() {
        return String::new();
    }
    // SAFETY: LibRaw::strerror() は静的な NUL 終端の文字列を返す。
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// シム・LibRaw の戻り値をエラーに変換する。`what` は何をしていたか（日本語）。
fn check(rc: i32, what: &str) -> Result<(), RawError> {
    if rc == libraw_code::SUCCESS {
        return Ok(());
    }
    if rc > 0 {
        // LibRaw の規約: 正の値はシステムコールのエラー（errno）。
        return Err(RawError::Io(std::io::Error::from_raw_os_error(rc)));
    }
    let detail = |msg: &str| format!("{what}: {msg}（LibRaw {rc}: {}）", libraw_message(rc));
    let err = match rc {
        libraw_code::FILE_UNSUPPORTED => {
            RawError::Unsupported(detail("LibRaw が対応していない形式です"))
        }
        libraw_code::NO_THUMBNAIL | libraw_code::REQUEST_FOR_NONEXISTENT_THUMBNAIL => {
            RawError::Unsupported(detail("埋め込みサムネイルがありません"))
        }
        libraw_code::UNSUPPORTED_THUMBNAIL => {
            RawError::Unsupported(detail("対応していない形式の埋め込みサムネイルです"))
        }
        libraw_code::NOT_IMPLEMENTED => {
            RawError::Unsupported(detail("LibRaw がこの処理に対応していません"))
        }
        libraw_code::IO_ERROR => RawError::Decode(detail(
            "読み込みに失敗しました（ファイルが途中で切れている・壊れている可能性があります）",
        )),
        libraw_code::DATA_ERROR => RawError::Decode(detail("RAW のデータが壊れています")),
        libraw_code::UNSUFFICIENT_MEMORY => RawError::Decode(detail("メモリが足りません")),
        libraw_code::TOO_BIG => RawError::Decode(detail("RAW のデータが大きすぎます")),
        libraw_code::MEMPOOL_OVERFLOW => {
            RawError::Decode(detail("LibRaw の内部のメモリの上限を超えました"))
        }
        libraw_code::BAD_CROP => RawError::Decode(detail("切り抜きの範囲が不正です")),
        libraw_code::CANCELLED_BY_CALLBACK => RawError::Decode(detail("取り消されました")),
        libraw_code::UNSPECIFIED_ERROR
        | libraw_code::REQUEST_FOR_NONEXISTENT_IMAGE
        | libraw_code::OUT_OF_ORDER_CALL
        | libraw_code::INPUT_CLOSED => RawError::Decode(detail("LibRaw の処理に失敗しました")),
        code::NO_BAYER_DATA => RawError::Unsupported(format!(
            "{what}: ベイヤー配列の u16 のデータがありません（浮動小数点・3 色などの RAW）"
        )),
        code::BAD_GEOMETRY => RawError::Decode(format!(
            "{what}: 有効画素の範囲が RAW のバッファの外にあります"
        )),
        code::BAD_ALLOC => RawError::Decode(format!("{what}: メモリが足りません")),
        code::EXCEPTION => RawError::Decode(format!("{what}: LibRaw の処理中に例外が発生しました")),
        code::NO_THUMB_DATA => {
            RawError::Decode(format!("{what}: 埋め込みサムネイルのデータがありません"))
        }
        code::THUMB_TRUNCATED => RawError::Decode(format!(
            "{what}: 埋め込みサムネイルのデータがファイルの終わりを超えています（途中で切れている可能性があります）"
        )),
        code::BUFFER_SIZE | code::NULL_ARG | code::ABI_MISMATCH => {
            RawError::Decode(format!("{what}: シムの呼び出しが不正です（{rc}）"))
        }
        _ => RawError::Decode(detail("LibRaw の処理に失敗しました")),
    };
    Err(err)
}

/// C の固定長の文字列（NUL 終端）を、前後の空白を除いた文字列にする。空なら `None`。
///
/// EXIF の文字列は UTF-8 とは限らないため、不正なバイトは置き換える。
fn c_string(buf: &[c_char]) -> Option<String> {
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| u8::from_ne_bytes(c.to_ne_bytes()))
        .collect();
    let s = String::from_utf8_lossy(&bytes);
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    (!t.is_empty()).then(|| t.to_owned())
}

/// `c_char` 1 つを `u8` にする。
fn c_byte(c: c_char) -> u8 {
    u8::from_ne_bytes(c.to_ne_bytes())
}

/// LibRaw のインスタンス（シムの `genzo_lr`）。
///
/// `'a` はメモリ上のデータを開いたときの、そのデータの借用の期間。
pub(crate) struct Processor<'a> {
    handle: NonNull<ffi::GenzoLr>,
    /// Windows で、ファイルを Rust で読み込んで渡した場合のデータ（`handle` より後に解放する）。
    _owned: Option<Vec<u8>>,
    _borrow: PhantomData<&'a [u8]>,
}

impl Drop for Processor<'_> {
    fn drop(&mut self) {
        // SAFETY: handle は genzo_lr_new が返した有効なポインタで、ここで 1 回だけ解放する。
        // LibRaw が参照するデータ（_owned・借用したデータ）は、この後に解放される。
        unsafe { ffi::genzo_lr_free(self.handle.as_ptr()) };
    }
}

impl<'a> Processor<'a> {
    /// インスタンスを作る。
    fn new() -> Result<Self, RawError> {
        check_abi()?;
        let handle = NonNull::new(ffi::genzo_lr_new()).ok_or_else(|| {
            RawError::Decode("LibRaw のインスタンスを作れません（メモリが足りません）".to_owned())
        })?;
        Ok(Self {
            handle,
            _owned: None,
            _borrow: PhantomData,
        })
    }

    fn ptr(&mut self) -> *mut ffi::GenzoLr {
        self.handle.as_ptr()
    }

    /// 読み込み元を開いてメタデータを解析する。
    pub(crate) fn open(source: Source<'a>) -> Result<Self, RawError> {
        match source {
            Source::Path(path) => Self::open_path(path),
            Source::Bytes(data) => Self::open_bytes(data),
        }
    }

    /// メモリ上のデータを開く。
    fn open_bytes(data: &'a [u8]) -> Result<Self, RawError> {
        if data.is_empty() {
            return Err(RawError::Unsupported("データが空です".to_owned()));
        }
        let mut p = Self::new()?;
        // SAFETY: data は 'a の間有効で、Processor<'a> はそれより長く生きない。
        // LibRaw は data を読むだけで、Processor を破棄するまで参照する。
        let rc = unsafe {
            ffi::genzo_lr_open_buffer(p.ptr(), data.as_ptr().cast::<c_void>(), data.len())
        };
        check(rc, "RAW のデータを開けません")?;
        Ok(p)
    }

    /// 所有するデータを開く（Windows で、ワイド文字のパスに LibRaw が対応していない場合と、
    /// 長いパスを LibRaw が開けない場合）。
    ///
    /// メモリから開くため、最後の 1 バイトだけが欠けたファイルは検出できない
    /// （[`crate::decode_bytes`] と同じ制限。tests/libraw.rs の `truncated_files_return_errors`）。
    #[cfg(any(windows, not(unix)))]
    fn open_owned(data: Vec<u8>) -> Result<Self, RawError> {
        if data.is_empty() {
            return Err(RawError::Unsupported("ファイルが空です".to_owned()));
        }
        let mut p = Self::new()?;
        // SAFETY: data の確保した領域は p._owned に移した後も動かず、p を破棄するとき
        // （LibRaw のインスタンスを解放した後）に解放される。
        let rc = unsafe {
            ffi::genzo_lr_open_buffer(p.ptr(), data.as_ptr().cast::<c_void>(), data.len())
        };
        p._owned = Some(data);
        check(rc, "RAW のファイルを開けません")?;
        Ok(p)
    }

    /// ファイルを開く（読み取り専用。DATA-01）。
    fn open_path(path: &Path) -> Result<Self, RawError> {
        // 存在しない・ファイルでない場合は、LibRaw に渡す前に入出力のエラーにする。
        let meta = std::fs::metadata(path)?;
        if !meta.is_file() {
            return Err(RawError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("通常のファイルではありません: {}", path.display()),
            )));
        }
        // 読み取りの権限がないなどで開けない場合も、LibRaw に渡す前に入出力のエラーにする
        // （LibRaw は開けないファイルを LIBRAW_IO_ERROR で返し、壊れたファイルと区別できない）。
        // 読み取り専用で開き、すぐに閉じる（DATA-01）。
        drop(std::fs::File::open(path)?);
        Self::open_path_native(path)
    }

    #[cfg(unix)]
    fn open_path_native(path: &Path) -> Result<Self, RawError> {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            RawError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "パスに NUL 文字が含まれています",
            ))
        })?;
        let mut p = Self::new()?;
        // SAFETY: c_path は NUL 終端の文字列で、呼び出しの間有効。LibRaw はパスをコピーする。
        let rc = unsafe { ffi::genzo_lr_open_file(p.ptr(), c_path.as_ptr()) };
        check(rc, "RAW のファイルを開けません")?;
        Ok(p)
    }

    #[cfg(windows)]
    fn open_path_native(path: &Path) -> Result<Self, RawError> {
        use std::os::windows::ffi::OsStrExt;
        let original: Vec<u16> = path.as_os_str().encode_wide().collect();
        if original.contains(&0) {
            return Err(RawError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "パスに NUL 文字が含まれています",
            )));
        }
        // MAX_PATH を超える長いパスは、LibRaw の中の C の実行時ライブラリが接頭辞なしでは開けず、
        // LIBRAW_IO_ERROR（「壊れている可能性」）になる。std の事前の確認は自動で `\\?\` を付けて
        // 通るため、長いパスは拡張長パスにしてから渡す（指摘 F20。crate::win_long_path）。
        // 短いパスは従来どおりそのまま渡す。
        let absolute: Vec<u16> = std::path::absolute(path)?
            .as_os_str()
            .encode_wide()
            .collect();
        let extended = crate::win_long_path::extended_length_path(&absolute);
        let is_long = extended.is_some();
        let mut wide = extended.unwrap_or(original);
        wide.push(0);
        let mut p = Self::new()?;
        // SAFETY: wide は NUL 終端の UTF-16 の文字列で、呼び出しの間有効。
        let rc = unsafe { ffi::genzo_lr_open_wfile(p.ptr(), wide.as_ptr()) };
        if rc == libraw_code::NOT_IMPLEMENTED || (is_long && rc == libraw_code::IO_ERROR) {
            // NOT_IMPLEMENTED: LibRaw が Unicode のパスに対応せずにビルドされている。
            // 長いパスの IO_ERROR: C の実行時ライブラリが拡張長パスを受け付けない場合に備える
            // （std では開けることを open_path で確かめてある。Windows の実機では未確認）。
            // どちらも Rust で読み込んで渡す。本当に壊れたファイルは、メモリから開くときに
            // LibRaw がエラーにする。
            drop(p);
            return Self::open_owned(std::fs::read(path)?);
        }
        check(rc, "RAW のファイルを開けません")?;
        Ok(p)
    }

    #[cfg(not(any(unix, windows)))]
    fn open_path_native(path: &Path) -> Result<Self, RawError> {
        Self::open_owned(std::fs::read(path)?)
    }

    /// RAW のデータを展開する。
    fn unpack(&mut self) -> Result<(), RawError> {
        // SAFETY: self.ptr() は有効なインスタンス。
        let rc = unsafe { ffi::genzo_lr_unpack(self.ptr()) };
        check(rc, "RAW のデータを展開できません")
    }

    fn sizes(&mut self) -> Result<SizesSnapshot, RawError> {
        let mut s = ffi::Sizes::zeroed();
        // SAFETY: s はシムの genzo_lr_sizes と同じレイアウト（check_abi で大きさを確認済み）。
        let rc = unsafe { ffi::genzo_lr_get_sizes(self.ptr(), &mut s) };
        check(rc, "寸法を取得できません")?;
        Ok(SizesSnapshot {
            raw_width: s.raw_width,
            raw_height: s.raw_height,
            width: s.width,
            height: s.height,
            top_margin: s.top_margin,
            left_margin: s.left_margin,
            raw_pitch: s.raw_pitch,
            filters: s.filters,
            colors: s.colors,
            dng_version: s.dng_version,
            is_foveon: s.is_foveon != 0,
            fuji_rotated: s.fuji_rotated != 0,
            cfa: s.cfa,
            cdesc: c_string(&s.cdesc).unwrap_or_default(),
        })
    }

    fn color(&mut self) -> Result<ColorSnapshot, RawError> {
        // 約 16 KB あるのでヒープに置く。
        let mut c = Box::new(ffi::Color::zeroed());
        // SAFETY: c はシムの genzo_lr_color と同じレイアウト（check_abi で大きさを確認済み）。
        let rc = unsafe { ffi::genzo_lr_get_color(self.ptr(), &mut *c) };
        check(rc, "色の情報を取得できません")?;
        let dng = |d: &ffi::DngColor| DngColorSnapshot {
            illuminant: d.illuminant,
            color_matrix: d.colormatrix,
            calibration: d.calibration,
        };
        Ok(ColorSnapshot {
            black: c.black,
            maximum: c.maximum,
            cblack: c.cblack.to_vec(),
            linear_max: c.linear_max,
            cam_mul: c.cam_mul,
            pre_mul: c.pre_mul,
            cam_xyz: c.cam_xyz,
            rgb_cam: c.rgb_cam,
            dng_color: [dng(&c.dng_color[0]), dng(&c.dng_color[1])],
            dng_analog_balance: c.dng_analogbalance,
            raw_bps: c.raw_bps,
        })
    }

    fn meta(&mut self) -> Result<MetaSnapshot, RawError> {
        let mut m = Box::new(ffi::Meta::zeroed());
        // SAFETY: m はシムの genzo_lr_meta と同じレイアウト（check_abi で大きさを確認済み）。
        let rc = unsafe { ffi::genzo_lr_get_meta(self.ptr(), &mut *m) };
        check(rc, "撮影情報を取得できません")?;
        Ok(MetaSnapshot {
            make: c_string(&m.make),
            model: c_string(&m.model),
            exif_make: c_string(&m.exif_make),
            exif_model: c_string(&m.exif_model),
            lens: c_string(&m.lens).filter(|l| !l.starts_with("----")),
            makernotes_lens: c_string(&m.makernotes_lens),
            iso_speed: m.iso_speed,
            shutter: m.shutter,
            aperture: m.aperture,
            focal_len: m.focal_len,
            timestamp: m.timestamp,
            timestamp_local: c_string(&m.timestamp_local),
            datetime_original: c_string(&m.datetime_original),
            subsec_time_original: c_string(&m.subsec_time_original),
            offset_time_original: c_string(&m.offset_time_original),
            gps: GpsSnapshot {
                parsed: m.gps_parsed != 0,
                latitude: m.gps_latitude,
                longitude: m.gps_longitude,
                latitude_ref: c_byte(m.gps_latref),
                longitude_ref: c_byte(m.gps_longref),
            },
            flip: m.flip,
            width: m.width,
            height: m.height,
        })
    }

    /// 有効画素の範囲の CFA の値をコピーする。
    fn copy_raw(&mut self, dst: &mut [u16]) -> Result<(), RawError> {
        // SAFETY: dst は dst.len() 個の u16 を書ける領域。シムは dst_len と寸法が一致する
        // ことを確かめてから、その範囲だけに書く。
        let rc = unsafe { ffi::genzo_lr_copy_raw(self.ptr(), dst.as_mut_ptr(), dst.len()) };
        check(rc, "RAW のデータを取り出せません")
    }

    fn error_count(&mut self) -> i32 {
        // SAFETY: self.ptr() は有効なインスタンス。
        unsafe { ffi::genzo_lr_error_count(self.ptr()) }.max(0)
    }

    fn decoder_name(&mut self) -> String {
        let mut buf: [c_char; 128] = [0; 128];
        // SAFETY: buf は 128 バイトの領域。シムは NUL 終端で切り詰めて書く。
        let rc = unsafe { ffi::genzo_lr_decoder_name(self.ptr(), buf.as_mut_ptr(), buf.len()) };
        if rc != 0 {
            return String::new();
        }
        c_string(&buf).unwrap_or_default()
    }

    /// 埋め込みサムネイルを読み込み、バイト列と形式を返す。
    fn thumbnail(&mut self) -> Result<(ffi::Thumb, Vec<u8>), RawError> {
        let mut t = ffi::Thumb::default();
        // SAFETY: t はシムの genzo_lr_thumb と同じレイアウト。
        let rc = unsafe { ffi::genzo_lr_unpack_thumb(self.ptr(), &mut t) };
        check(rc, "埋め込みサムネイルを読み込めません")?;
        let len = t.length as usize;
        if len == 0 || len > MAX_THUMBNAIL_BYTES {
            return Err(RawError::Decode(format!(
                "埋め込みサムネイルの大きさ {len} バイトが不正です（上限 {MAX_THUMBNAIL_BYTES}）"
            )));
        }
        let mut data = vec![0u8; len];
        // SAFETY: data は len バイトの領域。シムは len が LibRaw のサムネイルの長さと
        // 一致することを確かめてからコピーする。
        let rc = unsafe { ffi::genzo_lr_copy_thumb(self.ptr(), data.as_mut_ptr(), len) };
        check(rc, "埋め込みサムネイルを取り出せません")?;
        Ok((t, data))
    }
}

/// RAW を展開する（[`crate::decode_file_with_details`] の実体）。
pub(crate) fn decode(source: Source<'_>) -> Result<DecodedRaw, RawError> {
    // p より先に作り、p を破棄した後に解放する（ローカル変数は作った順の逆に破棄される）。
    let _guard = lock_libraw();
    let mut p = Processor::open(source)?;
    // 展開の前に、寸法の上限と対応している CFA かを確かめる（大きな確保と無駄な展開を避ける）。
    let before = p.sizes()?;
    convert::check_dimensions(&before)?;
    convert::cfa_from_libraw(&before)?;
    p.unpack()?;
    let sizes = p.sizes()?;
    convert::check_dimensions(&sizes)?;
    convert::cfa_from_libraw(&sizes)?;
    let color = p.color()?;
    let meta = p.meta()?;
    let pixels = sizes.width as usize * sizes.height as usize;
    let mut data = vec![0u16; pixels];
    p.copy_raw(&mut data)?;
    let decoder = DecoderInfo {
        decoder_id: decoder_id(),
        unpack_function: p.decoder_name(),
        data_error_count: p.error_count(),
    };
    convert::build_decoded(&sizes, &color, &meta, data, decoder)
}

/// 撮影情報を読む（[`crate::read_metadata`] の実体）。
pub(crate) fn metadata(source: Source<'_>) -> Result<PhotoMetadata, RawError> {
    // p より先に作り、p を破棄した後に解放する（ローカル変数は作った順の逆に破棄される）。
    let _guard = lock_libraw();
    let mut p = Processor::open(source)?;
    let meta = p.meta()?;
    Ok(convert::photo_metadata(&meta).0)
}

/// 埋め込みサムネイルを取り出す（[`crate::extract_thumbnail`] の実体）。
pub(crate) fn thumbnail(source: Source<'_>) -> Result<EmbeddedThumbnail, RawError> {
    // p より先に作り、p を破棄した後に解放する（ローカル変数は作った順の逆に破棄される）。
    let _guard = lock_libraw();
    let mut p = Processor::open(source)?;
    let (t, data) = p.thumbnail()?;
    convert::thumbnail_from_libraw(t.format, t.width, t.height, t.colors, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_and_versions_match() {
        check_abi().unwrap();
        // 版そのものは環境で異なる（Ubuntu の apt は 0.21 系、Homebrew は 0.22 系など）ため固定しない。
        // ヘッダとライブラリの版の一致は check_abi で確かめる。
        let v = version();
        let id = decoder_id();
        assert!(id.starts_with("libraw-"), "{id}");
        // 版の文字列（"0.21.2-Release"）と識別子（"libraw-0.21.2"）が対応する。
        assert!(
            v.starts_with(id.trim_start_matches("libraw-")),
            "{v} / {id}"
        );
        assert_eq!(split_version(0x0000_1502), (0, 21, 2));
    }

    #[test]
    fn errors_map_to_kinds() {
        assert!(check(0, "x").is_ok());
        assert!(matches!(
            check(libraw_code::FILE_UNSUPPORTED, "x"),
            Err(RawError::Unsupported(_))
        ));
        assert!(matches!(
            check(libraw_code::NO_THUMBNAIL, "x"),
            Err(RawError::Unsupported(_))
        ));
        assert!(matches!(
            check(libraw_code::IO_ERROR, "x"),
            Err(RawError::Decode(_))
        ));
        assert!(matches!(check(2, "x"), Err(RawError::Io(_))));
        assert!(matches!(
            check(code::NO_BAYER_DATA, "x"),
            Err(RawError::Unsupported(_))
        ));
        assert!(matches!(check(-12345, "x"), Err(RawError::Decode(_))));
        assert!(matches!(
            check(code::THUMB_TRUNCATED, "x"),
            Err(RawError::Decode(_))
        ));
        let msg = check(libraw_code::DATA_ERROR, "展開")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("展開") && msg.contains("-100008"), "{msg}");
    }

    #[test]
    fn c_strings_are_trimmed() {
        let to_c = |b: u8| c_char::from_ne_bytes([b]);
        let mut buf: [c_char; 16] = [0; 16];
        for (i, b) in b"  SONY \0junk".iter().enumerate() {
            buf[i] = to_c(*b);
        }
        assert_eq!(c_string(&buf).as_deref(), Some("SONY"));
        let zeros: [c_char; 4] = [0; 4];
        assert_eq!(c_string(&zeros), None);
        // NUL がなくても配列の長さで止まる。
        let full = [to_c(b'A'); 4];
        assert_eq!(c_string(&full).as_deref(), Some("AAAA"));
    }
}
