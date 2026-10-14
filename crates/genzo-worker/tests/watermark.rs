//! 透かし（EXP-03。`Job::RenderWatermark`）の結合テスト（実際に `genzo-worker` のバイナリを起動する）。
//!
//! - 文字の透かし（テストのフォント。`genzo_media::testing`）: 寸法・色・フォントにない文字・箱に収める
//! - 画像の透かし（透明度のある PNG）
//! - 共有メモリが足りない場合の確保し直し（作った透かしを覚えて送り直しに使う）
//! - 読めないフォント・ないファイルはそのジョブだけのエラー（ワーカーは動き続ける）

mod common;

use std::path::Path;

use common::config;
use genzo_jobs::CancellationToken;
use genzo_model::{Align, OutputColorSpace, RgbColor, WatermarkContent};
use genzo_worker::shm::SHM_HEADER_LEN;
use genzo_worker::{
    InitialShmBytes, WatermarkRequest, WorkerClient, WorkerClientError, WorkerErrorKind,
};

fn token() -> CancellationToken {
    CancellationToken::new()
}

fn text_request(font: &Path, text: &str, size_px: u32, max: (u32, u32)) -> WatermarkRequest {
    WatermarkRequest {
        content: WatermarkContent::Text {
            text: text.to_owned(),
            font: font.to_path_buf(),
            font_index: 0,
            color: RgbColor([255, 255, 255]),
        },
        size_px,
        max_width: max.0,
        max_height: max.1,
        align: Align::End,
        color_space: OutputColorSpace::Srgb,
    }
}

#[test]
fn text_and_image_watermarks_are_rendered_in_the_worker() {
    let root = tempfile::tempdir().unwrap();
    let font = root.path().join("test.ttf");
    std::fs::write(&font, genzo_media::testing::test_font()).unwrap();
    let mut cfg = config(root.path());
    // 最初はヘッダだけの大きさにして、必ず確保し直しが起きるようにする。
    cfg.initial_shm = InitialShmBytes {
        watermark: SHM_HEADER_LEN as u64,
        ..InitialShmBytes::default()
    };
    let mut c = WorkerClient::spawn(cfg).unwrap();

    // 1 回目は「容量不足」と「確保し直した後」の 2 回の応答、2 回目は覚えた大きさで 1 回。
    for expected_responses in [2, 1] {
        let before = c.stats().responses;
        let w = c
            .render_watermark(&text_request(&font, "AB漢", 100, (5000, 5000)), &token())
            .unwrap();
        assert_eq!(c.stats().responses - before, expected_responses);
        assert_eq!(w.missing, vec!['漢']);
        assert!(!w.shrunk);
        // A（600 × 700）・B・.notdef（送り幅 500）の 3 文字（em 1000 を 100 画素に）。
        assert!(
            w.bitmap.width() > 180 && w.bitmap.width() < 200,
            "{}",
            w.bitmap.width()
        );
        assert!(
            w.bitmap
                .rgba()
                .chunks_exact(4)
                .all(|p| p[..3] == [65_535; 3])
        );
        assert!(w.bitmap.rgba().chunks_exact(4).any(|p| p[3] == 65_535));
    }
    // 箱に収める。
    let small = c
        .render_watermark(&text_request(&font, "AB", 100, (40, 40)), &token())
        .unwrap();
    assert!(small.shrunk && small.bitmap.width() <= 42);

    // 画像（左半分が不透明の赤、右半分が透明）。
    let img = image::RgbaImage::from_fn(40, 20, |x, _| {
        image::Rgba(if x < 20 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 0]
        })
    });
    let logo = root.path().join("logo.png");
    img.save(&logo).unwrap();
    let w = c
        .render_watermark(
            &WatermarkRequest {
                content: WatermarkContent::Image { path: logo },
                size_px: 80,
                max_width: 1000,
                max_height: 1000,
                align: Align::Start,
                color_space: OutputColorSpace::Srgb,
            },
            &token(),
        )
        .unwrap();
    assert_eq!((w.bitmap.width(), w.bitmap.height()), (80, 40));
    let px = |x: u32, y: u32| {
        let i = ((y * 80 + x) * 4) as usize;
        w.bitmap.rgba()[i..i + 4].to_vec()
    };
    assert_eq!(px(5, 20)[3], 65_535);
    assert!(px(5, 20)[0] > 65_000 && px(5, 20)[1] < 500);
    assert_eq!(px(75, 20)[3], 0);

    // 読めないフォント・ないファイルは、そのジョブだけのエラー。
    let broken = root.path().join("broken.ttf");
    std::fs::write(&broken, b"not a font").unwrap();
    match c.render_watermark(&text_request(&broken, "A", 40, (99, 99)), &token()) {
        Err(WorkerClientError::Job(e)) => assert_eq!(e.kind, WorkerErrorKind::Decode, "{e}"),
        other => panic!("{other:?}"),
    }
    match c.render_watermark(
        &text_request(&root.path().join("none.ttf"), "A", 40, (99, 99)),
        &token(),
    ) {
        Err(WorkerClientError::Job(e)) => assert_eq!(e.kind, WorkerErrorKind::NotFound, "{e}"),
        other => panic!("{other:?}"),
    }
    // 大きさが範囲外の依頼は送らない。
    assert!(matches!(
        c.render_watermark(&text_request(&font, "A", 0, (99, 99)), &token()),
        Err(WorkerClientError::InvalidArgument(_))
    ));
    // ワーカーは動き続ける。
    c.render_watermark(&text_request(&font, "A", 40, (99, 99)), &token())
        .unwrap();
}
