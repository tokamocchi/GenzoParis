//! 書き出しの仕上げ（出力シャープ・透かし。EXP-03）と、動画の書き出し（元のファイルのコピー。EXP-05）の結合テスト
//! （04 の 2.4 節・6.4 節）。

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use common::{Env, all_items, by_name, import, wait_done, wait_ok};
use genzo_api::{
    CopyImportStage, Core, ExportFormat, ExportOutcome, ExportReport, ExportSettings, JobResult,
    OutputSharpening, RenameRule, RgbColor, SharpenAmount, SharpenMedia, VariantId, VideoExport,
    Watermark, WatermarkAnchor, WatermarkContent,
};
use genzo_model::ConflictPolicy;

fn export_with(
    core: &Core,
    ids: &[VariantId],
    s: &ExportSettings,
    dest: &Path,
    naming: Option<&RenameRule>,
) -> ExportReport {
    let job = core.export_with(ids, s, dest, naming).unwrap();
    match wait_ok(core, job) {
        JobResult::Export(r) => r,
        other => panic!("{other:?}"),
    }
}

fn written(r: &ExportReport, i: usize) -> (PathBuf, Vec<String>) {
    match &r.items[i].outcome {
        ExportOutcome::Written { path, warnings, .. } => (path.clone(), warnings.clone()),
        other => panic!("{other:?}"),
    }
}

/// 左が暗く右が明るい、縦の段差の PNG（8bit）。
fn write_step_png(path: &Path, (w, h): (u32, u32)) {
    let img = image::RgbImage::from_fn(w, h, |x, _| {
        if x < w / 2 {
            image::Rgb([50, 50, 50])
        } else {
            image::Rgb([200, 200, 200])
        }
    });
    img.save(path).unwrap();
}

fn read16(path: &Path) -> image::ImageBuffer<image::Rgb<u16>, Vec<u16>> {
    match image::open(path).unwrap() {
        image::DynamicImage::ImageRgb16(i) => i,
        other => panic!("16bit ではない: {:?}", other.color()),
    }
}

fn png16() -> ExportSettings {
    ExportSettings {
        format: ExportFormat::Png16,
        ..ExportSettings::default()
    }
}

/// 出力シャープ: 段差の両側の差が大きくなり、平らな部分は変わらない（16bit ではビット単位で同じ）。強いほど差が大きい。
/// 8bit の形式でも仕上げの後に 8bit にして書ける。
#[test]
fn output_sharpening_raises_edge_contrast_after_resizing() {
    let env = Env::new();
    write_step_png(&env.photos.join("STEP.png"), (64, 48));
    let core = env.open();
    import(&core, &env.photos);
    let v = by_name(&all_items(&core), "STEP.png").variant_id;
    let out = env.dir.path().join("out");

    let plain = export_with(&core, &[v], &png16(), &out.join("plain"), None);
    let (plain_path, _) = written(&plain, 0);
    let base = read16(&plain_path);
    let edge = |s: SharpenAmount, dir: &str| {
        let settings = ExportSettings {
            output_sharpening: Some(OutputSharpening {
                media: SharpenMedia::Screen,
                amount: s,
            }),
            ..png16()
        };
        let r = export_with(&core, &[v], &settings, &out.join(dir), None);
        let img = read16(&written(&r, 0).0);
        assert_eq!(img.dimensions(), base.dimensions());
        img
    };
    let low = edge(SharpenAmount::Low, "low");
    let high = edge(SharpenAmount::High, "high");
    let y = 24;
    let (l, r) = (31, 32);
    assert!(high.get_pixel(l, y)[0] < base.get_pixel(l, y)[0]);
    assert!(high.get_pixel(r, y)[0] > base.get_pixel(r, y)[0]);
    let diff = |i: &image::ImageBuffer<image::Rgb<u16>, Vec<u16>>| {
        i32::from(i.get_pixel(r, y)[0]) - i32::from(i.get_pixel(l, y)[0])
    };
    assert!(diff(&base) < diff(&low) && diff(&low) < diff(&high));
    // 平らな部分は変わらない。
    assert_eq!(high.get_pixel(3, y), base.get_pixel(3, y));
    assert_eq!(high.get_pixel(60, 3), base.get_pixel(60, 3));

    // 印刷用（解像度で半径を決める）・JPEG（仕上げの後に 8bit）。
    let jpeg = ExportSettings {
        format: ExportFormat::Jpeg { quality: 95 },
        resolution_ppi: 300,
        output_sharpening: Some(OutputSharpening {
            media: SharpenMedia::MattePaper,
            amount: SharpenAmount::Standard,
        }),
        ..ExportSettings::default()
    };
    let r = export_with(&core, &[v], &jpeg, &out.join("jpeg"), None);
    let img = image::open(written(&r, 0).0).unwrap();
    assert_eq!((img.width(), img.height()), (64, 48));
}

/// 透かし（文字）: 位置の周りだけが変わり、ほかの画素は変わらない。フォントにない文字は警告。フォントがなければ
/// 透かしなしで書き出さずに失敗にする。見本（watermark_preview）も作れる。
#[test]
fn text_watermark_is_drawn_and_unreadable_fonts_fail_the_photo() {
    let env = Env::new();
    write_step_png(&env.photos.join("STEP.png"), (120, 80));
    let font = env.dir.path().join("fonts").join("test.ttf");
    std::fs::create_dir_all(font.parent().unwrap()).unwrap();
    std::fs::write(&font, genzo_media::testing::test_font()).unwrap();
    let core = env.open();
    import(&core, &env.photos);
    let v = by_name(&all_items(&core), "STEP.png").variant_id;
    let out = env.dir.path().join("out");
    let plain = read16(&written(&export_with(&core, &[v], &png16(), &out.join("p"), None), 0).0);

    let mark = |text: &str, font: &Path| Watermark {
        anchor: WatermarkAnchor::TopLeft,
        size: 20,
        opacity: 100,
        margin: 5,
        ..Watermark::new(WatermarkContent::Text {
            text: text.to_owned(),
            font: font.to_path_buf(),
            font_index: 0,
            color: RgbColor([255, 0, 0]),
        })
    };
    let settings = ExportSettings {
        watermark: Some(mark("A漢", &font)),
        ..png16()
    };
    let r = export_with(&core, &[v], &settings, &out.join("w"), None);
    let (path, warnings) = written(&r, 0);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("ない文字") && w.contains('漢')),
        "{warnings:?}"
    );
    let img = read16(&path);
    // 短辺 80 の 20 % = 16 画素の文字、余白 4 画素。左上の「A」（em の 0.05〜0.65 × 0.1〜0.8）の中は赤。
    let p = img.get_pixel(9, 12);
    assert_eq!((p[0], p[1], p[2]), (65_535, 0, 0), "{p:?}");
    // 右下は変わらない。
    assert_eq!(img.get_pixel(110, 70), plain.get_pixel(110, 70));

    // フォントがない・読めない → 失敗（ファイルを書かない）。
    let broken = env.dir.path().join("fonts").join("broken.ttf");
    std::fs::write(&broken, b"not a font").unwrap();
    for (f, dir) in [
        (env.dir.path().join("none.ttf"), "none"),
        (broken, "broken"),
    ] {
        let s = ExportSettings {
            watermark: Some(mark("A", &f)),
            ..png16()
        };
        let r = export_with(&core, &[v], &s, &out.join(dir), None);
        assert_eq!((r.written, r.failed), (0, 1), "{dir}: {r:?}");
        let files = std::fs::read_dir(out.join(dir)).unwrap().count();
        assert_eq!(
            files, 0,
            "{dir}: 失敗した写真のファイル（一時ファイルを含む）を残さない"
        );
    }

    // 見本。
    let preview = core
        .watermark_preview(&mark("A漢", &font), Some((300, 200)))
        .unwrap();
    assert_eq!((preview.width, preview.height), (300, 200));
    assert!(preview.mark_width > 0 && preview.mark_height > 0);
    // PNG の先頭の印の base64。
    assert!(
        preview.png_base64.starts_with("iVBORw0KGgo"),
        "{}",
        &preview.png_base64[..16]
    );
    assert!(preview.warnings.iter().any(|w| w.contains('漢')));
    // 設定の誤りはジョブを始める前に。
    let bad = ExportSettings {
        watermark: Some(Watermark {
            opacity: 0,
            ..mark("A", &font)
        }),
        ..png16()
    };
    assert!(core.export(&[v], &bad, &out.join("bad")).is_err());
}

/// 動画（EXP-05）: 再エンコードせずにコピーする（中身・更新日時が同じ、元のファイルは変わらない）。写真と一緒に
/// 選んでも同じ書き出し先に置き、連番は両方を数える。同じ名前は上書きの設定でも上書きせずに連番を付ける。スキップ・
/// 書き出さない設定・GPS を削除する設定では飛ばす。全体のハッシュを記録する。ffmpeg がなければ飛ばす。
#[test]
fn videos_are_copied_without_reencoding() {
    let env = Env::new();
    write_step_png(&env.photos.join("A.png"), (48, 32));
    if !common::make_video(&env.photos.join("V.mp4"), "2024-05-01T09:00:00Z") {
        return;
    }
    let source = env.photos.join("V.mp4");
    let original = std::fs::read(&source).unwrap();
    let source_mtime = std::fs::metadata(&source).unwrap().modified().unwrap();
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.png").variant_id;
    let v = by_name(&items, "V.mp4").variant_id;
    let out = env.dir.path().join("out");
    let copy_videos = ExportSettings {
        remove_gps: false,
        ..ExportSettings::default()
    };
    let naming = RenameRule {
        template: "trip_{seq}".to_owned(),
        sequence_start: 1,
        sequence_digits: 2,
        ..RenameRule::default()
    };
    let r = export_with(&core, &[a, v], &copy_videos, &out, Some(&naming));
    assert_eq!(
        (r.written, r.copied, r.skipped, r.failed),
        (1, 1, 0, 0),
        "{r:?}"
    );
    assert_eq!(written(&r, 0).0, out.join("trip_01.jpg"));
    let ExportOutcome::Copied {
        path,
        bytes,
        full_hash,
        warnings,
    } = &r.items[1].outcome
    else {
        panic!("{:?}", r.items[1]);
    };
    assert_eq!(path, &out.join("trip_02.mp4"), "拡張子は元のファイルのまま");
    assert_eq!(*bytes, original.len() as u64);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(std::fs::read(path).unwrap(), original, "再エンコードしない");
    assert_eq!(
        std::fs::metadata(path).unwrap().modified().unwrap(),
        source_mtime
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        original,
        "元のファイルは変わらない"
    );
    assert_eq!(full_hash.len(), 64);
    // 全体のハッシュを記録した（ORG-03）。
    let details = core.variant_details(v).unwrap();
    assert_eq!(
        details.files[0].full_hash.as_deref(),
        Some(full_hash.as_str())
    );

    // 同じ名前: 上書きの設定でも上書きしない（連番）。スキップの設定なら飛ばす。名前の規則がなければ元の名前。
    let again = out.join("again");
    let overwrite = ExportSettings {
        on_conflict: ConflictPolicy::Overwrite,
        ..copy_videos.clone()
    };
    let r = export_with(&core, &[v], &overwrite, &again, None);
    assert_eq!(r.copied, 1, "{r:?}");
    assert!(again.join("V.mp4").exists());
    let r = export_with(&core, &[v], &overwrite, &again, None);
    let ExportOutcome::Copied { path, warnings, .. } = &r.items[0].outcome else {
        panic!("{:?}", r.items[0]);
    };
    assert_eq!(path, &again.join("V-1.mp4"), "{r:?}");
    assert!(
        warnings.iter().any(|w| w.contains("上書きしません")),
        "{warnings:?}"
    );
    assert_eq!(std::fs::read(again.join("V.mp4")).unwrap(), original);
    let skip = ExportSettings {
        on_conflict: ConflictPolicy::Skip,
        ..copy_videos.clone()
    };
    let r = export_with(&core, &[v], &skip, &again, None);
    assert!(
        matches!(
            &r.items[0].outcome,
            ExportOutcome::Skipped {
                existing: Some(_),
                ..
            }
        ),
        "{r:?}"
    );
    // 同じ動画を 2 回選んでも 1 回だけ（重なりは 1 件にまとめる）。
    let r = export_with(&core, &[v, v], &copy_videos, &out.join("twice"), None);
    assert_eq!(r.copied, 1, "{r:?}");
    // 書き出さない設定・GPS を削除する設定（既定の画面）。動画は連番に数えない。
    for (s, needle) in [
        (
            ExportSettings {
                videos: VideoExport::Skip,
                ..copy_videos.clone()
            },
            "書き出さない設定",
        ),
        (
            ExportSettings {
                remove_gps: true,
                ..ExportSettings::default()
            },
            "位置情報",
        ),
    ] {
        let dir = out.join(needle);
        let r = export_with(&core, &[v, a], &s, &dir, Some(&naming));
        assert_eq!((r.written, r.copied, r.skipped), (1, 0, 1), "{r:?}");
        match &r.items[0].outcome {
            ExportOutcome::Skipped { reason, .. } => assert!(reason.contains(needle), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(written(&r, 1).0, dir.join("trip_01.jpg"));
    }
    // 一時ファイルを残さない。
    for e in std::fs::read_dir(&out).unwrap() {
        let name = e.unwrap().file_name().to_string_lossy().into_owned();
        assert!(!name.starts_with(".genzo-"), "{name}");
    }
}

/// コピーした動画の結果（パス・全体のハッシュ・警告）。
fn copied(r: &ExportReport, i: usize) -> (PathBuf, String, Vec<String>) {
    match &r.items[i].outcome {
        ExportOutcome::Copied {
            path,
            full_hash,
            warnings,
            ..
        } => (path.clone(), full_hash.clone(), warnings.clone()),
        other => panic!("{other:?}"),
    }
}

fn set_mtime(path: &Path, t: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(t)
        .unwrap();
}

/// フォルダの中の名前（並べたもの）。
fn names_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = match std::fs::read_dir(dir) {
        Ok(r) => r
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect(),
        Err(_) => Vec::new(),
    };
    v.sort();
    v
}

/// 動画の書き出しの取り消し（EXP-05。決定事項ログ No.168）: コピーの途中（一時ファイルに書いた後、照合の前）で
/// 取り消すと、一時ファイルを消し、動画も後の写真も書き出さず、結果は取り消し。元のファイルは変えない（検証の
/// ラウンド 1 の指摘）。
#[test]
fn cancelling_a_video_copy_leaves_nothing_behind() {
    let env = Env::new();
    write_step_png(&env.photos.join("A.png"), (48, 32));
    let source = env.photos.join("V.mp4");
    if !common::make_video(&source, "2024-05-01T09:00:00Z") {
        return;
    }
    let original = std::fs::read(&source).unwrap();
    let (reached_tx, reached_rx) = mpsc::channel::<PathBuf>();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let reached_tx = parking_lot::Mutex::new(reached_tx);
    let go_rx = parking_lot::Mutex::new(go_rx);
    let core = env.open_with_hooks(|h| {
        h.on_copy_import = Some(Arc::new(move |stage: &CopyImportStage| {
            if let CopyImportStage::Written(tmp) = stage {
                // テストが取り消すまで待つ。
                reached_tx.lock().send(tmp.clone()).unwrap();
                go_rx.lock().recv_timeout(Duration::from_secs(60)).unwrap();
            }
            Ok(())
        }));
    });
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.png").variant_id;
    let v = by_name(&items, "V.mp4").variant_id;
    let out = env.dir.path().join("out");
    let job = core
        .export_with(&[v, a], &ExportSettings::default(), &out, None)
        .unwrap();
    let tmp = reached_rx.recv_timeout(Duration::from_secs(120)).unwrap();
    assert!(tmp.exists(), "一時ファイルに書いた後: {}", tmp.display());
    assert_eq!(
        tmp.parent(),
        Some(out.as_path()),
        "書き出し先と同じフォルダ"
    );
    core.cancel_job(job).unwrap();
    go_tx.send(()).unwrap();
    let info = wait_done(&core, job);
    let Some(JobResult::Export(r)) = info.result else {
        panic!("{info:?}");
    };
    assert!(r.cancelled, "{r:?}");
    assert_eq!((r.written, r.copied, r.failed), (0, 0, 0), "{r:?}");
    assert!(
        r.items.is_empty(),
        "取り消した動画・後の写真は結果に入れない: {r:?}"
    );
    // 一時ファイルも、動画・写真のファイルも残さない。
    assert!(!tmp.exists());
    assert_eq!(names_in(&out), Vec::<String>::new());
    assert_eq!(std::fs::read(&source).unwrap(), original);
    // 全体のハッシュは記録しない（コピーを確かめていない）。
    assert_eq!(core.variant_details(v).unwrap().files[0].full_hash, None);
    core.close().unwrap();
}

/// 動画の元のファイルの変化（EXP-05。決定事項ログ No.168）: 登録の後に変わっていれば、警告を付けて今のファイルを
/// コピーし、全体のハッシュは記録し直さない。登録の内容（大きさ・更新日時・先頭と末尾）のままで途中だけが変わって
/// いれば、記録した全体のハッシュと違うことを警告する（検証のラウンド 1 の指摘）。
#[test]
fn changed_video_sources_are_copied_with_warnings() {
    let env = Env::new();
    let source = env.photos.join("V.mp4");
    if !common::make_video(&source, "2024-05-01T09:00:00Z") {
        return;
    }
    // クイックハッシュ（先頭と末尾の 64 KiB）に入らない途中の部分を作るため、末尾に 0 を足す（ffprobe は読める）。
    std::fs::File::options()
        .append(true)
        .open(&source)
        .unwrap()
        .write_all(&vec![0u8; 256 * 1024])
        .unwrap();
    let core = env.open();
    import(&core, &env.photos);
    let v = by_name(&all_items(&core), "V.mp4").variant_id;
    let out = env.dir.path().join("out");
    let settings = ExportSettings::default();
    let r = export_with(&core, &[v], &settings, &out.join("1"), None);
    let (_, first_hash, warnings) = copied(&r, 0);
    assert!(warnings.is_empty(), "{warnings:?}");
    let recorded = core.variant_details(v).unwrap().files[0]
        .full_hash
        .clone()
        .expect("全体のハッシュを記録する");
    assert_eq!(recorded, first_hash);

    // 途中だけを変える（大きさ・更新日時・先頭と末尾は同じ）。
    let mtime = std::fs::metadata(&source).unwrap().modified().unwrap();
    let mut bytes = std::fs::read(&source).unwrap();
    let mid = bytes.len() / 2;
    assert!(mid > 64 * 1024 && mid + 64 * 1024 < bytes.len());
    bytes[mid] ^= 0xFF;
    std::fs::write(&source, &bytes).unwrap();
    set_mtime(&source, mtime);
    let r = export_with(&core, &[v], &settings, &out.join("2"), None);
    let (path, hash, warnings) = copied(&r, 0);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("記録した内容と違います")),
        "{warnings:?}"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "今のファイルをコピーする"
    );
    assert_ne!(hash, recorded);
    assert_eq!(
        core.variant_details(v).unwrap().files[0]
            .full_hash
            .as_deref(),
        Some(recorded.as_str()),
        "記録は変えない"
    );

    // 登録の後に変わった（更新日時）。
    set_mtime(&source, mtime + Duration::from_secs(60));
    let r = export_with(&core, &[v], &settings, &out.join("3"), None);
    let (path, _, warnings) = copied(&r, 0);
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("登録の後に元のファイルが変わっています")),
        "{warnings:?}"
    );
    assert!(
        !warnings
            .iter()
            .any(|w| w.contains("記録した内容と違います")),
        "{warnings:?}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        core.variant_details(v).unwrap().files[0]
            .full_hash
            .as_deref(),
        Some(recorded.as_str())
    );
    core.close().unwrap();
}
