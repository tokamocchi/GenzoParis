//! v1 のパッケージ「view」の結合テスト: 説明のメタデータの編集（LIB-15。一括編集・まとめ・テキスト検索・
//! JSON のコマンド・書き出しの XMP / Exif）と、ビフォー / アフター（DEV-31。ビフォーの描画・表示範囲・
//! 差し替え）。

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::{Env, WAIT_LIMIT, all_items, by_name, drain, import, wait_ok, write_jpeg};
use genzo_api::{
    ApiError, BeforeSource, CatalogChange, Command, Core, DevelopSettings, Event, ExportFormat,
    ExportOutcome, ExportSettings, FrameInfo, JobResult, RegionRequest, RegionSpace, Response,
    SearchFilter, SearchSort, VariantId,
};
use genzo_model::{CropRect, HistoryEntryId, MetadataField as F};

fn search_text(core: &Core, text: &str) -> u64 {
    core.search(
        &SearchFilter {
            text: Some(text.to_owned()),
            ..Default::default()
        },
        SearchSort::default(),
    )
    .unwrap()
    .count
}

fn change(field: F, value: &str) -> (F, Option<String>) {
    (field, Some(value.to_owned()))
}

/// 一括編集（仮想コピーは同じ asset）・まとめ（複数の値）・詳細・テキスト検索・イベント・元ファイルは変えない。
#[test]
fn metadata_is_edited_in_bulk_and_searchable() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    write_jpeg(&env.photos.join("B.jpg"), (64, 48), 2, None);
    let before = common::hashes(&env.photos);
    let core = env.open();
    import(&core, &env.photos);
    let items = all_items(&core);
    let a = by_name(&items, "A.jpg").variant_id;
    let b = by_name(&items, "B.jpg").variant_id;
    let copy = core.create_virtual_copy(a, Some("白黒")).unwrap();
    let rx = core.subscribe();

    let n = core
        .set_metadata(
            &[a, b, copy],
            &[
                change(F::Creator, "山田 太郎"),
                change(F::Copyright, "© 2024 山田 太郎"),
                change(F::CopyrightStatus, "copyrighted"),
            ],
        )
        .unwrap();
    assert_eq!(n, 2, "仮想コピーは同じ asset なので 2 件");
    core.set_metadata(&[a], &[change(F::Title, "清水寺の朝")])
        .unwrap();
    let events = common::drain(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            Event::CatalogChanged {
                change: CatalogChange::Metadata,
                ..
            }
        )),
        "{events:?}"
    );

    // 詳細（仮想コピーも同じ値。キャプションは MVP の項目と同じ）。
    let d = core.variant_details(copy).unwrap();
    assert_eq!(d.metadata.title.as_deref(), Some("清水寺の朝"));
    assert_eq!(d.metadata.creator.as_deref(), Some("山田 太郎"));
    core.set_caption(&[a], Some("京都")).unwrap();
    let d = core.variant_details(a).unwrap();
    assert_eq!(d.caption.as_deref(), Some("京都"));
    assert_eq!(d.metadata.caption.as_deref(), Some("京都"));

    // まとめ: 作成者は共通、タイトルは A だけ（複数の値）。
    let s = core.metadata_summary(&[a, b, copy]).unwrap();
    assert_eq!(s.assets, 2);
    let field = |f: F| s.fields.iter().find(|x| x.field == f).unwrap();
    assert_eq!(field(F::Creator).value.as_deref(), Some("山田 太郎"));
    assert!(!field(F::Creator).mixed);
    assert!(field(F::Title).mixed);
    assert_eq!(field(F::Title).value, None);

    // テキスト検索（タイトル・作成者は対象。仮想コピーも見つかる）。
    assert_eq!(search_text(&core, "清水寺"), 2, "A とその仮想コピー");
    assert_eq!(search_text(&core, "山田"), 3);

    // 不正な値は何も変えない。
    let err = core
        .set_metadata(
            &[b],
            &[change(F::Title, "x"), change(F::CopyrightStatus, "maybe")],
        )
        .unwrap_err();
    assert_eq!(err.kind(), genzo_api::ErrorKind::InvalidInput, "{err}");
    assert_eq!(core.variant_details(b).unwrap().metadata.title, None);

    // 削除（null）。
    core.set_metadata(&[a], &[(F::Title, None)]).unwrap();
    assert_eq!(search_text(&core, "清水寺"), 0);
    core.close().unwrap();
    assert_eq!(common::hashes(&env.photos), before, "元ファイルは変えない");
}

/// JSON のコマンド（UI と同じ経路）: set_metadata（変える項目だけ・null で削除）と metadata_summary。
#[test]
fn metadata_commands_round_trip_as_json() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    let cmd: Command = serde_json::from_value(serde_json::json!({
        "type": "set_metadata",
        "variant_ids": [a.get()],
        "changes": { "title": "  夕焼け  ", "city": "京都市", "creator": null }
    }))
    .unwrap();
    assert_eq!(core.execute(cmd).unwrap(), Response::Updated { count: 1 });
    let r = core
        .execute(Command::MetadataSummary {
            variant_ids: vec![a],
        })
        .unwrap();
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["type"], "metadata");
    assert_eq!(json["assets"], 1);
    let title = json["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["field"] == "title")
        .unwrap();
    assert_eq!(title["value"], "夕焼け");
    assert_eq!(title["mixed"], false);
    // 詳細の JSON にも metadata がある。
    let d = core
        .execute(Command::VariantDetails { variant_id: a })
        .unwrap();
    let dj = serde_json::to_value(&d).unwrap();
    assert_eq!(dj["metadata"]["city"], "京都市");
    assert!(dj["metadata"]["creator"].is_null());
    // 知らない項目の名前は解釈の誤り。
    let bad = serde_json::from_value::<Command>(serde_json::json!({
        "type": "set_metadata", "variant_ids": [1], "changes": { "nope": "x" }
    }));
    assert!(bad.is_err());
    let mut changes = BTreeMap::new();
    changes.insert(F::Headline, Some("見出し".to_owned()));
    let typed = Command::SetMetadata {
        variant_ids: vec![a],
        changes,
    };
    let round: Command = serde_json::from_value(serde_json::to_value(&typed).unwrap()).unwrap();
    assert_eq!(round, typed);
    core.close().unwrap();
}

fn export_one(core: &Core, v: VariantId, format: ExportFormat, dest: &Path) -> std::path::PathBuf {
    let s = ExportSettings {
        format,
        ..Default::default()
    };
    let job = core.export(&[v], &s, dest).unwrap();
    let JobResult::Export(r) = wait_ok(core, job) else {
        panic!("書き出しの結果ではない");
    };
    match &r.items[0].outcome {
        ExportOutcome::Written { path, .. } => path.clone(),
        other => panic!("{other:?}"),
    }
}

/// 書き出したファイルに、カタログの説明のメタデータが XMP（すべての項目）と Exif（ASCII の値だけ）で入る。
#[test]
fn export_embeds_catalog_metadata_as_xmp_and_exif() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (64, 48), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let a = all_items(&core)[0].variant_id;
    core.set_metadata(
        &[a],
        &[
            change(F::Title, "清水寺 & 朝"),
            change(F::Caption, "Morning at Kiyomizu"),
            change(F::Creator, "山田 太郎"),
            change(F::Copyright, "(C) 2024 Taro Yamada"),
            change(F::CreatorEmail, "taro@example.com"),
            change(F::City, "京都市"),
        ],
    )
    .unwrap();
    let out = env.dir.path().join("out");
    let jpeg = export_one(&core, a, ExportFormat::Jpeg { quality: 90 }, &out);
    let bytes = std::fs::read(&jpeg).unwrap();
    let xmp_start = bytes
        .windows(10)
        .position(|w| w == b"<x:xmpmeta")
        .expect("XMP がある");
    let xmp_end = bytes
        .windows(12)
        .position(|w| w == b"</x:xmpmeta>")
        .unwrap();
    let xmp = String::from_utf8(bytes[xmp_start..xmp_end + 12].to_vec()).unwrap();
    assert!(xmp.contains("清水寺 &amp; 朝"), "{xmp}");
    assert!(xmp.contains("<rdf:li>山田 太郎</rdf:li>"));
    assert!(xmp.contains("<photoshop:City>京都市</photoshop:City>"));
    assert!(xmp.contains("<Iptc4xmpCore:CiEmailWork>taro@example.com</Iptc4xmpCore:CiEmailWork>"));
    let e = exif::Reader::new()
        .read_from_container(&mut std::io::Cursor::new(&bytes))
        .unwrap();
    let ascii = |t: exif::Tag| {
        e.get_field(t, exif::In::PRIMARY)
            .map(|f| f.display_value().to_string())
    };
    assert_eq!(
        ascii(exif::Tag::ImageDescription).as_deref(),
        Some("\"Morning at Kiyomizu\"")
    );
    assert_eq!(
        ascii(exif::Tag::Copyright).as_deref(),
        Some("\"(C) 2024 Taro Yamada\"")
    );
    assert_eq!(
        ascii(exif::Tag::Artist),
        None,
        "日本語の作成者は Exif に書かない"
    );
    core.close().unwrap();
}

// ---------------------------------------------------------------------------
// ビフォー / アフター（DEV-31）
// ---------------------------------------------------------------------------

/// 設定を変えて、その世代のプレビューを待つ。
fn render(core: &Core, s: &DevelopSettings) -> FrameInfo {
    let g = core.set_settings(s, false).unwrap();
    core.wait_for_frame(g, WAIT_LIMIT)
        .unwrap()
        .expect("描けている")
}

/// ビフォーの世代のプレビューを待つ。
fn wait_before(core: &Core, generation: u64) -> FrameInfo {
    core.wait_for_before(generation, WAIT_LIMIT)
        .unwrap()
        .expect("ビフォーが描けている")
}

fn edited() -> DevelopSettings {
    let mut s = DevelopSettings {
        exposure_ev: 1.5,
        contrast: 30.0,
        ..Default::default()
    };
    s.geometry.crop = CropRect {
        left: 0.25,
        top: 0.0,
        right: 1.0,
        bottom: 0.75,
    };
    s
}

/// 既定のビフォーは取り込んだ直後の設定（既定の設定）で、切り抜きはアフターと同じ。アフターだけの変更では
/// 描き直さず、角度・出どころを変えると描き直す。今の設定の写し・やめる・開き直すとやめた状態。
#[test]
fn before_uses_the_initial_settings_with_the_after_geometry() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (160, 120), 1, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    assert!(matches!(
        core.set_before(Some(BeforeSource::Initial)),
        Err(ApiError::NoDevelopSession)
    ));
    core.open_develop(v).unwrap();
    let after = render(&core, &edited());
    let rx = core.subscribe();

    let info = core
        .set_before(Some(BeforeSource::Initial))
        .unwrap()
        .unwrap();
    assert_eq!(info.label, "読み込み");
    assert_eq!(info.source, BeforeSource::Initial);
    let bf = wait_before(&core, info.generation);
    assert_eq!(
        (bf.width, bf.height),
        (after.width, after.height),
        "切り抜きはアフターと同じ"
    );
    assert_eq!(
        (bf.output_width, bf.output_height),
        (after.output_width, after.output_height)
    );
    assert_eq!(bf.histogram.pixel_count, 0, "ヒストグラムは求めない");
    let before_px = core.before_frame().unwrap().display_rgb8;
    let after_px = core.develop_frame().unwrap().display_rgb8;
    let mean = |p: &[u8]| p.iter().map(|&x| u64::from(x)).sum::<u64>() as f64 / p.len() as f64;
    assert!(
        mean(&before_px) + 10.0 < mean(&after_px),
        "ビフォーは露光量 +1.5 の前"
    );
    assert!(
        drain(&rx)
            .iter()
            .any(|e| matches!(e, Event::BeforeUpdated { variant_id, .. } if *variant_id == v))
    );
    // ビフォーの画素は「既定の設定 ＋ 同じ切り抜き」のアフターと同じ。
    let plain = DevelopSettings {
        geometry: edited().geometry,
        ..DevelopSettings::default()
    };
    render(&core, &plain);
    assert_eq!(core.develop_frame().unwrap().display_rgb8, before_px);

    // アフターだけの変更（露光量）では描き直さない。
    let g0 = core
        .develop_state()
        .unwrap()
        .unwrap()
        .before
        .unwrap()
        .generation;
    render(&core, &edited());
    let st = core.develop_state().unwrap().unwrap();
    assert_eq!(st.before.as_ref().unwrap().generation, g0);
    assert!(
        !drain(&rx)
            .iter()
            .any(|e| matches!(e, Event::BeforeUpdated { .. }))
    );
    // 角度を変えると描き直す。
    let mut rotated = edited();
    rotated.geometry.angle_deg = 5.0;
    render(&core, &rotated);
    let g1 = core
        .develop_state()
        .unwrap()
        .unwrap()
        .before
        .unwrap()
        .generation;
    assert!(g1 > g0);
    wait_before(&core, g1);
    // 同じ出どころを設定し直しても、描いたものを使う（描き直さない）。
    let again = core
        .set_before(Some(BeforeSource::Initial))
        .unwrap()
        .unwrap();
    assert_eq!(again.generation, g1);

    // 今の設定の写し: ビフォーとアフターが同じ画素。
    let cur = core
        .set_before(Some(BeforeSource::Current))
        .unwrap()
        .unwrap();
    assert_eq!(cur.label, "アフターの設定のコピー");
    wait_before(&core, cur.generation);
    assert_eq!(
        core.before_frame().unwrap().display_rgb8,
        core.develop_frame().unwrap().display_rgb8
    );

    // この写真の履歴にない項目は not_found。
    assert!(matches!(
        core.set_before(Some(BeforeSource::History {
            entry_id: HistoryEntryId::new(99_999)
        })),
        Err(ApiError::NotFound(_))
    ));
    // 履歴の項目（最初の項目 = 読み込み）。
    let first = core.history(v).unwrap()[0].id;
    let h = core
        .set_before(Some(BeforeSource::History { entry_id: first }))
        .unwrap()
        .unwrap();
    assert_eq!(h.label, "履歴: 読み込み");

    // やめる → なし。開き直すとやめた状態。
    assert_eq!(core.set_before(None).unwrap(), None);
    assert!(core.before_frame().is_none());
    core.set_before(Some(BeforeSource::Initial)).unwrap();
    core.close_develop().unwrap();
    core.open_develop(v).unwrap();
    assert_eq!(core.develop_state().unwrap().unwrap().before, None);
    assert!(core.before_frame().is_none());
    core.close().unwrap();
}

/// 仮想コピーの既定のビフォーは、仮想コピーを作ったときの設定（履歴の最初の項目）。
#[test]
fn before_of_a_virtual_copy_is_its_creation_state() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (96, 64), 2, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    core.set_settings(&edited(), true).unwrap();
    core.end_drag().unwrap();
    core.close_develop().unwrap();
    let copy = core.create_virtual_copy(v, None).unwrap();
    core.open_develop(copy).unwrap();
    let mut changed = edited();
    changed.exposure_ev = -2.0;
    render(&core, &changed);
    let info = core
        .set_before(Some(BeforeSource::Initial))
        .unwrap()
        .unwrap();
    assert_eq!(info.label, "仮想コピーの作成");
    wait_before(&core, info.generation);
    let before_px = core.before_frame().unwrap().display_rgb8;
    // 作ったときの設定（マスターの設定の写し）のアフターと同じ。
    render(&core, &edited());
    assert_eq!(core.develop_frame().unwrap().display_rgb8, before_px);
    core.close().unwrap();
}

/// ビフォーの表示範囲: アフターと同じ範囲の要求で同じ寸法・違う画素。互いに取り消さない。ビフォーを設定して
/// いなければ invalid_argument。JSON のコマンドでも同じ。
#[test]
fn before_region_matches_the_after_region_grid() {
    let env = Env::new();
    write_jpeg(&env.photos.join("A.jpg"), (160, 120), 3, None);
    let core = env.open();
    import(&core, &env.photos);
    let v = all_items(&core)[0].variant_id;
    core.open_develop(v).unwrap();
    render(&core, &edited());
    let req = RegionRequest {
        variant_id: v,
        space: RegionSpace::Output,
        x: 10.0,
        y: 5.0,
        width: 40.0,
        height: 30.0,
        scale: 1.0,
    };
    assert!(matches!(
        core.render_before_region(&req),
        Err(ApiError::InvalidArgument(_))
    ));
    core.set_before(Some(BeforeSource::Initial)).unwrap();
    let a = core.render_region(&req).unwrap();
    let b = core.render_before_region(&req).unwrap();
    assert!(!a.info.before && b.info.before);
    assert_eq!(
        (
            b.info.x,
            b.info.y,
            b.info.width,
            b.info.height,
            b.info.full_width,
            b.info.full_height
        ),
        (
            a.info.x,
            a.info.y,
            a.info.width,
            a.info.height,
            a.info.full_width,
            a.info.full_height
        )
    );
    assert_ne!(a.display_rgb8, b.display_rgb8);
    // それぞれの最後の結果が残る（互いに置き換えない）。
    assert!(!core.region_frame().unwrap().info.before);
    assert!(core.before_region_frame().unwrap().info.before);
    // JSON のコマンド。
    let r = core
        .execute(
            serde_json::from_value(serde_json::json!({
                "type": "set_before", "source": { "kind": "initial" }
            }))
            .unwrap(),
        )
        .unwrap();
    let json = serde_json::to_value(&r).unwrap();
    assert_eq!(json["type"], "before");
    assert_eq!(json["before"]["source"]["kind"], "initial");
    let r = core
        .execute(Command::RenderBeforeRegion { request: req })
        .unwrap();
    assert!(matches!(r, Response::Region(info) if info.before));
    let r = core.execute(Command::RenderBefore).unwrap();
    assert!(matches!(r, Response::Frame(_)));
    let off = core
        .execute(
            serde_json::from_value(serde_json::json!({ "type": "set_before", "source": null }))
                .unwrap(),
        )
        .unwrap();
    assert_eq!(off, Response::Before { before: None });
    core.close().unwrap();
}
