//! 比較明・比較暗合成の結合テスト（DEV-34。docs/composite_design.md の 8.1 節）。

mod common;

use std::path::{Path, PathBuf};

use common::{Env, JST_MINUTES, all_items, by_name, exif, hashes, import, wait_done, wait_ok};
use genzo_api::{
    ApiError, CompositeMode, CompositeReport, CompositeSettings, Core, ExportFormat, ExportOutcome,
    ExportSettings, JobResult, OutputColorSpace, VariantId,
};
use genzo_media::{DynRgbImage, ExifData, RgbImage8};
use genzo_model::{CropRect, DevelopSettings};
use genzo_pipeline::finish::output::{Dither, requantize_u16_to_u8};

const W: u32 = 96;
const H: u32 = 64;

/// 暗い背景に、色の付いた明るい四角（花火の代わり）を置いた JPEG を書く。
fn write_burst(path: &Path, rect: (u32, u32, u32, u32), color: [u8; 3], exif: &ExifData) {
    let (x0, y0, x1, y1) = rect;
    let img = image::RgbImage::from_fn(W, H, |x, y| {
        if (x0..x1).contains(&x) && (y0..y1).contains(&y) {
            image::Rgb(color)
        } else {
            // 背景にも少し模様を入れる（写真ごとに違う）。
            image::Rgb([
                (8 + (x + y) % 7) as u8,
                (6 + x % 5) as u8,
                (10 + y % 3 + color[2] as u32 % 4) as u8,
            ])
        }
    });
    let rgb = RgbImage8::from_raw(W, H, img.into_raw()).unwrap();
    let exif_bytes = genzo_media::exif_tiff_bytes(exif, W, H, OutputColorSpace::Srgb).unwrap();
    let bytes = genzo_media::jpeg::encode_jpeg(&rgb, 95, None, Some(&exif_bytes)).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// 3 枚の「花火」を書いて取り込む。撮影日時の順は C → A → B（名前の順と違う）。
fn setup(env: &Env) -> (Core, [VariantId; 3]) {
    let at = |t: &str| exif(Some(t), Some(JST_MINUTES), None);
    write_burst(
        &env.photos.join("A.jpg"),
        (10, 10, 50, 40),
        [250, 40, 30],
        &at("2026-08-01 20:00:02"),
    );
    write_burst(
        &env.photos.join("B.jpg"),
        (30, 20, 80, 60),
        [40, 230, 60],
        &at("2026-08-01 20:00:03"),
    );
    write_burst(
        &env.photos.join("C.jpg"),
        (5, 30, 40, 60),
        [60, 80, 240],
        &at("2026-08-01 20:00:01"),
    );
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let ids = [
        by_name(&items, "A.jpg").variant_id,
        by_name(&items, "B.jpg").variant_id,
        by_name(&items, "C.jpg").variant_id,
    ];
    (core, ids)
}

fn composite(
    core: &Core,
    ids: &[VariantId],
    s: &CompositeSettings,
    dest: Option<&Path>,
) -> CompositeReport {
    let job = core.composite(ids, s, dest).unwrap();
    match wait_ok(core, job) {
        JobResult::Composite(r) => r,
        other => panic!("{other:?}"),
    }
}

fn pixels16(path: &Path) -> Vec<u16> {
    match genzo_media::decode_image_file(path).unwrap().pixels {
        DynRgbImage::Rgb16(i) => i.as_raw().to_vec(),
        DynRgbImage::Rgb8(_) => panic!("16bit のはず: {}", path.display()),
    }
}

fn pixels8(path: &Path) -> Vec<u8> {
    match genzo_media::decode_image_file(path).unwrap().pixels {
        DynRgbImage::Rgb8(i) => i.as_raw().to_vec(),
        DynRgbImage::Rgb16(_) => panic!("8bit のはず: {}", path.display()),
    }
}

/// 1 枚ずつ、合成と同じ設定（TIFF 16bit・フル解像度）で書き出した画素。
fn exported(core: &Core, ids: &[VariantId], space: OutputColorSpace, dest: &Path) -> Vec<Vec<u16>> {
    let s = ExportSettings {
        format: ExportFormat::Tiff16,
        color_space: space,
        ..ExportSettings::default()
    };
    let job = core.export(ids, &s, dest).unwrap();
    let JobResult::Export(r) = wait_ok(core, job) else {
        panic!()
    };
    r.items
        .iter()
        .map(|it| match &it.outcome {
            ExportOutcome::Written { path, .. } => pixels16(path),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn files_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    v.sort();
    v
}

/// 比較明（既定の設定）: 撮影日時の順に並べ、1 枚目と同じフォルダに `<名前>-lighten-<枚数>.tif` で保存する。画素は、
/// 1 枚ずつ書き出した TIFF 16bit のチャンネルごとの最大とビット単位で同じ（16bit・ディザなしで現像している）。
/// カタログに登録し、元の写真とスタックにまとめて一番上にする。元のファイルは変わらない。
#[test]
fn lighten_is_the_per_channel_max_of_the_developed_photos() {
    let env = Env::new();
    let (core, [a, b, c]) = setup(&env);
    let before = hashes(&env.photos);

    let check = core
        .composite_check(&[a, b, c], &CompositeSettings::default(), None)
        .unwrap();
    assert_eq!(check.sources, vec![c, a, b], "撮影日時の順");
    assert!(check.problems.is_empty(), "{check:?}");
    assert_eq!(check.file_name.as_deref(), Some("C-lighten-3.tif"));
    assert!(check.geometry_differs.is_empty() && check.settings_differ.is_empty());
    assert_eq!(
        check.estimated_buffer_bytes,
        u64::from(W) * u64::from(H) * 6,
        "16bit の RGB 1 枚"
    );

    let r = composite(&core, &[a, b, c], &CompositeSettings::default(), None);
    assert!(r.failure.is_none() && !r.cancelled, "{r:?}");
    assert_eq!(r.sources, vec![c, a, b]);
    assert_eq!(r.composited, 3);
    assert_eq!(r.size, Some((W, H)));
    let path = r.path.clone().unwrap();
    assert_eq!(path, env.photos.join("C-lighten-3.tif"));

    let out = env.dir.path().join("each");
    let each = exported(&core, &[c, a, b], OutputColorSpace::AdobeRgb, &out);
    let expect: Vec<u16> = (0..each[0].len())
        .map(|i| each.iter().map(|p| p[i]).max().unwrap())
        .collect();
    assert_eq!(pixels16(&path), expect);

    // 登録とスタック。
    let result = r.registered.expect("登録した");
    assert!(r.registration_error.is_none(), "{r:?}");
    assert!(r.stacked, "{r:?}");
    let details = core.variant_details(result).unwrap();
    let stack = details.stack.expect("スタックに入る");
    assert_eq!(stack.top_variant_id, result);
    assert_eq!(stack.members.len(), 4);
    for v in [a, b, c] {
        assert!(stack.members.contains(&v));
    }
    // 撮影日時は 1 枚目（C）の値。折りたたんだスタックは、合成の結果（一番上）だけが見える。
    let items = all_items(&core);
    assert_eq!(
        items.iter().map(|i| i.variant_id).collect::<Vec<_>>(),
        vec![result]
    );
    let composed = core.variant_details(result).unwrap().summary;
    assert_eq!(composed.file_name, "C-lighten-3.tif");
    let first = core.variant_details(c).unwrap().summary;
    assert!(composed.capture.local().is_some());
    assert_eq!(composed.capture.local(), first.capture.local());

    // 元のファイルは変わらない（増えたのは合成の結果だけ）。
    let mut after = hashes(&env.photos);
    after.remove(&path);
    assert_eq!(after, before);

    // もう一度合成すると、上書きせずに連番を付ける。
    let again = composite(&core, &[a, b, c], &CompositeSettings::default(), None);
    let p2 = again.path.unwrap();
    assert_ne!(p2, path);
    assert!(path.exists() && p2.exists());
}

/// 比較暗とカラー比較明。保存先の指定・スタックにまとめない設定・色空間の設定。
#[test]
fn darken_and_lighter_color_with_options() {
    let env = Env::new();
    let (core, [a, b, c]) = setup(&env);
    let dest = env.dir.path().join("composites");
    let each = exported(
        &core,
        &[c, a, b],
        OutputColorSpace::Srgb,
        &env.dir.path().join("each"),
    );

    let s = CompositeSettings {
        mode: CompositeMode::Darken,
        color_space: OutputColorSpace::Srgb,
        stack_with_sources: false,
        ..CompositeSettings::default()
    };
    let r = composite(&core, &[a, b, c], &s, Some(&dest));
    let path = r.path.clone().unwrap();
    assert_eq!(path, dest.join("C-darken-3.tif"));
    let expect: Vec<u16> = (0..each[0].len())
        .map(|i| each.iter().map(|p| p[i]).min().unwrap())
        .collect();
    assert_eq!(pixels16(&path), expect);
    assert!(!r.stacked);
    let result = r.registered.unwrap();
    assert!(core.variant_details(result).unwrap().stack.is_none());

    // カラー比較明: 画素ごとに、どれか 1 枚の画素がそのまま入る（チャンネルを混ぜない）。
    let s = CompositeSettings {
        mode: CompositeMode::LighterColor,
        color_space: OutputColorSpace::Srgb,
        stack_with_sources: false,
        ..CompositeSettings::default()
    };
    let r = composite(&core, &[a, b, c], &s, Some(&dest));
    let got = pixels16(r.path.as_ref().unwrap());
    let mut mixed_somewhere = false;
    let lighten: Vec<u16> = (0..each[0].len())
        .map(|i| each.iter().map(|p| p[i]).max().unwrap())
        .collect();
    for (i, px) in got.chunks_exact(3).enumerate() {
        let k = i * 3;
        assert!(
            each.iter().any(|p| &p[k..k + 3] == px),
            "画素 {i} はどれかの写真の画素のまま"
        );
        mixed_somewhere |= lighten[k..k + 3] != *px;
    }
    assert!(
        mixed_somewhere,
        "比較明（チャンネルを混ぜる）とは違う画素がある"
    );
}

/// 8bit の形式: 16bit で合成してから 1 回だけ量子化する（TIFF 16bit の結果に `requantize_u16_to_u8` を 1 回かけた
/// ものと同じ）。
#[test]
fn eight_bit_formats_are_quantized_once_after_compositing() {
    let env = Env::new();
    let (core, [a, b, c]) = setup(&env);
    let dest = env.dir.path().join("out");
    let base = CompositeSettings {
        stack_with_sources: false,
        ..CompositeSettings::default()
    };
    let r16 = composite(&core, &[a, b, c], &base, Some(&dest));
    let r8 = composite(
        &core,
        &[a, b, c],
        &CompositeSettings {
            format: ExportFormat::Png8,
            ..base.clone()
        },
        Some(&dest),
    );
    assert_eq!(
        r8.path.as_deref(),
        Some(dest.join("C-lighten-3.png").as_path())
    );
    let p16 = pixels16(r16.path.as_ref().unwrap());
    assert_eq!(
        pixels8(r8.path.as_ref().unwrap()),
        requantize_u16_to_u8(&p16, W, (0, 0), Dither::DEFAULT_8BIT)
    );
}

/// 合成できないとき: 写真が 1 枚・切り抜きで寸法が違う（何も書かずに、どの写真かを返す）・動画を除く。
#[test]
fn problems_are_reported_and_nothing_is_written_on_failure() {
    let env = Env::new();
    let (core, [a, b, c]) = setup(&env);

    // 1 枚だけ。
    let check = core
        .composite_check(&[a], &CompositeSettings::default(), None)
        .unwrap();
    assert_eq!(check.problems.len(), 1, "{check:?}");
    assert!(matches!(
        core.composite(&[a], &CompositeSettings::default(), None),
        Err(ApiError::InvalidArgument(_))
    ));

    // B だけ切り抜く: 確認では幾何の違いを警告し、合成は B で失敗して何も書かない。
    core.open_develop(b).unwrap();
    let cropped = DevelopSettings {
        geometry: genzo_model::Geometry {
            crop: CropRect {
                left: 0.1,
                top: 0.0,
                right: 0.9,
                bottom: 1.0,
            },
            ..genzo_model::Geometry::default()
        },
        ..core.develop_settings(b).unwrap()
    };
    core.set_settings(&cropped, false).unwrap();
    core.flush_develop().unwrap();
    core.close_develop().unwrap();

    let check = core
        .composite_check(&[a, b, c], &CompositeSettings::default(), None)
        .unwrap();
    assert_eq!(check.geometry_differs, vec![b]);
    assert!(check.settings_differ.is_empty());
    assert!(check.problems.is_empty(), "警告だけ（止めない）");

    let dest = env.dir.path().join("fail");
    let job = core
        .composite(&[a, b, c], &CompositeSettings::default(), Some(&dest))
        .unwrap();
    let info = wait_done(&core, job);
    let Some(JobResult::Composite(r)) = info.result else {
        panic!("{info:?}")
    };
    let f = r.failure.expect("寸法が違うので失敗");
    assert_eq!(f.variant_id, Some(b));
    assert!(f.error.message.contains("寸法"), "{}", f.error.message);
    assert!(r.path.is_none() && r.registered.is_none());
    assert_eq!(r.composited, 2, "C と A まで重ねた");
    assert!(
        !dest.exists(),
        "何も書かない（保存先のフォルダも作らない）: {:?}",
        files_in(&dest)
    );
}

/// 取り消し: いつ取り消しても、取り消しとして終わったなら何も残さない（保存先のフォルダも作らない）。取り消しが
/// 間に合わずに終わったなら、登録まで済んでいる（途中の状態を残さない）。
#[test]
fn cancelling_leaves_nothing_behind() {
    let env = Env::new();
    let (core, [a, b, c]) = setup(&env);
    let base = CompositeSettings {
        stack_with_sources: false,
        ..CompositeSettings::default()
    };
    for (i, delay_ms) in [0u64, 5, 20, 60].into_iter().enumerate() {
        let dest = env.dir.path().join(format!("cancel-{i}"));
        let job = core.composite(&[a, b, c], &base, Some(&dest)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        let _ = core.cancel_job(job);
        let info = wait_done(&core, job);
        let Some(JobResult::Composite(r)) = info.result else {
            // 取り消しでジョブそのものが結果なしで終わることもある。
            assert!(!dest.exists(), "{info:?}");
            continue;
        };
        if r.cancelled {
            assert!(r.path.is_none() && r.registered.is_none(), "{r:?}");
            assert!(!dest.exists(), "何も残さない: {:?}", files_in(&dest));
        } else {
            assert!(r.failure.is_none(), "{r:?}");
            assert!(r.registered.is_some(), "{r:?}");
            assert!(r.path.as_ref().unwrap().exists());
        }
    }
}
