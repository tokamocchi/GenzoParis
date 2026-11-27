//! 選別支援（ORG-06。[`genzo_api::Core::culling_assist`]）の結合テスト: 似た写真のグループと、その中でピントの
//! 甘い見込みの写真。何も変えない。

mod common;

use std::path::Path;

use common::{Env, JST_MINUTES, all_items, by_name, exif, import, wait_ok, write_jpeg};
use genzo_api::{ApiError, CullingOptions, CullingReport, JobResult, OutputColorSpace, VariantId};
use genzo_media::RgbImage8;

const W: u32 = 320;
const H: u32 = 214;

/// 大きな明暗の構図（`scene` で変わる）に細かい格子の模様を重ねた JPEG を書く。`blur` 回、3 × 3 の平均でぼかす
/// （構図は残り、細かい模様だけが消える）。
fn write_pattern(path: &Path, scene: u32, blur: usize, local: &str) {
    let mut px: Vec<f32> = (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            // 構図: 横に 9 つの帯の明るさを、`scene` ごとに違う並びにする。
            let band = x * 9 / W;
            let base = 60.0 + ((band * (scene + 3) * 7 + scene * 5) % 9) as f32 * 14.0;
            // 細かい模様（4 画素の格子。±18）。
            let fine = if (x / 4 + y / 4).is_multiple_of(2) {
                -18.0
            } else {
                18.0
            };
            base + fine
        })
        .collect();
    for _ in 0..blur {
        let src = px.clone();
        for y in 1..H - 1 {
            for x in 1..W - 1 {
                let mut sum = 0.0;
                for dy in 0..3 {
                    for dx in 0..3 {
                        sum += src[((y + dy - 1) * W + x + dx - 1) as usize];
                    }
                }
                px[(y * W + x) as usize] = sum / 9.0;
            }
        }
    }
    let rgb: Vec<u8> = px.iter().flat_map(|v| [*v as u8; 3]).collect();
    let image = RgbImage8::from_raw(W, H, rgb).unwrap();
    let e = exif(Some(local), Some(JST_MINUTES), None);
    let exif_bytes = genzo_media::exif_tiff_bytes(&e, W, H, OutputColorSpace::Srgb).unwrap();
    std::fs::write(
        path,
        genzo_media::jpeg::encode_jpeg(&image, 92, None, Some(&exif_bytes)).unwrap(),
    )
    .unwrap();
}

fn assist(core: &genzo_api::Core, ids: &[VariantId], options: &CullingOptions) -> CullingReport {
    match wait_ok(core, core.culling_assist(ids, options).unwrap()) {
        JobResult::CullingAssist(r) => r,
        other => panic!("{other:?}"),
    }
}

#[test]
fn similar_photos_are_grouped_and_soft_ones_are_found() {
    let env = Env::new();
    // 同じ構図を 3 枚続けて撮った（2 枚目はぼけている）。
    write_pattern(&env.photos.join("A1.jpg"), 1, 0, "2024-05-01 10:00:00");
    write_pattern(&env.photos.join("A2.jpg"), 1, 8, "2024-05-01 10:00:02");
    write_pattern(&env.photos.join("A3.jpg"), 1, 0, "2024-05-01 10:00:04");
    // 別の構図（時刻は近い）。
    write_pattern(&env.photos.join("B1.jpg"), 4, 0, "2024-05-01 10:00:06");
    // 同じ構図だが、時刻が離れている。
    write_pattern(&env.photos.join("C1.jpg"), 1, 0, "2024-05-01 11:00:00");
    // 撮影日時のない写真。
    write_jpeg(&env.photos.join("N.jpg"), (64, 48), 3, None);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let id = |n: &str| by_name(&items, n).variant_id;
    let all: Vec<VariantId> = items.iter().map(|i| i.variant_id).collect();

    let r = assist(&core, &all, &CullingOptions::default());
    assert_eq!(r.analyzed, 5, "{r:?}");
    assert_eq!(r.without_time, 1);
    assert_eq!(
        (r.skipped_videos, r.without_thumbnail, r.missing),
        (0, 0, 0)
    );
    assert!(!r.cancelled);
    assert_eq!(r.groups.len(), 1, "{r:?}");
    let g = &r.groups[0];
    assert_eq!(
        g.variant_ids,
        vec![id("A1.jpg"), id("A2.jpg"), id("A3.jpg")]
    );
    assert_eq!(g.soft, vec![id("A2.jpg")]);
    assert!(g.sharpest == id("A1.jpg") || g.sharpest == id("A3.jpg"));
    assert_eq!((r.grouped, r.soft), (3, 1));
    // 何も変えていない（スタック・評価）。
    assert!(all_items(&core).iter().all(|i| i.stack.is_none()));

    // 時刻の差を広げると、離れた同じ構図は…間に別の構図（B1）があるので続かない。違いを広く許すと、まとまる。
    let wide = assist(
        &core,
        &all,
        &CullingOptions {
            max_gap_seconds: 3600,
            max_distance: 32,
            ..CullingOptions::default()
        },
    );
    assert_eq!(wide.groups.len(), 1);
    assert_eq!(wide.groups[0].variant_ids.len(), 5);

    // JSON のコマンド。1 枚だけ・範囲外の設定は断る。
    let v: serde_json::Value = serde_json::from_str(&core.execute_json(
        &serde_json::json!({"type": "culling_assist", "variant_ids": all}).to_string(),
    ))
    .unwrap();
    assert_eq!(v["ok"]["type"], "job_started", "{v}");
    let JobResult::CullingAssist(again) = wait_ok(&core, v["ok"]["job_id"].as_u64().unwrap())
    else {
        panic!()
    };
    assert_eq!(again.groups, r.groups);
    assert!(matches!(
        core.culling_assist(&all[..1], &CullingOptions::default()),
        Err(ApiError::InvalidArgument(_))
    ));
    assert!(matches!(
        core.culling_assist(
            &all,
            &CullingOptions {
                soft_ratio: 1.5,
                ..CullingOptions::default()
            }
        ),
        Err(ApiError::InvalidArgument(_))
    ));
    core.close().unwrap();
}
