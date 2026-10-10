//! 書き出しの結合テスト（01 のストーリー 7、EXP-01・EXP-04・SEC-03、04 の 2.4 節・6.4 節）。

mod common;

use std::path::{Path, PathBuf};

use common::{Env, JST_MINUTES, all_items, by_name, exif, hashes, import, wait_ok, write_jpeg};
use genzo_api::{
    Core, ErrorKind, ExportFormat, ExportOutcome, ExportReport, ExportSettings, ExportSize,
    JobResult, OutputColorSpace, VariantId,
};
use genzo_model::{ConflictPolicy, GpsCoord};

fn export(core: &Core, ids: &[VariantId], s: &ExportSettings, dest: &Path) -> ExportReport {
    let job = core.export(ids, s, dest).unwrap();
    match wait_ok(core, job) {
        JobResult::Export(r) => r,
        other => panic!("{other:?}"),
    }
}

fn written_path(r: &ExportReport, i: usize) -> PathBuf {
    match &r.items[i].outcome {
        ExportOutcome::Written { path, .. } => path.clone(),
        other => panic!("{other:?}"),
    }
}

fn has_gps(path: &Path) -> bool {
    let bytes = std::fs::read(path).unwrap();
    match exif::Reader::new().read_from_container(&mut std::io::Cursor::new(&bytes)) {
        Ok(e) => e
            .get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY)
            .is_some(),
        Err(_) => false,
    }
}

fn icc_description(path: &Path) -> String {
    let d = genzo_media::decode_image_file(path).unwrap();
    assert!(d.profile.is_embedded(), "ICC を埋め込む（IQ-06）");
    d.profile
        .to_icc()
        .unwrap()
        .description()
        .unwrap_or_default()
        .to_owned()
}

/// テスト 5: JPEG / TIFF / PNG、色空間、長辺の指定、GPS の削除、原本と同じ場所・同じ名前への書き出しの
/// 拒否、衝突時の連番。元ファイルは変わらない（DATA-01）。
#[test]
fn export_formats_sizes_gps_and_original_protection() {
    let env = Env::new();
    let gps = GpsCoord::new(35.0, 139.0);
    write_jpeg(
        &env.photos.join("IMG_0001.jpg"),
        (160, 120),
        1,
        Some(&exif(Some("2024-05-01 10:00:00"), Some(JST_MINUTES), gps)),
    );
    write_jpeg(&env.photos.join("IMG_0002.jpg"), (90, 120), 2, None);
    let before = hashes(&env.photos);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let v1 = by_name(&items, "IMG_0001.jpg").variant_id;
    let v2 = by_name(&items, "IMG_0002.jpg").variant_id;
    let out = env.dir.path().join("out");

    // JPEG・sRGB・長辺 64・GPS を削除。
    let s = ExportSettings {
        format: ExportFormat::Jpeg { quality: 85 },
        color_space: OutputColorSpace::Srgb,
        size: ExportSize::LongEdge(64),
        remove_gps: true,
        on_conflict: ConflictPolicy::Sequence,
    };
    let r = export(&core, &[v1, v2], &s, &out);
    assert_eq!((r.written, r.skipped, r.failed), (2, 0, 0), "{r:?}");
    let p1 = written_path(&r, 0);
    assert_eq!(p1, out.join("IMG_0001.jpg"));
    let img = image::open(&p1).unwrap();
    assert_eq!((img.width(), img.height()), (64, 48));
    let img2 = image::open(written_path(&r, 1)).unwrap();
    assert_eq!((img2.width(), img2.height()), (48, 64), "縦長は高さが長辺");
    assert!(icc_description(&p1).contains("sRGB"));
    assert!(!has_gps(&p1), "GPS を書かない（SEC-03）");

    // GPS を残す設定・TIFF 16bit・Display P3・元の寸法。
    let s2 = ExportSettings {
        format: ExportFormat::Tiff16,
        color_space: OutputColorSpace::DisplayP3,
        size: ExportSize::Original,
        remove_gps: false,
        on_conflict: ConflictPolicy::Sequence,
    };
    let r = export(&core, &[v1], &s2, &out);
    let tif = written_path(&r, 0);
    assert_eq!(tif.extension().unwrap(), "tif");
    let img = image::open(&tif).unwrap();
    assert_eq!((img.width(), img.height()), (160, 120));
    assert!(matches!(img, image::DynamicImage::ImageRgb16(_)));
    assert!(icc_description(&tif).contains("Display P3"));
    assert!(has_gps(&tif), "remove_gps = false なら GPS を書く");

    // PNG 16bit・Adobe RGB。
    let s3 = ExportSettings {
        format: ExportFormat::Png16,
        color_space: OutputColorSpace::AdobeRgb,
        size: ExportSize::LongEdge(100),
        remove_gps: true,
        on_conflict: ConflictPolicy::Sequence,
    };
    let r = export(&core, &[v1], &s3, &out);
    let png = written_path(&r, 0);
    let img = image::open(&png).unwrap();
    assert_eq!((img.width(), img.height()), (100, 75));
    assert!(icc_description(&png).contains("Adobe RGB"));

    // 衝突時の連番（同じ名前がある → -1、-2）。
    let r = export(&core, &[v1], &s, &out);
    assert_eq!(written_path(&r, 0), out.join("IMG_0001-1.jpg"));
    let r = export(&core, &[v1], &s, &out);
    assert_eq!(written_path(&r, 0), out.join("IMG_0001-2.jpg"));
    // スキップ。
    let skip = ExportSettings {
        on_conflict: ConflictPolicy::Skip,
        ..s
    };
    let r = export(&core, &[v1], &skip, &out);
    assert_eq!(r.skipped, 1);
    assert!(matches!(
        &r.items[0].outcome,
        ExportOutcome::Skipped { existing: Some(p), .. } if *p == out.join("IMG_0001.jpg")
    ));
    // 上書き（原本ではない書き出し済みのファイルは置き換えられる）。
    let overwrite = ExportSettings {
        on_conflict: ConflictPolicy::Overwrite,
        ..s
    };
    let r = export(&core, &[v1], &overwrite, &out);
    assert!(matches!(
        &r.items[0].outcome,
        ExportOutcome::Written { replaced: true, .. }
    ));

    // 原本と同じ場所・同じ名前への書き出しは拒否する（上書きの設定でも。6.4 節）。
    let r = export(&core, &[v1, v2], &overwrite, &env.photos);
    assert_eq!(r.failed, 2, "{r:?}");
    for item in &r.items {
        match &item.outcome {
            ExportOutcome::Failed { error } => {
                assert_eq!(error.kind, ErrorKind::ProtectedDestination, "{error:?}");
                assert!(error.user_actionable);
            }
            other => panic!("{other:?}"),
        }
    }
    // 連番の設定なら、原本の隣に別の名前で書く（原本は変えない）。
    let r = export(&core, &[v1], &s, &env.photos);
    assert_eq!(written_path(&r, 0), env.photos.join("IMG_0001-1.jpg"));
    std::fs::remove_file(env.photos.join("IMG_0001-1.jpg")).unwrap();

    // 書き出しは、保存済みの現像設定で行う（露光量を下げると暗くなる）。
    let base = image::open(out.join("IMG_0001.jpg")).unwrap().to_rgb8();
    core.paste_settings(
        &genzo_api::DevelopSettings {
            exposure_ev: -2.0,
            ..Default::default()
        },
        &[v1],
        Default::default(),
    )
    .unwrap();
    let dark_dir = env.dir.path().join("dark");
    let r = export(&core, &[v1], &s, &dark_dir);
    let dark = image::open(written_path(&r, 0)).unwrap().to_rgb8();
    let mean = |i: &image::RgbImage| i.as_raw().iter().map(|&v| f64::from(v)).sum::<f64>();
    assert!(mean(&dark) < mean(&base) * 0.8);

    core.close().unwrap();
    assert_eq!(
        hashes(&env.photos),
        before,
        "元ファイルは変わらない（DATA-01）"
    );
}

/// 書き出しの設定の誤りは、ジョブを投入する前に知らせる。
#[test]
fn invalid_export_settings_are_rejected() {
    let env = Env::new();
    let core = env.open();
    let bad = ExportSettings {
        format: ExportFormat::Jpeg { quality: 0 },
        ..Default::default()
    };
    let err = core
        .export(&[VariantId::new(1)], &bad, env.dir.path())
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    core.close().unwrap();
}

/// 名前が同じ別のフォルダの写真を「上書き」で書き出す設定（原本の照合の対象: 6.4 節）。
fn overwrite_original_size() -> ExportSettings {
    ExportSettings {
        format: ExportFormat::Jpeg { quality: 80 },
        size: ExportSize::Original,
        on_conflict: ConflictPolicy::Overwrite,
        ..Default::default()
    }
}

/// フォルダ `dir` の中のファイルの variant（マスター）。フォルダは名前で比べる（OS によるパスの表記の
/// 違い（ドライブ文字の大文字・小文字など）に左右されないように。テストのフォルダの名前はすべて違う）。
fn variant_in(core: &Core, dir: &Path) -> VariantId {
    let found: Vec<VariantId> = all_items(core)
        .into_iter()
        .filter(|s| s.is_master)
        .map(|s| s.variant_id)
        .filter(|&v| {
            core.variant_details(v).unwrap().files.iter().any(|f| {
                f.path
                    .as_deref()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    == dir.file_name()
            })
        })
        .collect();
    assert_eq!(found.len(), 1, "{} の写真: {found:?}", dir.display());
    found[0]
}

fn assert_protected(r: &ExportReport, what: &str) {
    assert_eq!((r.written, r.failed), (0, 1), "{what}: {r:?}");
    match &r.items[0].outcome {
        ExportOutcome::Failed { error } => {
            assert_eq!(
                error.kind,
                ErrorKind::ProtectedDestination,
                "{what}: {error:?}"
            )
        }
        other => panic!("{what}: {other:?}"),
    }
}

/// 書き出し先が、カタログに **別のパス**（シンボリックリンクを経由したフォルダ）で登録された原本の
/// 場所でも、上書きの設定で原本を置き換えない（6.4 節「パスの文字列だけでなく、ファイルの同一性でも
/// 照合する」。DATA-01）。書き出し先は、実際のフォルダ・別のリンク・`..` を含むパスの 3 通り。
#[cfg(unix)]
#[test]
fn overwrite_refuses_originals_registered_through_a_linked_folder() {
    let env = Env::new();
    let real = env.photos.join("real");
    let other = env.photos.join("other");
    write_jpeg(&real.join("IMG_0001.jpg"), (64, 48), 1, None);
    write_jpeg(&other.join("IMG_0001.jpg"), (48, 64), 2, None);
    let link = env.dir.path().join("link");
    let link2 = env.dir.path().join("link2");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    std::os::unix::fs::symlink(&real, &link2).unwrap();
    let before = hashes(&env.photos);
    let core = env.open();
    // real はリンクを経由して登録する（カタログにはリンクのパスが残る）。
    import(&core, &link);
    import(&core, &other);
    let v_other = variant_in(&core, &other);
    let s = overwrite_original_size();
    for (what, dest) in [
        ("実際のフォルダ", real.clone()),
        ("別のリンク", link2.clone()),
        ("`..` を含むパス", other.join("..").join("real")),
    ] {
        let r = export(&core, &[v_other], &s, &dest);
        assert_protected(&r, what);
    }
    core.close().unwrap();
    assert_eq!(
        hashes(&env.photos),
        before,
        "元ファイルは変わらない（DATA-01）"
    );
}

/// 大文字・小文字を区別しないファイルシステム（Windows・macOS の既定）では、書き出し先のフォルダ名の
/// 大文字・小文字が違っても、上書きの設定でカタログの原本を置き換えない（6.4 節）。区別するファイル
/// システム（Linux の既定など）では、別のフォルダになるので飛ばす。
#[test]
fn overwrite_refuses_originals_when_the_folder_differs_only_in_case() {
    let env = Env::new();
    let dir = env.photos.join("Case");
    let other = env.photos.join("other");
    write_jpeg(&dir.join("IMG_0001.jpg"), (64, 48), 1, None);
    let upper = env.photos.join("CASE");
    if !upper.join("IMG_0001.jpg").is_file() {
        eprintln!("大文字・小文字を区別するファイルシステムのため飛ばす");
        return;
    }
    write_jpeg(&other.join("IMG_0001.jpg"), (48, 64), 2, None);
    let before = hashes(&env.photos);
    let core = env.open();
    import(&core, &env.photos);
    let v_other = variant_in(&core, &other);
    let r = export(&core, &[v_other], &overwrite_original_size(), &upper);
    assert_protected(&r, "大文字のフォルダ名");
    core.close().unwrap();
    assert_eq!(hashes(&env.photos), before);
}
