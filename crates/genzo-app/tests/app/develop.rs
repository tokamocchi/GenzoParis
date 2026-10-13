//! (c) 現像: OpenDevelop → SetSettings → WaitForFrame → 画素のバイト列（frames の形）のヘッダーの寸法と
//! 画素の長さが合う。画面の色空間（Display P3 / sRGB）の切り替え。ビフォー / アフター（DEV-31）の
//! ビフォーの画素のバイト列。

use genzo_api::{DevelopSettings, FrameInfo, RegionInfo, Response, VariantId};
use genzo_app::commands;
use genzo_app::state::{AppState, DisplayTarget};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::support::{Env, WAIT_LIMIT, all_items, import, ok, write_image};

const SRGB: &str = "GenzoParis sRGB (IEC 61966-2-1)";
const DISPLAY_P3: &str = "GenzoParis Display P3";

/// frames の形のバイト列（ヘッダーの長さ・ヘッダーの JSON・RGBA の画素）を読む。寸法と画素の長さが
/// 合うこと、不透明であることを確かめる。
fn decode<H: DeserializeOwned>(bytes: &[u8], size: impl Fn(&H) -> (u32, u32)) -> (H, Vec<u8>) {
    assert!(bytes.len() > 4, "空でない");
    let len = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let header: H = serde_json::from_slice(&bytes[4..4 + len]).expect("ヘッダーの JSON");
    let pixels = bytes[4 + len..].to_vec();
    let (w, h) = size(&header);
    assert!(w > 0 && h > 0);
    assert_eq!(pixels.len(), w as usize * h as usize * 4, "幅 × 高さ × 4");
    assert!(pixels.chunks_exact(4).all(|p| p[3] == 255), "不透明");
    (header, pixels)
}

fn set_settings(state: &AppState, settings: &DevelopSettings) -> u64 {
    match ok(
        state,
        json!({"type": "set_settings", "settings": settings, "dragging": false}),
    ) {
        Response::Generation { generation } => generation,
        other => panic!("{other:?}"),
    }
}

fn wait_frame(state: &AppState, generation: u64) -> FrameInfo {
    let timeout_ms = WAIT_LIMIT.as_millis() as u64;
    match ok(
        state,
        json!({"type": "wait_for_frame", "generation": generation, "timeout_ms": timeout_ms}),
    ) {
        Response::FrameOrNone { frame: Some(f) } if f.generation >= generation => *f,
        other => panic!("世代 {generation} のプレビューが描けない: {other:?}"),
    }
}

/// 今の描画の世代（DevelopState）。
fn generation(state: &AppState) -> u64 {
    match ok(state, json!({"type": "develop_state"})) {
        Response::DevelopOrNone { state: Some(s) } => s.generation,
        other => panic!("{other:?}"),
    }
}

/// 現像のプレビューのバイト列を読んで、描いた情報と合うことを確かめる。
fn develop_pixels(state: &AppState, id: VariantId, frame: &FrameInfo) -> Vec<u8> {
    let bytes = commands::develop_frame_bytes(state).unwrap();
    let (header, pixels) = decode::<FrameInfo>(&bytes, |h| (h.width, h.height));
    assert_eq!(header.variant_id, id);
    assert!(header.generation >= frame.generation);
    assert_eq!((header.width, header.height), (frame.width, frame.height));
    assert!(
        !header.display_assumed_srgb,
        "画面のプロファイルを指定している"
    );
    pixels
}

pub fn develop_and_region_frames_match_their_headers() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("現像 の 写真");
    write_image(&photos.join("夕焼け の 空.jpg"), (160, 120), 5);
    let path = env.catalog("現像.db");

    // 既定は sRGB（開いたカタログに指定する）。
    let status = state.open_catalog(&path, true).unwrap();
    assert!(!status.display.assumed_srgb, "{:?}", status.display);
    assert_eq!(status.display.description.as_deref(), Some(SRGB));
    import(&state, &photos);
    let id = all_items(&state)[0].variant_id;
    assert!(
        commands::develop_frame_bytes(&state).unwrap().is_empty(),
        "現像中の写真がなければ空"
    );
    assert!(commands::region_frame_bytes(&state).unwrap().is_empty());

    let dev = match ok(&state, json!({"type": "open_develop", "variant_id": id})) {
        Response::Develop(d) => d,
        other => panic!("{other:?}"),
    };
    assert_eq!(dev.variant_id, id);
    let mut settings = dev.settings.clone();
    settings.exposure_ev = 0.5;
    let g = set_settings(&state, &settings);
    let frame = wait_frame(&state, g);
    assert_eq!(frame.variant_id, id);
    let srgb = develop_pixels(&state, id, &frame);

    // Display P3 にすると、開いているカタログにすぐ反映し、同じ設定を P3 の画素で描き直す。
    let info = state
        .set_display_target(DisplayTarget::DisplayP3)
        .unwrap()
        .expect("開いているカタログに反映する");
    assert!(!info.assumed_srgb, "{info:?}");
    assert_eq!(info.description.as_deref(), Some(DISPLAY_P3));
    assert_eq!(state.status().unwrap().unwrap().display, info);
    let frame = wait_frame(&state, generation(&state));
    let p3 = develop_pixels(&state, id, &frame);
    assert_eq!(p3.len(), srgb.len());
    assert_ne!(p3, srgb, "彩度の高い色は、P3 と sRGB で値が違う");

    // 表示範囲の描画（ルーペの等倍）も、ヘッダーの寸法と画素の長さが合う。
    let region = match ok(
        &state,
        json!({"type": "render_region", "request": {
            "variant_id": id, "x": 10.0, "y": 20.0, "width": 80.0, "height": 60.0, "scale": 1.0
        }}),
    ) {
        Response::Region(r) => r,
        other => panic!("{other:?}"),
    };
    assert_eq!((region.width, region.height), (80, 60));
    let bytes = commands::region_frame_bytes(&state).unwrap();
    let (header, _) = decode::<RegionInfo>(&bytes, |h| (h.width, h.height));
    assert_eq!(header, *region);
    assert!(!header.display_assumed_srgb);

    // 閉じている間に指定した色空間は、次に開いたときに使う。
    ok(&state, json!({"type": "close_develop"}));
    state.close_catalog(false).unwrap();
    assert_eq!(state.set_display_target(DisplayTarget::Srgb).unwrap(), None);
    let status = state.open_catalog(&path, false).unwrap();
    assert_eq!(status.display.description.as_deref(), Some(SRGB));
    state.close_catalog(false).unwrap();
    assert_eq!(
        state.set_display_target(DisplayTarget::DisplayP3).unwrap(),
        None
    );
    let status = state.open_catalog(&path, false).unwrap();
    assert_eq!(status.display.description.as_deref(), Some(DISPLAY_P3));
    state.close_catalog(false).unwrap();
}

/// ビフォー / アフター（DEV-31）: set_before → wait_for_before → before_frame のバイト列（ヘッダーは FrameInfo）。
/// ビフォーの表示範囲（before_region_frame。ヘッダーの before が真）。画面の色空間を変えるとビフォーも描き直す。
pub fn before_frames_match_their_headers() {
    let env = Env::new();
    let state = env.state();
    let photos = env.photos("ビフォー");
    write_image(&photos.join("朝.jpg"), (160, 120), 7);
    let path = env.catalog("ビフォー.db");
    state.open_catalog(&path, true).unwrap();
    import(&state, &photos);
    let id = all_items(&state)[0].variant_id;
    ok(&state, json!({"type": "open_develop", "variant_id": id}));
    assert!(
        commands::before_frame_bytes(&state).unwrap().is_empty(),
        "設定していなければ空"
    );
    let mut settings = DevelopSettings {
        exposure_ev: 1.0,
        ..Default::default()
    };
    settings.geometry.crop.left = 0.5;
    let g = set_settings(&state, &settings);
    let after = wait_frame(&state, g);
    let info = match ok(
        &state,
        json!({"type": "set_before", "source": {"kind": "initial"}}),
    ) {
        Response::Before { before: Some(b) } => b,
        other => panic!("{other:?}"),
    };
    let timeout_ms = WAIT_LIMIT.as_millis() as u64;
    let wait_before = |generation: u64| match ok(
        &state,
        json!({"type": "wait_for_before", "generation": generation, "timeout_ms": timeout_ms}),
    ) {
        Response::FrameOrNone { frame: Some(f) } if f.generation >= generation => *f,
        other => panic!("ビフォーが描けない: {other:?}"),
    };
    let bf = wait_before(info.generation);
    let (header, srgb) = decode::<FrameInfo>(&commands::before_frame_bytes(&state).unwrap(), |h| {
        (h.width, h.height)
    });
    assert_eq!(header.variant_id, id);
    assert_eq!((header.width, header.height), (after.width, after.height));
    assert_eq!(header.generation, bf.generation);

    // 表示範囲。
    let region = match ok(
        &state,
        json!({"type": "render_before_region", "request": {
            "variant_id": id, "x": 0.0, "y": 0.0, "width": 40.0, "height": 30.0
        }}),
    ) {
        Response::Region(r) => r,
        other => panic!("{other:?}"),
    };
    assert!(region.before);
    let (rh, _) =
        decode::<RegionInfo>(&commands::before_region_frame_bytes(&state).unwrap(), |h| {
            (h.width, h.height)
        });
    assert_eq!(rh, *region);

    // 画面の色空間を変えると、ビフォーも描き直す。
    state.set_display_target(DisplayTarget::DisplayP3).unwrap();
    let st = match ok(&state, json!({"type": "develop_state"})) {
        Response::DevelopOrNone { state: Some(s) } => s,
        other => panic!("{other:?}"),
    };
    let g2 = st.before.expect("ビフォーは続く").generation;
    assert!(g2 > info.generation);
    wait_before(g2);
    let (_, p3) = decode::<FrameInfo>(&commands::before_frame_bytes(&state).unwrap(), |h| {
        (h.width, h.height)
    });
    assert_ne!(p3, srgb);
    ok(&state, json!({"type": "close_develop"}));
    state.close_catalog(false).unwrap();
}
