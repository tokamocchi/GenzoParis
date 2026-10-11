//! 実機のサンプル（利用者の私的な写真の RAW）の回帰テスト（02 の MAINT-03。実機の ARW の不具合
//! A-1〜A-3 の確認）。
//!
//! 環境変数 `GENZO_PRIVATE_SAMPLES` でサンプルのフォルダを指定したときだけ動く（未設定なら何もせずに
//! 成功する）。LibRaw が必要（`--features libraw`。機能フラグがなければスキップ）。時間がかかるので
//! `--release` で実行する。
//!
//! ```text
//! GENZO_PRIVATE_SAMPLES=<フォルダ> cargo test -p genzo-pipeline --features libraw --release \
//!     --test private_samples -- --nocapture
//! ```
//!
//! - サンプルは SHA-256 で識別し、期待値（`tests/private_samples.json`。写真の内容を明かさない数値と
//!   ハッシュだけ）と照合する。期待値のないファイルは、共通の性質（展開できるか、きれいなエラーに
//!   なる。データの誤りがない。端の帯がない。NaN がない）だけを確かめる。
//! - 期待値を計測した値で書き直すには `GENZO_UPDATE_PRIVATE_EXPECT=1`（ID と SHA-256 だけを書いた
//!   項目を足してから実行すると、ほかの値が入る）。書き直しの実行は失敗にしないので、書き直した後は
//!   git の差分を確認し、書き直しなしでもう一度実行する。
//! - 壊したファイル（途中で切ったもの・データの位置をファイルの外にしたもの）は OS の一時ディレクトリに
//!   作り、テストの終わりに消す。サンプル・サンプルから作った画像はリポジトリに入れない。
//! - 出力には ID と SHA-256 の先頭 12 桁だけを出す（ファイル名・パスは出さない）。
//!
//! | 確かめること | 内容 |
//! |---|---|
//! | T0 共通 | 展開の成否（失敗なら `Unsupported` / `Decode`）、データの誤り 0、既定の切り抜きが有効画素の中、撮影情報の寸法 = 見える範囲、出力に NaN がない、端の帯がない |
//! | T1 展開 | 寸法・既定の切り抜き・向き・CFA・黒 / 白レベル・飽和の閾値・記録方式・データの終わりが期待値と一致 |
//! | T2 範囲と端 | 書き出しの元の寸法 = 既定の切り抜きの寸法（向きを反映）。4 辺に詰め物の帯・色のずれた線がない |
//! | T3 飽和 | 張り付いた緑の画素に飽和の印が付く割合、印の付く画素の数 |
//! | T4 壊したファイル | 途中で切ったファイルは、データの終わりより前で切ればエラー、後なら元と同じ CFA。データの位置をファイルの外にしたファイルはエラー |
//! | T6 現像の統計 | 既定の現像（CPU 版）の L\* の平均・幅・C\* の平均と、カメラの埋め込み JPEG との L\* の平均の差 |

use std::path::{Path, PathBuf};

use genzo_model::{DevelopSettings, ExportFormat, ExportSettings, ExportSize, OutputColorSpace};
use genzo_pipeline::{
    Engine, EngineConfig, ExportOptions, ExportPixels, NeverCancel, PhotoSource, SourceId,
    SourceImage,
};
use genzo_raw::{RawError, RawImage, ThumbnailFormat};
use genzo_testkit::private_samples::{
    self as ps, DecodeExpect, MAX_EDGE_COLOR_STEP_RATIO, MIN_EDGE_FLATNESS_RATIO, REQUIRE_ENV,
    RenderExpect, SampleExpect, UPDATE_ENV,
};

/// 期待値のファイル。
fn expect_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/private_samples.json")
}

/// ワークスペースの根（サンプルのフォルダがこの中を指していないことを確かめる）。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 統計を取る出力の長辺の既定値（px。カメラの小さいプレビューと同じ。quality.md の 1.2 節）。
const RENDER_LONG_EDGE: u32 = 1616;
/// 統計の許容差の既定値（L\*・C\* の単位。**仮置き**: LibRaw・OS の違いによる小さな差を許し、
/// ステージ 15 のカーブ・彩度の変更（数 L\* 以上）は検出できる大きさ）。
const RENDER_TOLERANCE: f64 = 0.5;
/// 飽和の印の付く画素の数の範囲（計測値の ±1%）。
const MARKED_PIXELS_MARGIN: f64 = 0.01;

/// 期待値のファイルの検査（普通の `cargo test` で動く。サンプルがなくても、リポジトリに置く値の
/// 形とプライバシーを確かめる）。
#[test]
fn expect_file_has_only_allowed_values() {
    let f = ps::load_expect(&expect_path()).unwrap();
    for s in &f.samples {
        assert!(
            s.decode.is_some() || s.render.is_some() || s.camera_model.is_none(),
            "{}",
            s.id
        );
    }
}

/// サンプルのテスト（`GENZO_PRIVATE_SAMPLES` を指定したときだけ動く）。
#[test]
fn private_samples() {
    let require = ps::env_flag(REQUIRE_ENV);
    let skip = |why: &str| {
        assert!(!require, "{REQUIRE_ENV}=1 なのに実行できません: {why}");
        eprintln!("実機のサンプルのテストをスキップ: {why}");
    };
    let dir = match ps::sample_dir(&workspace_root()) {
        Ok(Some(d)) => d,
        Ok(None) => return skip(&format!("{} が未設定", ps::SAMPLES_ENV)),
        Err(e) => panic!("{e}"),
    };
    if !genzo_raw::LIBRAW_ENABLED {
        return skip("LibRaw が無効（--features libraw で実行してください）");
    }
    let update = ps::env_flag(UPDATE_ENV);
    let mut expect = ps::load_expect(&expect_path()).unwrap();
    let files = ps::list_raw_files(&dir).expect("サンプルのフォルダを読めません");
    eprintln!(
        "サンプル {} 個、期待値 {} 件、LibRaw {}",
        files.len(),
        expect.samples.len(),
        genzo_raw::libraw_version().unwrap_or("?")
    );
    let mut matched = 0;
    let mut failures: Vec<String> = Vec::new();
    for path in &files {
        let hash = ps::sha256_file(path).expect("サンプルを読めません");
        let idx = expect.samples.iter().position(|s| s.sha256 == hash);
        let label = match idx {
            Some(i) => expect.samples[i].id.clone(),
            None => format!("unknown-{}", ps::short_hash(&hash)),
        };
        eprintln!("== {label}（{}）", ps::short_hash(&hash));
        let mut ctx = Check {
            label: label.clone(),
            failures: Vec::new(),
        };
        let measured = check_sample(path, idx.map(|i| &expect.samples[i]), &mut ctx);
        // DATA-01: 元ファイルを書き換えていない。
        let after = ps::sha256_file(path).expect("サンプルを読めません");
        ctx.check(after == hash, "テストの後でサンプルのハッシュが変わった");
        failures.extend(ctx.failures);
        if let Some(i) = idx {
            matched += 1;
            if update {
                apply_measured(&mut expect.samples[i], measured);
            }
        }
    }
    if update {
        ps::save_expect(&expect_path(), &expect).unwrap();
        eprintln!("期待値のファイルを書き直しました（git の差分を確認してください）");
    }
    if matched == 0 {
        eprintln!("期待値のあるサンプルが 1 つもありません");
        assert!(
            !require,
            "{REQUIRE_ENV}=1 なのに期待値のあるサンプルがありません"
        );
    }
    if update {
        // 書き直しの実行では、古い期待値との違いは失敗にしない（書き直した期待値で、もう一度
        // 書き直しなしで実行して確かめる）。
        if !failures.is_empty() {
            eprintln!("書き直す前の期待値との違い:\n{}", failures.join("\n"));
        }
        return;
    }
    assert!(failures.is_empty(), "失敗:\n{}", failures.join("\n"));
}

/// 1 つのサンプルの失敗を集める。
struct Check {
    label: String,
    failures: Vec<String>,
}

impl Check {
    fn check(&mut self, ok: bool, what: impl AsRef<str>) {
        if !ok {
            let msg = format!("{}: {}", self.label, what.as_ref());
            eprintln!("  失敗: {}", what.as_ref());
            self.failures.push(msg);
        }
    }
}

/// 計測した値（期待値の書き直し用）。
#[derive(Default)]
struct Measured {
    camera_model: Option<String>,
    decode: Option<DecodeExpect>,
    marked_pixels: Option<u64>,
    render: Option<(ps::LabStats, ps::LabStats)>,
}

fn check_sample(path: &Path, expect: Option<&SampleExpect>, c: &mut Check) -> Measured {
    let mut m = Measured::default();
    // T0・T1: 展開。
    let decoded = match genzo_raw::decode_file_with_details(path) {
        Ok(d) => d,
        Err(e) => {
            let clean = matches!(e, RawError::Unsupported(_) | RawError::Decode(_));
            eprintln!("  展開できない（{}）", error_kind(&e));
            c.check(
                clean,
                format!("展開のエラーの種類が想定外: {}", error_kind(&e)),
            );
            c.check(
                expect.and_then(|s| s.decode.as_ref()).is_none(),
                "期待値があるのに展開できない",
            );
            return m;
        }
    };
    let (img, d) = (&decoded.image, &decoded.details);
    let crop = img.default_crop.map(|r| [r.x, r.y, r.width, r.height]);
    let data_end = u64::try_from(d.data_offset).unwrap_or(0) + d.data_size;
    let measured = DecodeExpect {
        unpack_function: d.unpack_function.clone(),
        raw_size: [d.raw_width, d.raw_height],
        active_size: [img.width, img.height],
        default_crop: crop,
        orientation: img.metadata.orientation.to_exif(),
        cfa: img.cfa.name().to_owned(),
        black_level: img.black_level,
        white_level: img.white_level,
        saturation_level: img.saturation_level,
        sony_raw_file_type: d.sony_raw_file_type,
        data_end,
    };
    eprintln!(
        "  展開: {} 有効画素 {}×{} 既定の切り抜き {:?} 黒 {:?} 白 {} 閾値 {:.2} 記録方式 {:?} 確認 {}",
        d.unpack_function,
        img.width,
        img.height,
        crop,
        img.black_level,
        img.white_level,
        img.saturation_level,
        d.sony_raw_file_type,
        d.data_range_checked
    );
    c.check(d.data_error_count == 0, "LibRaw がデータの誤りを検出した");
    c.check(
        img.visible_area().fits_within(img.width, img.height),
        "既定の切り抜きが有効画素からはみ出している",
    );
    let v = img.visible_area();
    c.check(
        (img.metadata.width, img.metadata.height) == (Some(v.width), Some(v.height)),
        "撮影情報の寸法が見える範囲と違う",
    );
    if let Some(note) = &d.default_crop_note {
        eprintln!("  既定の切り抜きを使わなかった: {note}");
    }
    m.camera_model = img
        .metadata
        .model
        .clone()
        .filter(|s| ps::is_allowed_model(s));
    if let Some(e) = expect.and_then(|s| s.decode.as_ref()) {
        compare_decode(e, &measured, c);
    }
    m.decode = Some(measured);

    // T3: 飽和。
    let marked = img
        .data
        .iter()
        .filter(|&&v| f32::from(v) >= img.saturation_level)
        .count() as u64;
    m.marked_pixels = Some(marked);
    if let Some(sat) = expect.and_then(|s| s.saturation.as_ref()) {
        let (plateau, plateau_marked) = green_plateau(img, sat.plateau_dn);
        let fraction = if plateau == 0 {
            1.0
        } else {
            plateau_marked as f64 / plateau as f64
        };
        eprintln!(
            "  飽和: 張り付いた緑 {plateau} 画素のうち印 {plateau_marked}（{fraction:.4}）、印の付く画素 {marked}"
        );
        c.check(plateau > 0, "張り付いた緑の画素がない");
        c.check(
            fraction >= sat.min_marked_fraction,
            format!(
                "張り付いた緑に印が付く割合 {fraction} が {} 未満",
                sat.min_marked_fraction
            ),
        );
        c.check(
            (sat.marked_pixels[0]..=sat.marked_pixels[1]).contains(&marked),
            format!(
                "印の付く画素の数 {marked} が範囲 {:?} の外",
                sat.marked_pixels
            ),
        );
    }

    // T4: 壊したファイル（展開の前の確認の対象の展開の関数のとき）。
    corrupted_files(path, &decoded, c);

    // T0・T2・T6: 既定の現像（CPU 版）。
    let render_expect = expect.and_then(|s| s.render.as_ref());
    let long_edge = render_expect.map_or(RENDER_LONG_EDGE, |r| r.long_edge);
    let source = PhotoSource::new(
        SourceId::new(genzo_model::FileId::new(1), 1),
        SourceImage::Raw(std::sync::Arc::new(decoded.image.clone())),
    )
    .unwrap();
    let Some((out, full)) = render(&source, long_edge, c) else {
        return m;
    };
    let oriented = if img.metadata.orientation.swaps_dimensions() {
        (v.height, v.width)
    } else {
        (v.width, v.height)
    };
    c.check(
        full == oriented,
        format!("書き出しの元の寸法 {full:?} が既定の切り抜きの寸法 {oriented:?} と違う"),
    );
    check_edges(&out, "既定の切り抜き", c, true);
    // 参考: 既定の切り抜きを外した現像（修正の前の動作）の端の指標（判定はしない）。
    if img.default_crop.is_some() {
        let mut no_crop: RawImage = decoded.image.clone();
        no_crop.default_crop = None;
        let source = PhotoSource::new(
            SourceId::new(genzo_model::FileId::new(2), 1),
            SourceImage::Raw(std::sync::Arc::new(no_crop)),
        )
        .unwrap();
        if let Some((out, _)) = render(&source, long_edge, c) {
            check_edges(&out, "参考: 既定の切り抜きなし", c, false);
        }
    }
    let (w, h, px) = &out;
    let Some(stats) = ps::srgb_lab_stats(px) else {
        return m;
    };
    let camera = camera_stats(path);
    eprintln!(
        "  現像 {w}×{h}: L* 平均 {:.2} 幅 {:.2} C* 平均 {:.2}",
        stats.lstar_mean,
        stats.lstar_width(),
        stats.chroma_mean
    );
    if let Some(cam) = &camera {
        eprintln!(
            "  カメラの JPEG: L* 平均 {:.2} 幅 {:.2}（差 {:+.2}）",
            cam.lstar_mean,
            cam.lstar_width(),
            stats.lstar_mean - cam.lstar_mean
        );
    }
    if let Some(r) = render_expect {
        let near = |a: f64, b: f64| (a - b).abs() <= r.tolerance;
        c.check(
            near(stats.lstar_mean, r.lstar_mean),
            "L* の平均が期待値と違う",
        );
        c.check(
            near(stats.lstar_width(), r.lstar_width),
            "L* の幅が期待値と違う",
        );
        c.check(
            near(stats.chroma_mean, r.chroma_mean),
            "C* の平均が期待値と違う",
        );
        match &camera {
            Some(cam) => {
                c.check(
                    near(cam.lstar_mean, r.camera_lstar_mean)
                        && near(cam.lstar_width(), r.camera_lstar_width),
                    "カメラの JPEG の統計が期待値と違う（埋め込み JPEG の読み方が変わった）",
                );
                c.check(
                    (stats.lstar_mean - cam.lstar_mean).abs() <= r.max_lstar_mean_gap,
                    "カメラの JPEG との L* の平均の差が上限を超えた",
                );
            }
            None => c.check(false, "カメラの埋め込み JPEG を読めない"),
        }
    }
    if let Some(cam) = camera {
        m.render = Some((stats, cam));
    }
    m
}

fn error_kind(e: &RawError) -> &'static str {
    match e {
        RawError::Unsupported(_) => "Unsupported",
        RawError::Decode(_) => "Decode",
        RawError::Io(_) => "Io",
        _ => "その他（検証）",
    }
}

fn compare_decode(e: &DecodeExpect, m: &DecodeExpect, c: &mut Check) {
    c.check(e.unpack_function == m.unpack_function, "展開の関数が違う");
    c.check(e.raw_size == m.raw_size, "RAW のバッファの寸法が違う");
    c.check(e.active_size == m.active_size, "有効画素の寸法が違う");
    c.check(e.default_crop == m.default_crop, "既定の切り抜きが違う");
    c.check(e.orientation == m.orientation, "向きが違う");
    c.check(e.cfa == m.cfa, "CFA の配列が違う");
    c.check(
        e.black_level
            .iter()
            .zip(&m.black_level)
            .all(|(a, b)| ps::close(*b, *a)),
        "黒レベルが違う",
    );
    c.check(ps::close(m.white_level, e.white_level), "白レベルが違う");
    c.check(
        ps::close(m.saturation_level, e.saturation_level),
        "飽和の閾値が違う",
    );
    c.check(
        e.sony_raw_file_type == m.sony_raw_file_type,
        "記録方式が違う",
    );
    c.check(e.data_end == m.data_end, "データの終わりが違う");
}

/// 張り付いた緑の画素（`plateau` 以上）の数と、そのうち飽和の閾値以上（印が付く）の数。
fn green_plateau(img: &RawImage, plateau: u16) -> (u64, u64) {
    let (mut n, mut marked) = (0u64, 0u64);
    for y in 0..img.height {
        for x in 0..img.width {
            if !img.cfa.color_at(x as usize, y as usize).is_green() {
                continue;
            }
            let v = img.data[(y * img.width + x) as usize];
            if v >= plateau {
                n += 1;
                if f32::from(v) >= img.saturation_level {
                    marked += 1;
                }
            }
        }
    }
    (n, marked)
}

/// 壊したファイル（T4）。一時ディレクトリに作り、終わったら消す。
fn corrupted_files(path: &Path, decoded: &genzo_raw::DecodedRaw, c: &mut Check) {
    let d = &decoded.details;
    let bytes = std::fs::read(path).expect("サンプルを読めません");
    let len = bytes.len();
    let data_end = usize::try_from(d.data_offset).unwrap_or(0) + d.data_size as usize;
    let original = cfa_hash(&decoded.image.data);
    let dir = tempfile::tempdir().expect("一時ディレクトリを作れません");
    let mut cases: Vec<(String, Vec<u8>, bool)> = Vec::new();
    for (name, cut) in [
        ("10%", len / 10),
        ("50%", len / 2),
        ("90%", len * 9 / 10),
        ("最後の 1 KiB を削る", len.saturating_sub(1024)),
    ] {
        cases.push((name.to_owned(), bytes[..cut].to_vec(), cut < data_end));
    }
    // データの位置をファイルの外に（StripOffsets の項目を探して書き換える）。
    if let Some(patched) = patch_strip_offset(&bytes, d.data_offset, len as u32 + 4096) {
        cases.push(("データの位置がファイルの外".to_owned(), patched, true));
    } else {
        eprintln!(
            "  壊したファイル: StripOffsets の項目が見つからない（データの位置の書き換えは省略）"
        );
    }
    for (i, (name, data, should_fail)) in cases.iter().enumerate() {
        let p = dir.path().join(format!("corrupt{i}.raw"));
        std::fs::write(&p, data).expect("一時ファイルを書けません");
        let r = genzo_raw::decode_file(&p);
        let outcome = match &r {
            Ok(img) => {
                if cfa_hash(&img.data) == original {
                    "成功（元と同じ CFA）"
                } else {
                    "成功（元と違う CFA）"
                }
            }
            Err(e) => error_kind(e),
        };
        eprintln!("  壊したファイル {name}: {outcome}");
        if d.data_range_checked {
            if *should_fail {
                c.check(
                    matches!(r, Err(RawError::Decode(_))),
                    format!("壊したファイル（{name}）がエラーにならない"),
                );
            } else {
                c.check(
                    outcome == "成功（元と同じ CFA）",
                    format!("データの後ろで切ったファイル（{name}）が元と同じに展開されない"),
                );
            }
            // メモリから開く場合も同じ（Windows の長いパスの回避策の経路）。
            let rb = genzo_raw::decode_bytes(data).and_then(|d| d.into_verified_image());
            c.check(
                rb.is_err() == *should_fail,
                format!("メモリから開いた壊したファイル（{name}）の結果が違う"),
            );
        } else {
            // 確認の対象外の展開の関数: 本体が落ちないこと（ここまで来たこと）だけ。
            c.check(
                !matches!(r, Err(RawError::Io(_))),
                format!("壊したファイル（{name}）が入出力のエラーになった"),
            );
        }
    }
    drop(dir);
}

/// CFA の値（リトルエンディアンの u16 の列）の SHA-256。
fn cfa_hash(v: &[u16]) -> String {
    let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
    ps::sha256_bytes(&bytes)
}

/// リトルエンディアンの TIFF の StripOffsets（タグ 273、LONG、数 1）の項目のうち、値が `offset` の
/// ものを `new_offset` に書き換えたバイト列。見つからなければ `None`。
fn patch_strip_offset(bytes: &[u8], offset: i64, new_offset: u32) -> Option<Vec<u8>> {
    let offset = u32::try_from(offset).ok()?;
    if bytes.get(..2)? != b"II" {
        return None;
    }
    let mut pattern = Vec::with_capacity(12);
    pattern.extend_from_slice(&273u16.to_le_bytes());
    pattern.extend_from_slice(&4u16.to_le_bytes());
    pattern.extend_from_slice(&1u32.to_le_bytes());
    pattern.extend_from_slice(&offset.to_le_bytes());
    let limit = bytes.len().min(offset as usize);
    let pos = bytes[..limit]
        .windows(12)
        .position(|w| w == pattern.as_slice())?;
    let mut out = bytes.to_vec();
    out[pos + 8..pos + 12].copy_from_slice(&new_offset.to_le_bytes());
    Some(out)
}

/// 書き出した画像（幅, 高さ, sRGB の画素（0〜1））。
type Rendered = (u32, u32, Vec<[f32; 3]>);

/// 既定の現像を書き出し（PNG 16bit・sRGB・長辺 `long_edge`）、画像と元の寸法を返す。
fn render(source: &PhotoSource, long_edge: u32, c: &mut Check) -> Option<(Rendered, (u32, u32))> {
    let engine = Engine::new(EngineConfig::default()).unwrap();
    let export = ExportSettings {
        format: ExportFormat::Png16,
        color_space: OutputColorSpace::Srgb,
        size: ExportSize::LongEdge(long_edge),
        ..Default::default()
    };
    let out = match engine.export(
        source,
        &DevelopSettings::default(),
        &export,
        &ExportOptions::default(),
        &NeverCancel,
    ) {
        Ok(o) => o,
        Err(e) => {
            c.check(false, format!("現像できない: {e}"));
            return None;
        }
    };
    c.check(out.non_finite_replaced == 0, "NaN・無限大を置き換えた");
    let ExportPixels::Rgb16(v) = &out.pixels else {
        c.check(false, "16bit の出力ではない");
        return None;
    };
    let px: Vec<[f32; 3]> = v
        .chunks_exact(3)
        .map(|p| [0, 1, 2].map(|i| f32::from(p[i]) / 65535.0))
        .collect();
    c.check(
        px.iter().flatten().all(|v| v.is_finite()),
        "出力に NaN がある",
    );
    Some(((out.width, out.height, px), out.full_resolution_size))
}

/// 4 辺の端の帯（T2）。`strict` なら閾値で判定する。
fn check_edges(out: &Rendered, what: &str, c: &mut Check, strict: bool) {
    let (w, h, px) = out;
    let Some(bands) = ps::edge_bands(*w as usize, *h as usize, px) else {
        eprintln!("  {what}: 小さすぎて端の帯の指標を計算できない");
        return;
    };
    let names = ["上", "下", "左", "右"];
    let text: Vec<String> = bands
        .iter()
        .zip(names)
        .map(|(b, n)| {
            format!(
                "{n} 平らさ {:.3} 色の段差 {:.2}",
                b.flatness_ratio, b.color_step_ratio
            )
        })
        .collect();
    eprintln!("  {what}の端: {}", text.join("、"));
    if strict {
        for (b, n) in bands.iter().zip(names) {
            c.check(
                b.flatness_ratio >= MIN_EDGE_FLATNESS_RATIO,
                format!("{n}の端に平らな帯がある（{:.3}）", b.flatness_ratio),
            );
            c.check(
                b.color_step_ratio <= MAX_EDGE_COLOR_STEP_RATIO,
                format!("{n}の端に色のずれた線がある（{:.2}）", b.color_step_ratio),
            );
        }
    }
}

/// カメラの埋め込み JPEG（LibRaw が選んだもの）の L\* の統計。
fn camera_stats(path: &Path) -> Option<ps::LabStats> {
    let t = genzo_raw::extract_thumbnail(path).ok()?;
    if t.format != ThumbnailFormat::Jpeg {
        return None;
    }
    let (_, _, px) = ps::decode_jpeg_srgb(&t.data).ok()?;
    ps::srgb_lab_stats(&px)
}

/// 計測した値を期待値に入れる（ID・SHA-256・人が決めた値（張り付く値・割合の下限・許容差・差の上限）は
/// 保つ）。
fn apply_measured(s: &mut SampleExpect, m: Measured) {
    if m.camera_model.is_some() {
        s.camera_model = m.camera_model;
    }
    if m.decode.is_some() {
        s.decode = m.decode;
    }
    if let (Some(sat), Some(n)) = (s.saturation.as_mut(), m.marked_pixels) {
        let d = ((n as f64) * MARKED_PIXELS_MARGIN).ceil() as u64;
        sat.marked_pixels = [n.saturating_sub(d), n + d];
    }
    if let Some((r, cam)) = m.render {
        let (long_edge, tolerance) = s
            .render
            .as_ref()
            .map_or((RENDER_LONG_EDGE, RENDER_TOLERANCE), |e| {
                (e.long_edge, e.tolerance)
            });
        let gap = (r.lstar_mean - cam.lstar_mean).abs();
        let round2 = |v: f64| (v * 100.0).round() / 100.0;
        s.render = Some(RenderExpect {
            long_edge,
            lstar_mean: round2(r.lstar_mean),
            lstar_width: round2(r.lstar_width()),
            chroma_mean: round2(r.chroma_mean),
            camera_lstar_mean: round2(cam.lstar_mean),
            camera_lstar_width: round2(cam.lstar_width()),
            // 今の差より 1 L* 大きい値（ステージ 15 のカーブなどで暗く・明るくずれたら検出する）。
            max_lstar_mean_gap: (gap + 1.0).ceil(),
            tolerance,
        });
    }
}
