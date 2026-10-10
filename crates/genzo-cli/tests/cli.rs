//! `genzo` の結合テスト（実際の実行ファイルを `env!("CARGO_BIN_EXE_genzo")` で動かす。ORG-05・MAINT-01、
//! 05 の 2.4 節「回帰テストと CLI が CI で動く」）。
//!
//! - 合成の画像（JPEG・PNG・TIFF）、ffmpeg があれば動画、LibRaw が有効な build なら合成 DNG のフォルダで、
//!   init → import → search → rate → search --min-rating → develop set → export（ICC と GPS の有無）→
//!   render → backup → check を行い、`--json` の出力を解析して確かめる。
//! - 元ファイルのハッシュが操作の前後で変わらないこと（DATA-01）。
//! - 不正な引数で終了コード 2、エラーで 1。
//! - 結果は出力の内容と終了コードで決め、時間では決めない。CI（環境変数 `CI`）では GPU を使わない
//!   （genzo-api のテストと同じ方針）。ffmpeg・LibRaw がなければ、それを使う部分を飛ばす。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use genzo_media::{ExifData, FfmpegTools, RgbImage8};
use genzo_model::{GpsCoord, OutputColorSpace};
use serde_json::Value;

const GENZO: &str = env!("CARGO_BIN_EXE_genzo");

/// CI のランナーか（環境変数 `CI` が空でなく、`0`・`false` でない）。
fn on_ci() -> bool {
    std::env::var("CI").is_ok_and(|v| {
        let v = v.trim().to_ascii_lowercase();
        !v.is_empty() && v != "0" && v != "false"
    })
}

/// 実行の結果。
struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "JSON ではない（{e}）\nstdout: {}\nstderr: {}",
                self.stdout, self.stderr
            )
        })
    }
}

/// `genzo` を実行する（環境変数のカタログ・データのフォルダ・ログの指定は引き継がない）。
fn genzo_with(args: &[&str], env: &[(&str, &Path)], stdin: Option<&str>) -> Run {
    let mut cmd = Command::new(GENZO);
    cmd.args(args)
        .env_remove("GENZO_CATALOG")
        .env_remove("GENZO_DATA_DIR")
        .env_remove("GENZO_LOG")
        .env_remove("GENZO_BENCH_DIR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("genzo を起動できる");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// テストの環境（一時フォルダ・写真のフォルダ・カタログ）。
struct Env {
    dir: tempfile::TempDir,
    photos: PathBuf,
    catalog: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        let catalog = dir.path().join("cat").join("catalog.db");
        Self {
            dir,
            photos,
            catalog,
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    /// カタログを指定して実行する（GPU は使わない。進捗は出さない）。
    fn run(&self, args: &[&str]) -> Run {
        self.run_stdin(args, None)
    }

    fn run_stdin(&self, args: &[&str], stdin: Option<&str>) -> Run {
        let catalog = self.catalog.to_string_lossy().into_owned();
        let mut all: Vec<&str> = args.to_vec();
        all.extend(["--catalog", &catalog, "--gpu", "off", "--quiet"]);
        genzo_with(&all, &[], stdin)
    }

    /// `--json` を付けて実行し、成功を確かめて JSON を返す。
    fn json(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let r = self.run(&all);
        assert_eq!(
            r.code, 0,
            "{args:?} が失敗した\nstdout: {}\nstderr: {}",
            r.stdout, r.stderr
        );
        r.json()
    }
}

/// フォルダの中のすべてのファイルのハッシュ（DATA-01 の確認用）。
fn hashes(dir: &Path) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            let t = e.file_type().unwrap();
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                out.insert(
                    e.path(),
                    genzo_testkit::record::sha256_file_hex(e.path()).unwrap(),
                );
            }
        }
    }
    out
}

/// 合成の JPEG（Exif の撮影日時と GPS 付き）を書く。
fn write_jpeg(path: &Path, (w, h): (u32, u32), seed: u8, datetime: Option<&str>, gps: bool) {
    let px: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [
                (x * 255 / w) as u8,
                (y * 255 / h) as u8,
                seed.wrapping_mul(37).wrapping_add(40),
            ]
        })
        .collect();
    let rgb = RgbImage8::from_raw(w, h, px).unwrap();
    let exif = ExifData {
        make: Some("GenzoTest".to_owned()),
        model: Some("Synthetic".to_owned()),
        datetime_original: datetime
            .map(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()),
        offset_time_original: datetime.map(|_| chrono::FixedOffset::east_opt(9 * 3600).unwrap()),
        gps: gps.then(|| GpsCoord::new(35.0116, 135.7681).unwrap()),
        ..Default::default()
    };
    let exif_bytes = genzo_media::exif_tiff_bytes(&exif, w, h, OutputColorSpace::Srgb).unwrap();
    let bytes = genzo_media::jpeg::encode_jpeg(&rgb, 92, None, Some(&exif_bytes)).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// 合成の PNG / TIFF（撮影日時なし）を書く。
fn write_plain(path: &Path, (w, h): (u32, u32)) {
    image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 4) as u8, (y * 4) as u8, 90]))
        .save(path)
        .unwrap();
}

/// ffmpeg があれば短い動画を作る。
fn make_video(path: &Path) -> bool {
    let t = FfmpegTools::from_env();
    if !t.is_available() {
        eprintln!("ffmpeg / ffprobe がないため、動画の部分を飛ばす");
        return false;
    }
    let ok = Command::new(&t.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-y"])
        .args([
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=96x64:rate=10",
            "-pix_fmt",
            "yuv420p",
            "-c:v",
            "mpeg4",
            "-metadata",
            "creation_time=2024-05-02T01:00:00Z",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
        && path.exists();
    if !ok {
        eprintln!("テスト用の動画を作れないため、動画の部分を飛ばす");
    }
    ok
}

/// LibRaw が有効な build なら、合成 DNG（埋め込みの JPEG のプレビュー付き）を書く。
fn make_dng(path: &Path) -> bool {
    if !genzo_raw::LIBRAW_ENABLED {
        eprintln!("LibRaw が無効な build のため、DNG の部分を飛ばす");
        return false;
    }
    use genzo_testkit::bayer::{BayerParams, mosaic, working_to_camera};
    use genzo_testkit::image::FloatImage;
    const CAM_XYZ: [[f32; 3]; 3] = [
        [0.7424, -0.2329, -0.0466],
        [-0.4598, 1.2471, 0.2347],
        [-0.0715, 0.1505, 0.6066],
    ];
    let (w, h) = (96u32, 64u32);
    let scene = FloatImage::from_rgb_fn(w, h, |x, y| {
        [
            0.05 + 0.6 * x as f32 / w as f32,
            0.05 + 0.4 * y as f32 / h as f32,
            0.2,
        ]
    })
    .unwrap();
    let camera = working_to_camera(&scene, &CAM_XYZ).unwrap();
    let mut params = BayerParams {
        cam_xyz: Some(CAM_XYZ),
        black_level: [512.0; 4],
        ..BayerParams::default()
    };
    params.metadata.capture.datetime = Some("2024:05:10 08:00:00".to_owned());
    params.metadata.capture.offset = Some("+09:00".to_owned());
    let raw = mosaic(&camera, &params).unwrap();
    let preview_px: Vec<u8> = (0..w * h)
        .flat_map(|i| [(i % 256) as u8, 80, 120])
        .collect();
    let jpeg = genzo_media::jpeg::encode_jpeg(
        &RgbImage8::from_raw(w, h, preview_px).unwrap(),
        90,
        None,
        None,
    )
    .unwrap();
    let options = genzo_raw::dng::DngOptions {
        preview: Some(genzo_raw::dng::DngPreview::Jpeg {
            width: w,
            height: h,
            data: jpeg,
        }),
        ..Default::default()
    };
    genzo_raw::dng::write_file(path, &raw, &options).unwrap();
    true
}

/// 検索の結果の項目。
fn items(v: &Value) -> &Vec<Value> {
    v["items"].as_array().expect("items")
}

/// 名前の項目（マスター）の variant の ID。
fn id_of(search: &Value, name: &str) -> i64 {
    items(search)
        .iter()
        .find(|i| i["file_name"] == name && i["is_master"] == true)
        .unwrap_or_else(|| panic!("{name} がない: {search}"))["variant_id"]
        .as_i64()
        .unwrap()
}

fn names(search: &Value) -> Vec<String> {
    let mut v: Vec<String> = items(search)
        .iter()
        .map(|i| i["file_name"].as_str().unwrap().to_owned())
        .collect();
    v.sort();
    v
}

/// JPEG に ICC プロファイル（APP2 の ICC_PROFILE）があるか。
fn jpeg_has_icc(bytes: &[u8]) -> bool {
    bytes.windows(12).any(|w| w == b"ICC_PROFILE\0")
}

/// Exif に GPS の緯度があるか。
fn has_gps(path: &Path) -> bool {
    let file = std::fs::File::open(path).unwrap();
    let Ok(exif) = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file))
    else {
        return false;
    };
    exif.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY)
        .is_some()
}

/// 書き出しの結果の 1 件目のパス。
fn written_path(export: &Value) -> PathBuf {
    let item = &export["report"]["items"][0]["outcome"];
    assert_eq!(item["status"], "written", "{export}");
    PathBuf::from(item["path"].as_str().unwrap())
}

/// 主な流れ: init → import → search → rate → search --min-rating → develop → export → render →
/// backup → check（ORG-05。05 の 2.4 節の M2 / M3 の完了条件）。
#[test]
fn end_to_end_catalog_workflow() {
    let env = Env::new();
    let p = &env.photos;
    write_jpeg(
        &p.join("A.jpg"),
        (96, 64),
        1,
        Some("2024-05-01 10:00:00"),
        true,
    );
    write_jpeg(
        &p.join("B.jpg"),
        (80, 60),
        2,
        Some("2024-05-03 09:30:00"),
        false,
    );
    std::fs::create_dir_all(p.join("sub")).unwrap();
    write_plain(&p.join("sub").join("C.png"), (64, 48));
    write_plain(&p.join("sub").join("D.tif"), (40, 30));
    let video = make_video(&p.join("sub").join("V.mp4"));
    let dng = make_dng(&p.join("R.dng"));
    let expected = 4 + usize::from(video) + usize::from(dng);
    let before = hashes(p);

    // カタログの作成（2 回目はエラー）。
    let init = env.json(&["catalog", "init"]);
    assert_eq!(init["catalog"], env.catalog.to_string_lossy().as_ref());
    assert!(init["schema_version"].as_u64().unwrap() >= 1);
    assert!(env.catalog.is_file());
    assert!(
        env.path("cat/catalog-data").is_dir(),
        "既定のデータのフォルダ"
    );
    let again = env.run(&["catalog", "init"]);
    assert_eq!(again.code, 1, "{}", again.stderr);

    // 取り込み（2 回目は件数が増えない）。
    let report = env.json(&["import", p.to_str().unwrap()]);
    assert_eq!(report["files_found"], expected as u64, "{report}");
    assert_eq!(report["added"], expected as u64, "{report}");
    assert_eq!(report["errors"].as_array().unwrap().len(), 0, "{report}");
    let report = env.json(&["import", p.to_str().unwrap()]);
    assert_eq!(report["added"], 0);
    assert_eq!(report["unchanged"], expected as u64);

    // 検索。
    let all = env.json(&["search", "--limit", "0"]);
    assert_eq!(all["count"], expected as u64);
    assert_eq!(items(&all).len(), expected);
    let a = id_of(&all, "A.jpg");
    let b = id_of(&all, "B.jpg");
    let c = id_of(&all, "C.png");
    let d = id_of(&all, "D.tif");
    // 撮影日時順: A（5/1）→ B（5/3）、日時のないものは最後。
    let order: Vec<i64> = items(&all)
        .iter()
        .map(|i| i["variant_id"].as_i64().unwrap())
        .collect();
    let pos = |v: i64| order.iter().position(|&x| x == v).unwrap();
    assert!(pos(a) < pos(b), "{all}");
    assert!(pos(b) < pos(c) && pos(b) < pos(d), "{all}");
    // 件数と飛ばす件数。
    let page = env.json(&["search", "--limit", "1", "--offset", "1"]);
    assert_eq!(items(&page).len(), 1);
    assert_eq!(page["items"][0]["variant_id"], order[1]);
    // 表（人が読む形）と ID だけの出力。
    let table = env.run(&["search"]);
    assert_eq!(table.code, 0);
    assert!(table.stdout.contains("A.jpg") && table.stdout.contains("撮影日時"));
    let ids = env.run(&["search", "--ids", "--sort", "name"]);
    assert_eq!(ids.stdout.lines().count(), expected);

    // 評価 → 評価の下限で検索。
    assert_eq!(
        env.json(&["rate", &a.to_string(), &b.to_string(), "4"])["updated"],
        2
    );
    assert_eq!(env.json(&["rate", &c.to_string(), "1"])["updated"], 1);
    let rated = env.json(&["search", "--min-rating", "3"]);
    assert_eq!(names(&rated), vec!["A.jpg", "B.jpg"]);
    // 知らない variant を含む指定は、何も変えずにエラー。
    let r = env.run(&["rate", &a.to_string(), "99999", "2", "--json"]);
    assert_eq!(r.code, 1);
    assert_eq!(r.json()["error"]["kind"], "not_found");
    assert_eq!(names(&env.json(&["search", "--min-rating", "4"])).len(), 2);

    // フラグ・ラベル・キャプション・テキスト・期間・種別・フォルダ。期間の日付はカタログの既定の
    // タイムゾーン（OS のオフセット）で解釈されるので、どのタイムゾーンでも結果が変わらない日時の
    // 写真だけで確かめる（A: 5/1 01:00Z、B: 5/3 00:30Z、DNG: 5/9 23:00Z。動画は --kind photo で除く）。
    env.json(&["flag", &a.to_string(), "picked"]);
    env.json(&["flag", &d.to_string(), "rejected"]);
    env.json(&["label", &b.to_string(), "red"]);
    env.json(&["caption", &c.to_string(), "京都の夕焼け"]);
    assert_eq!(
        names(&env.json(&["search", "--flag", "picked"])),
        vec!["A.jpg"]
    );
    assert_eq!(
        names(&env.json(&["search", "--flag", "picked", "--flag", "rejected"])),
        vec!["A.jpg", "D.tif"]
    );
    assert_eq!(
        names(&env.json(&["search", "--label", "red"])),
        vec!["B.jpg"]
    );
    assert_eq!(
        names(&env.json(&["search", "--text", "夕焼け"])),
        vec!["C.png"]
    );
    assert_eq!(
        names(&env.json(&[
            "search",
            "--from",
            "2024-05-02",
            "--to",
            "2024-05-03",
            "--kind",
            "photo"
        ])),
        vec!["B.jpg"]
    );
    assert_eq!(
        names(&env.json(&["search", "--to", "2024-05-01", "--kind", "photo"])),
        vec!["A.jpg"],
        "上限の日付はその日を含む"
    );
    let sub = p.join("sub");
    let in_sub = env.json(&["search", "--folder", sub.to_str().unwrap()]);
    assert_eq!(in_sub["count"], 2 + u64::from(video));
    let top_only = env.json(&["search", "--folder", p.to_str().unwrap(), "--no-subfolders"]);
    assert_eq!(top_only["count"], 2 + u64::from(dng));
    if video {
        assert_eq!(
            names(&env.json(&["search", "--kind", "video"])),
            vec!["V.mp4"]
        );
        // 動画の長さ・fps・コーデック・寸法（VID-03。F40。テスト用の動画は mpeg4・1 秒・10 fps・96×64）。
        assert_eq!(
            names(&env.json(&[
                "search",
                "--codec",
                "MPEG4",
                "--max-duration",
                "5",
                "--min-fps",
                "9.5",
            ])),
            vec!["V.mp4"]
        );
        assert_eq!(env.json(&["search", "--min-duration", "5"])["count"], 0);
        assert_eq!(
            names(&env.json(&["search", "--kind", "video", "--min-long-edge", "96",])),
            vec!["V.mp4"]
        );
        let bad = env.run(&["search", "--max-duration", "-1"]);
        assert_eq!(bad.code, 2, "{}", bad.stderr);
        // 動画の表示は項目ごと（Rust の Debug の形にしない）。現像設定は出さない。
        let v = id_of(&all, "V.mp4");
        let shown = env.run(&["show", &v.to_string()]);
        assert_eq!(shown.code, 0, "{}", shown.stderr);
        assert!(shown.stdout.contains("コーデック"), "{}", shown.stdout);
        assert!(!shown.stdout.contains("VideoMetadata"), "{}", shown.stdout);
        assert!(!shown.stdout.contains("現像設定"), "{}", shown.stdout);
    }
    let by_rating = env.json(&["search", "--sort", "rating", "--desc", "--limit", "2"]);
    assert_eq!(by_rating["items"][0]["rating"], 4);
    // カメラ（メーカーと機種。完全一致）とレンズ。
    assert_eq!(
        names(&env.json(&["search", "--camera", "GenzoTest Synthetic"])),
        vec!["A.jpg", "B.jpg"]
    );
    assert_eq!(env.json(&["search", "--lens", "nothing"])["count"], 0);

    // 表示。
    let shown = env.json(&["show", &a.to_string()]);
    assert_eq!(shown["details"]["summary"]["rating"], 4);
    assert_eq!(shown["details"]["summary"]["flag"], 1);
    assert!(shown["details"]["gps"]["lat"].as_f64().is_some());
    assert_eq!(shown["develop"]["exposure_ev"], 0.0);
    let shown_c = env.json(&["show", &c.to_string()]);
    assert_eq!(shown_c["details"]["caption"], "京都の夕焼け");

    // 現像設定: ファイルから（部分的な指定をマージ）→ 標準入力から → 履歴 → Undo / Redo。
    let settings_file = env.path("exposure.json");
    std::fs::write(&settings_file, r#"{"exposure_ev": 0.5, "contrast": 10}"#).unwrap();
    let set = env.json(&[
        "develop",
        "set",
        &a.to_string(),
        "--json",
        settings_file.to_str().unwrap(),
    ]);
    assert_eq!(set["changed"], true);
    assert_eq!(set["settings"]["exposure_ev"], 0.5);
    let r = env.run_stdin(
        &["develop", "set", &a.to_string(), "--json", "-", "--json"],
        Some(r#"{"tone": {"shadows": 20}}"#),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let got = env.json(&["develop", "get", &a.to_string()]);
    assert_eq!(got["exposure_ev"], 0.5, "マージで前の変更が残る");
    assert_eq!(got["contrast"], 10.0);
    assert_eq!(got["tone"]["shadows"], 20.0);
    let history = env.json(&["develop", "history", &a.to_string()]);
    let labels: Vec<&str> = history
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        vec!["読み込み", "露光量 +0.50・コントラスト +10", "シャドウ +20"]
    );
    let undo = env.json(&["develop", "undo", &a.to_string()]);
    assert_eq!(undo["settings"]["tone"]["shadows"], 0.0);
    let redo = env.json(&["develop", "redo", &a.to_string()]);
    assert_eq!(redo["settings"]["tone"]["shadows"], 20.0);
    let none = env.json(&["develop", "redo", &a.to_string()]);
    assert_eq!(none["changed"], false);
    // 同じ値なら履歴に記録しない。
    let same = env.json(&[
        "develop",
        "set",
        &a.to_string(),
        "--json",
        settings_file.to_str().unwrap(),
    ]);
    assert_eq!(same["changed"], false);
    // 知らない項目・範囲外の値はエラー（設定は変わらない）。
    let bad = env.path("bad.json");
    std::fs::write(&bad, r#"{"tone": {"shadowz": 3}}"#).unwrap();
    let r = env.run(&[
        "develop",
        "set",
        &a.to_string(),
        "--json",
        bad.to_str().unwrap(),
    ]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("tone.shadowz"), "{}", r.stderr);
    std::fs::write(&bad, r#"{"exposure_ev": 50}"#).unwrap();
    assert_eq!(
        env.run(&[
            "develop",
            "set",
            &a.to_string(),
            "--json",
            bad.to_str().unwrap()
        ])
        .code,
        1
    );
    assert_eq!(
        env.json(&["develop", "get", &a.to_string()])["tone"]["shadows"],
        20.0
    );

    // コピー（DEV-30）・仮想コピー・初期化。
    let copied = env.json(&["develop", "copy", &a.to_string(), "--to", &b.to_string()]);
    assert_eq!(copied["updated"], 1);
    assert_eq!(
        env.json(&["develop", "get", &b.to_string()])["exposure_ev"],
        0.5
    );
    let partial = env.json(&[
        "develop",
        "copy",
        &a.to_string(),
        "--to",
        &d.to_string(),
        "--groups",
        "color",
    ]);
    assert_eq!(partial["updated"], 1);
    assert_eq!(
        env.json(&["develop", "get", &d.to_string()])["exposure_ev"],
        0.0,
        "色だけを写す"
    );
    let vc = env.json(&["develop", "virtual-copy", &a.to_string(), "--name", "白黒"]);
    let vc_id = vc["variant_id"].as_i64().unwrap();
    assert_eq!(env.json(&["search"])["count"], expected as u64 + 1);
    assert_eq!(
        env.json(&["develop", "get", &vc_id.to_string()])["exposure_ev"],
        0.5
    );
    let r = env.run(&["develop", "delete-copy", &a.to_string(), "--json"]);
    assert_eq!(r.code, 1, "マスターは仮想コピーとして削除できない");
    env.json(&["develop", "delete-copy", &vc_id.to_string()]);
    assert_eq!(env.json(&["search"])["count"], expected as u64);
    let reset = env.json(&["develop", "reset", &b.to_string()]);
    assert_eq!(reset["settings"]["exposure_ev"], 0.0);

    // 書き出し: JPEG（GPS を除く）と、GPS を残した JPEG、P3 の PNG 16bit（長辺 32）。
    let out_dir = env.path("out");
    let e1 = env.json(&[
        "export",
        &a.to_string(),
        "--out",
        out_dir.to_str().unwrap(),
        "--remove-gps",
        "--quality",
        "85",
    ]);
    assert_eq!(e1["report"]["written"], 1, "{e1}");
    let jpg = written_path(&e1);
    assert_eq!(jpg.file_name().unwrap(), "A.jpg");
    let bytes = std::fs::read(&jpg).unwrap();
    assert!(jpeg_has_icc(&bytes), "ICC プロファイルを埋め込む（IQ-06）");
    assert!(!has_gps(&jpg), "GPS を除く（SEC-03）");
    assert_eq!(image::load_from_memory(&bytes).unwrap().width(), 96);
    let e2 = env.json(&["export", &a.to_string(), "--out", out_dir.to_str().unwrap()]);
    let jpg2 = written_path(&e2);
    assert_ne!(jpg2, jpg, "連番を付ける");
    assert!(has_gps(&jpg2), "既定では GPS を残す");
    let e3 = env.json(&[
        "export",
        &a.to_string(),
        "--out",
        out_dir.to_str().unwrap(),
        "--format",
        "png16",
        "--color-space",
        "p3",
        "--long-edge",
        "32",
    ]);
    let png = written_path(&e3);
    let png_bytes = std::fs::read(&png).unwrap();
    assert!(png_bytes.windows(4).any(|w| w == b"iCCP"), "PNG に ICC");
    let decoded = image::load_from_memory(&png_bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (32, 21));
    assert_eq!(decoded.color(), image::ColorType::Rgb16);
    // スキップの設定では既存のファイルを置き換えない。
    let e4 = env.json(&[
        "export",
        &a.to_string(),
        "--out",
        out_dir.to_str().unwrap(),
        "--on-conflict",
        "skip",
    ]);
    assert_eq!(e4["report"]["skipped"], 1);
    // 元ファイルのあるフォルダへの上書きは拒否する（6.4 節）。一部の失敗は終了コード 1。
    let r = env.run(&[
        "export",
        &a.to_string(),
        "--out",
        p.to_str().unwrap(),
        "--format",
        "jpeg",
        "--on-conflict",
        "overwrite",
        "--json",
    ]);
    assert_eq!(r.code, 1, "{}", r.stdout);
    assert_eq!(r.json()["report"]["failed"], 1);
    if video {
        let v = id_of(&all, "V.mp4");
        let r = env.json(&["export", &v.to_string(), "--out", out_dir.to_str().unwrap()]);
        assert_eq!(
            r["report"]["skipped"], 1,
            "動画は書き出さない（EXP-05 は v1）"
        );
    }
    if dng {
        let rid = id_of(&all, "R.dng");
        let r = env.json(&[
            "export",
            &rid.to_string(),
            "--out",
            out_dir.to_str().unwrap(),
            "--format",
            "tiff",
        ]);
        let tif = written_path(&r);
        assert_eq!(tif.extension().unwrap(), "tif");
        let img = image::open(&tif).unwrap();
        assert_eq!((img.width(), img.height()), (96, 64));
    }

    // カタログなしの現像（ワーカーで展開）。
    let rendered = env.path("render/B-small.jpg");
    std::fs::write(env.path("render-settings.json"), r#"{"exposure_ev": -0.5}"#).unwrap();
    let r = genzo_with(
        &[
            "render",
            p.join("B.jpg").to_str().unwrap(),
            "--out",
            rendered.to_str().unwrap(),
            "--settings",
            env.path("render-settings.json").to_str().unwrap(),
            "--long-edge",
            "40",
            "--gpu",
            "off",
            "--json",
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let v = r.json();
    assert_eq!(v["output"], rendered.to_string_lossy().as_ref());
    assert_eq!(
        (v["width"].as_u64(), v["height"].as_u64()),
        (Some(40), Some(30))
    );
    assert_eq!(v["settings"]["exposure_ev"], -0.5);
    assert_eq!(v["backend"], "cpu");
    assert!(jpeg_has_icc(&std::fs::read(&rendered).unwrap()));

    // サムネイルの作り直し・ファイルの確認・情報。
    let thumbs = env.json(&["thumbs", "regenerate", &a.to_string(), &c.to_string()]);
    assert_eq!(thumbs["rendered"], 2, "{thumbs}");
    let info = env.json(&["catalog", "info"]);
    assert_eq!(info["counts"]["variants"], expected as u64);
    assert_eq!(info["counts"]["assets"], expected as u64);
    assert_eq!(info["previous_shutdown"], "clean");

    // バックアップ → 詳細チェック → 復元（別のパスへ）。
    let backup = env.json(&[
        "catalog",
        "backup",
        "--to",
        env.path("bk").to_str().unwrap(),
    ]);
    let backup_path = PathBuf::from(backup["path"].as_str().unwrap());
    assert!(backup_path.is_file());
    assert!(backup_path.starts_with(env.path("bk")));
    let check = env.json(&["catalog", "check", "--files"]);
    assert_eq!(check["ok"], true, "{check}");
    assert_eq!(check["integrity"]["ok"], true);
    assert_eq!(check["files"]["missing"].as_array().unwrap().len(), 0);
    let restored = env.path("restored/catalog.db");
    std::fs::create_dir_all(restored.parent().unwrap()).unwrap();
    let rs = env.json(&[
        "catalog",
        "restore",
        backup_path.to_str().unwrap(),
        "--to",
        restored.to_str().unwrap(),
    ]);
    assert_eq!(rs["displaced"], Value::Null);
    let r = genzo_with(
        &[
            "catalog",
            "info",
            "--catalog",
            restored.to_str().unwrap(),
            "--gpu",
            "off",
            "--json",
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["counts"]["variants"], expected as u64);

    // 除去（元ファイルは残る）と、確認なしのゴミ箱への移動の中止。
    let removed = env.json(&["remove", &d.to_string()]);
    assert_eq!(
        removed["report"]["removed_assets"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(removed["executed"], true, "中止したときと同じ形");
    assert_eq!(env.json(&["search"])["count"], expected as u64 - 1);
    assert!(p.join("sub").join("D.tif").is_file());
    let r = env.run(&["trash", &c.to_string()]);
    assert_eq!(r.code, 1, "--yes がなければ中止");
    assert!(
        r.stdout.contains("C.png"),
        "影響の一覧を表示する: {}",
        r.stdout
    );
    assert!(r.stderr.contains("--yes"), "{}", r.stderr);
    let r = env.run(&["trash", &c.to_string(), "--json"]);
    assert_eq!(r.code, 1);
    assert_eq!(r.json()["executed"], false);
    assert!(p.join("sub").join("C.png").is_file());
    assert_eq!(env.json(&["search"])["count"], expected as u64 - 1);

    // 元ファイルは変わらない（DATA-01）。
    assert_eq!(hashes(p), before, "元ファイルのハッシュが変わった");
}

/// 使い方の誤りは終了コード 2、--help と --version は 0。
#[test]
fn usage_errors_exit_with_2() {
    let env = Env::new();
    let none = genzo_with(&[], &[], None);
    assert_eq!(none.code, 2);
    for args in [
        &["rate", "1", "9"][..],
        &["rate", "1"],
        &["flag", "1", "maybe"],
        &["nosuchcommand"],
        &["search", "--from", "2024/05/01"],
        &["search", "--limit", "-1"],
        &["export", "1"],
        &["bench", "preview", "--synthetic", "8x8"],
        &["develop", "set", "1"],
        &["show", "abc"],
    ] {
        let r = env.run(args);
        assert_eq!(r.code, 2, "{args:?}: {}", r.stderr);
    }
    // カタログの指定がない。
    let r = genzo_with(&["search"], &[], None);
    assert_eq!(r.code, 2, "{}", r.stderr);
    assert!(r.stderr.contains("GENZO_CATALOG"));
    let r = genzo_with(&["search", "--json"], &[], None);
    assert_eq!(r.code, 2);
    assert_eq!(r.json()["error"]["kind"], "usage");
    // --quality は JPEG のときだけ。
    env.json(&["catalog", "init"]);
    let r = env.run(&[
        "export",
        "1",
        "--out",
        "o",
        "--format",
        "png8",
        "--quality",
        "50",
    ]);
    assert_eq!(r.code, 2, "{}", r.stderr);
    let help = genzo_with(&["--help"], &[], None);
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("catalog") && help.stdout.contains("bench"));
    assert!(
        !help.stdout.contains("__worker"),
        "隠しサブコマンドは表示しない"
    );
    let version = genzo_with(&["--version"], &[], None);
    assert_eq!(version.code, 0);
    assert!(version.stdout.starts_with("genzo "));
}

/// エラーは終了コード 1。`--json` では標準出力に `{"error": ...}`。
#[test]
fn errors_exit_with_1() {
    let env = Env::new();
    // カタログがない（作らない）。
    let r = env.run(&["search", "--json"]);
    assert_eq!(r.code, 1);
    assert_eq!(r.json()["error"]["kind"], "not_found");
    assert!(
        !env.catalog.exists(),
        "打ち間違えたパスにカタログを作らない"
    );
    env.json(&["catalog", "init"]);
    let r = env.run(&["show", "12345", "--json"]);
    assert_eq!(r.code, 1);
    let e = r.json();
    assert_eq!(e["error"]["kind"], "not_found");
    assert!(e["error"]["message"].as_str().unwrap().contains("12345"));
    let r = env.run(&["show", "12345"]);
    assert_eq!(r.code, 1);
    assert!(r.stdout.is_empty());
    assert!(r.stderr.starts_with("エラー:"), "{}", r.stderr);
    let r = env.run(&["import", env.path("nowhere").to_str().unwrap()]);
    assert_eq!(r.code, 1);
    let r = env.run(&["info", env.path("missing.jpg").to_str().unwrap()]);
    assert_eq!(r.code, 1);
    let r = env.run(&["info", env.path("notes.txt").to_str().unwrap(), "--json"]);
    assert_eq!(r.code, 1);
    assert_eq!(r.json()["error"]["kind"], "invalid_input");
}

/// 環境変数のカタログとデータのフォルダ、`info`（ワーカーでメタデータを読む）。
#[test]
fn environment_variables_and_info() {
    let env = Env::new();
    let data = env.path("my-data");
    let r = genzo_with(
        &["catalog", "init", "--json"],
        &[("GENZO_CATALOG", &env.catalog), ("GENZO_DATA_DIR", &data)],
        None,
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["data_dir"], data.to_string_lossy().as_ref());
    assert!(data.join("thumbs.db").is_file());
    // --catalog が環境変数より優先される。
    let other = env.path("other.db");
    let r = genzo_with(
        &[
            "catalog",
            "init",
            "--catalog",
            other.to_str().unwrap(),
            "--json",
        ],
        &[("GENZO_CATALOG", &env.catalog)],
        None,
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(other.is_file());
    assert!(env.path("other-data").is_dir());
    // --data-dir で指定する。
    let third = env.path("third.db");
    let third_data = env.path("third-store");
    let r = genzo_with(
        &[
            "catalog",
            "init",
            "--catalog",
            third.to_str().unwrap(),
            "--data-dir",
            third_data.to_str().unwrap(),
            "--json",
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(third_data.join("thumbs.db").is_file());
    assert!(!env.path("third-data").exists());

    let jpg = env.photos.join("I.jpg");
    write_jpeg(&jpg, (48, 32), 5, Some("2024-06-01 12:00:00"), true);
    let before = hashes(&env.photos);
    let info = env.json(&["info", jpg.to_str().unwrap()]);
    assert_eq!(info["kind"], "photo");
    assert_eq!(info["probe"]["format"], "jpeg");
    assert_eq!(info["probe"]["metadata"]["width"], 48);
    assert_eq!(info["probe"]["metadata"]["make"], "GenzoTest");
    assert!(info["probe"]["metadata"]["gps"]["lat"].as_f64().is_some());
    let text = env.run(&["info", jpg.to_str().unwrap()]);
    assert_eq!(text.code, 0);
    assert!(text.stdout.contains("GenzoTest"), "{}", text.stdout);
    assert_eq!(hashes(&env.photos), before);
}

/// `render` は入力のファイルへ書き出さない（6.4 節）。GPU を既定の設定（auto）にしても書き出せる。
#[test]
fn render_protects_the_input_and_uses_the_default_gpu_setting() {
    let env = Env::new();
    let src = env.photos.join("S.png");
    write_plain(&src, (60, 40));
    let before = hashes(&env.photos);
    let r = genzo_with(
        &[
            "render",
            src.to_str().unwrap(),
            "--out",
            src.to_str().unwrap(),
            "--on-conflict",
            "overwrite",
            "--json",
            "--gpu",
            "off",
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 1, "{}", r.stdout);
    assert_eq!(r.json()["error"]["kind"], "protected_destination");
    assert_eq!(hashes(&env.photos), before, "入力は置き換えない");
    // --format がなく、拡張子からも決められない。
    let r = genzo_with(
        &[
            "render",
            src.to_str().unwrap(),
            "--out",
            env.path("x.bin").to_str().unwrap(),
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 2);
    let out = env.path("S.tif");
    let gpu = if on_ci() { "off" } else { "auto" };
    let r = genzo_with(
        &[
            "render",
            src.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--gpu",
            gpu,
            "--json",
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let v = r.json();
    assert!(v["backend"] == "gpu" || v["backend"] == "cpu");
    let img = image::open(&out).unwrap();
    assert_eq!((img.width(), img.height()), (60, 40));
    assert_eq!(img.color(), image::ColorType::Rgb16, "TIFF は 16bit");
    assert_eq!(hashes(&env.photos), before);
}

/// `bench`: 1.8 節の記録（平均・95 パーセンタイル・最大、条件、環境）を JSON に書き、2 回目は前回と比べる。
#[test]
fn bench_records_results_and_compares_with_the_previous_run() {
    let env = Env::new();
    let dir = env.path("bench");
    let args = |target: &str, runs: &str| -> Vec<String> {
        [
            "bench",
            target,
            "--synthetic",
            "64x48",
            "--runs",
            runs,
            "--out",
            dir.to_str().unwrap(),
            "--gpu",
            "off",
            "--quiet",
            "--json",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };
    let run = |a: &[String]| {
        let refs: Vec<&str> = a.iter().map(String::as_str).collect();
        genzo_with(&refs, &[], None)
    };
    // プレビュー（応答時間は 1.8 節の規則の 30 回）。
    let r = run(&args("preview", "30"));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let v = r.json();
    let reports = v["reports"].as_array().unwrap();
    let names: Vec<&str> = reports
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "cli_preview_stage_c",
            "cli_preview_wb_drag_draft",
            "cli_preview_wb_release_final",
            "cli_preview_wb_release_final"
        ]
    );
    let conditions: Vec<&str> = reports
        .iter()
        .map(|r| r["result"]["condition"].as_str().unwrap())
        .collect();
    assert_eq!(conditions, vec!["warm", "warm", "cold", "warm"]);
    for rep in reports {
        let res = &rep["result"];
        assert_eq!(res["iterations"], 30);
        assert_eq!(res["meets_iteration_rule"], true);
        for k in ["mean_ms", "p95_ms", "max_ms"] {
            assert!(res["stats"][k].as_f64().unwrap() >= 0.0, "{k}");
        }
        assert!(res["target_ms"].as_f64().is_some());
        assert_eq!(res["notes"]["resolution"], "64x48");
        assert_eq!(res["notes"]["backend"], "cpu");
        assert!(res["environment"]["os"].as_str().is_some());
        assert_eq!(res["inputs"][0]["id"], "synthetic:cli_scene");
        assert!(rep["comparison"].is_null(), "初回は比べる相手がない");
        assert!(PathBuf::from(rep["path"].as_str().unwrap()).is_file());
    }
    let expected_input = if genzo_raw::LIBRAW_ENABLED {
        "worker_dng"
    } else {
        "in_memory"
    };
    assert_eq!(reports[0]["result"]["notes"]["input"], expected_input);
    // 2 回目は前回と比べる（悪化したかどうかは時間で変わるので、終了コードとの対応だけを確かめる）。
    let r = run(&args("preview", "30"));
    let v = r.json();
    for rep in v["reports"].as_array().unwrap() {
        assert!(rep["comparison"].is_object(), "{rep}");
    }
    let regressed = v["regressed"].as_bool().unwrap();
    assert_eq!(r.code, if regressed { 1 } else { 0 }, "{}", r.stderr);
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("cli_preview_stage_c.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(file["runs"].as_array().unwrap().len(), 2);

    // 書き出し（時間の内訳）。回数が規則（5 回）に足りないことも記録する。
    let r = run(&args("export", "2"));
    assert_eq!(r.code, 0, "{}", r.stderr);
    let v = r.json();
    let rep = &v["reports"][0];
    assert_eq!(rep["name"], "cli_export_jpeg");
    assert_eq!(rep["result"]["condition"], "cold");
    assert_eq!(rep["result"]["meets_iteration_rule"], false);
    assert_eq!(rep["passed"], false, "規則を満たさない計測は合格にしない");
    let phases = rep["result"]["phases"].as_object().unwrap();
    for k in ["render", "encode", "write"] {
        assert!(phases.contains_key(k), "{phases:?}");
    }
    assert_eq!(phases.contains_key("decode"), genzo_raw::LIBRAW_ENABLED);

    // ファイルを入力にする（ワーカーで展開する時間も内訳に入る。入力のハッシュを記録する）。
    let jpg = env.photos.join("bench.jpg");
    write_jpeg(&jpg, (64, 48), 4, None, false);
    let before = hashes(&env.photos);
    let r = genzo_with(
        &[
            "bench",
            "export",
            "--file",
            jpg.to_str().unwrap(),
            "--runs",
            "1",
            "--out",
            dir.to_str().unwrap(),
            "--gpu",
            "off",
            "--quiet",
            "--json",
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let v = r.json();
    let res = &v["reports"][0]["result"];
    assert_eq!(res["notes"]["input"], "file");
    assert_eq!(res["notes"]["source"], "image");
    assert_eq!(res["inputs"][0]["id"], "file:bench.jpg");
    assert_eq!(
        res["inputs"][0]["sha256"],
        genzo_testkit::record::sha256_file_hex(&jpg).unwrap()
    );
    assert!(res["phases"].as_object().unwrap().contains_key("decode"));
    assert_eq!(hashes(&env.photos), before, "入力は変わらない");
    // --file と --synthetic は同時に指定できない。
    let r = genzo_with(
        &[
            "bench",
            "export",
            "--file",
            jpg.to_str().unwrap(),
            "--synthetic",
            "64x48",
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 2);
}

/// ゴミ箱への移動（`--yes`）。OS のゴミ箱を汚さないよう、Linux で XDG_DATA_HOME を一時フォルダにして
/// 確かめる（freedesktop.org のゴミ箱。trash crate は同じマウントならホームのゴミ箱へ移す）。
#[cfg(target_os = "linux")]
#[test]
fn trash_with_yes_moves_files_to_the_trash() {
    let env = Env::new();
    write_jpeg(&env.photos.join("T.jpg"), (32, 24), 9, None, false);
    write_jpeg(&env.photos.join("K.jpg"), (32, 24), 8, None, false);
    env.json(&["catalog", "init"]);
    env.json(&["import", env.photos.to_str().unwrap(), "--no-previews"]);
    let all = env.json(&["search"]);
    let t = id_of(&all, "T.jpg");
    let xdg = env.path("xdg");
    std::fs::create_dir_all(&xdg).unwrap();
    let catalog = env.catalog.to_string_lossy().into_owned();
    let r = genzo_with(
        &[
            "trash",
            &t.to_string(),
            "--yes",
            "--catalog",
            &catalog,
            "--gpu",
            "off",
            "--json",
        ],
        &[("XDG_DATA_HOME", &xdg)],
        None,
    );
    assert_eq!(r.code, 0, "{}\n{}", r.stdout, r.stderr);
    let v = r.json();
    assert_eq!(v["report"]["trashed_files"].as_array().unwrap().len(), 1);
    assert!(!env.photos.join("T.jpg").exists());
    assert!(xdg.join("Trash").join("files").join("T.jpg").is_file());
    assert!(env.photos.join("K.jpg").is_file(), "他のファイルは変えない");
    assert_eq!(names(&env.json(&["search"])), vec!["K.jpg"]);
}

/// `--json` では、使い方の誤り（clap の解釈の誤りを含む）も標準出力に `{"error": {"kind": "usage"}}`
/// の JSON を 1 つだけ出す（終了コードは 2。標準出力を JSON として読む側が空の出力で止まらないように）。
#[test]
fn usage_errors_are_json_with_the_json_flag() {
    let env = Env::new();
    for args in [
        &["rate", "1", "9", "--json"][..],
        &["--json", "search", "--limit", "-1"],
        &["--json", "nosuchcommand"],
        &["--json"],
        &["develop", "set", "1", "--json"],
    ] {
        let r = env.run(args);
        assert_eq!(r.code, 2, "{args:?}: {}", r.stderr);
        let v = r.json();
        assert_eq!(v["error"]["kind"], "usage", "{args:?}");
        assert!(
            !v["error"]["message"].as_str().unwrap().is_empty(),
            "{args:?}"
        );
        assert!(!r.stderr.is_empty(), "説明は標準エラーにも出す");
    }
    // develop set の入力がない場合は、--json <FILE> で指定することを案内する。
    let r = env.run(&["develop", "set", "1"]);
    assert_eq!(r.code, 2);
    assert!(r.stderr.contains("--json <FILE>"), "{}", r.stderr);
    // --json がなければ標準出力には何も出さない。--help は JSON にしない（終了コード 0）。
    let r = env.run(&["rate", "1", "9"]);
    assert_eq!(r.code, 2);
    assert!(r.stdout.is_empty(), "{}", r.stdout);
    let help = genzo_with(&["search", "--help", "--json"], &[], None);
    assert_eq!(help.code, 0);
    assert!(help.stdout.contains("--min-rating"));
}

/// Windows で作られがちな文字コードの設定ファイル（BOM 付きの UTF-8・CRLF、Windows PowerShell 5.1 の
/// `>` が作る BOM 付きの UTF-16LE）と、空白・日本語を含むパス（引数はシェルを通さずに渡す）。
#[test]
fn windows_encodings_and_paths_with_spaces_and_japanese() {
    let env = Env::new();
    let dir = env.photos.join("旅行 2024").join("京都 1日目");
    std::fs::create_dir_all(&dir).unwrap();
    let photo = dir.join("夕焼け 1.jpg");
    write_jpeg(&photo, (48, 32), 7, Some("2024-05-01 18:00:00"), false);
    let before = hashes(&env.photos);
    env.json(&["catalog", "init"]);
    let report = env.json(&["import", env.photos.to_str().unwrap()]);
    assert_eq!(report["added"], 1, "{report}");
    let all = env.json(&["search", "--folder", dir.to_str().unwrap()]);
    let id = id_of(&all, "夕焼け 1.jpg").to_string();

    let bom = env.path("設定 bom.json");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"{\r\n  \"exposure_ev\": 0.5\r\n}\r\n");
    std::fs::write(&bom, bytes).unwrap();
    let set = env.json(&["develop", "set", &id, "--json", bom.to_str().unwrap()]);
    assert_eq!(set["settings"]["exposure_ev"], 0.5);
    let utf16 = env.path("設定 utf16.json");
    let mut bytes = vec![0xFF, 0xFE];
    for u in "{\"contrast\": 25}\r\n".encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&utf16, bytes).unwrap();
    let set = env.json(&["develop", "set", &id, "--settings", utf16.to_str().unwrap()]);
    assert_eq!(set["settings"]["contrast"], 25.0);
    assert_eq!(set["settings"]["exposure_ev"], 0.5, "マージ");
    // 標準入力の BOM 付きも読める。Shift_JIS などは invalid_input（終了コード 1）。
    let r = env.run_stdin(
        &["develop", "set", &id, "--json", "-", "--json"],
        Some("\u{feff}{\"contrast\": 30}"),
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    let sjis = env.path("sjis.json");
    std::fs::write(&sjis, [b'{', b'"', 0x82, 0xA0, b'"', b':', b'1', b'}']).unwrap();
    let r = env.run(&[
        "develop",
        "set",
        &id,
        "--json",
        sjis.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(r.code, 1);
    assert_eq!(r.json()["error"]["kind"], "invalid_input");

    // 空白・日本語を含む書き出し先と、カタログなしの現像。
    let out = env.path("書き出し 先");
    let e = env.json(&["export", &id, "--out", out.to_str().unwrap()]);
    let written = written_path(&e);
    assert_eq!(written.file_name().unwrap(), "夕焼け 1.jpg");
    assert!(written.starts_with(&out));
    let rendered = env.path("現像 結果").join("夕焼け 1.png");
    let r = genzo_with(
        &[
            "render",
            photo.to_str().unwrap(),
            "--out",
            rendered.to_str().unwrap(),
            "--settings",
            bom.to_str().unwrap(),
            "--gpu",
            "off",
            "--json",
        ],
        &[],
        None,
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(r.json()["output"], rendered.to_string_lossy().as_ref());
    assert!(rendered.is_file());
    assert_eq!(hashes(&env.photos), before, "元ファイルは変わらない");
}

/// データの保全: (1) カタログのデータのフォルダ（プレビューのキャッシュの JPEG）を写真として取り込まない、
/// (2) 復元先・書き出し先に写真のファイルを取り違えて指定しても、そのファイルを変えない。
#[test]
fn app_data_and_unrelated_files_are_not_touched() {
    let env = Env::new();
    // カタログを写真のフォルダの中に置く（Lightroom の既定のように、ピクチャの中にカタログがある配置）。
    let catalog = env.photos.join("genzo").join("catalog.db");
    let data = env.photos.join("genzo").join("catalog-data");
    write_jpeg(&env.photos.join("P.jpg"), (64, 48), 3, None, false);
    write_jpeg(&env.photos.join("Q.jpg"), (64, 48), 4, None, false);
    let photo_hashes = || {
        let mut h = hashes(&env.photos);
        h.retain(|p, _| !p.starts_with(env.photos.join("genzo")));
        h
    };
    let before = photo_hashes();
    let cat = catalog.to_string_lossy().into_owned();
    let run = |args: &[&str]| {
        let mut all = args.to_vec();
        all.extend(["--catalog", &cat, "--gpu", "off", "--quiet"]);
        genzo_with(&all, &[], None)
    };
    let ok_json = |args: &[&str]| {
        let mut all = args.to_vec();
        all.push("--json");
        let r = run(&all);
        assert_eq!(r.code, 0, "{args:?}\n{}\n{}", r.stdout, r.stderr);
        r.json()
    };
    ok_json(&["catalog", "init"]);
    // 1 回目の取り込みの後、現像結果のプレビュー（JPEG）がデータのフォルダにできる。
    let photos = env.photos.to_str().unwrap();
    assert_eq!(ok_json(&["import", photos])["added"], 2);
    let mut jpegs_in_data = 0;
    let mut stack = vec![data.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "jpg") {
                jpegs_in_data += 1;
            }
        }
    }
    assert!(jpegs_in_data > 0, "プレビューのキャッシュがある前提");
    // 2 回目の取り込みでも、キャッシュの JPEG は登録しない。
    let again = ok_json(&["import", photos]);
    assert_eq!(again["added"], 0, "{again}");
    assert_eq!(again["files_found"], 2, "{again}");
    assert_eq!(names(&ok_json(&["search"])), vec!["P.jpg", "Q.jpg"]);
    // データのフォルダそのものは取り込めない。
    let r = run(&["import", data.to_str().unwrap(), "--json"]);
    assert_eq!(r.code, 1, "{}", r.stdout);
    assert_eq!(r.json()["error"]["kind"], "invalid_input");

    // 復元先に写真を指定しても変えない（退避の名前に変えない）。
    let backup = ok_json(&["catalog", "backup"]);
    let backup_path = backup["path"].as_str().unwrap().to_owned();
    let p = env.photos.join("P.jpg");
    let r = run(&[
        "catalog",
        "restore",
        &backup_path,
        "--to",
        p.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(r.code, 1, "{}", r.stdout);
    assert_eq!(r.json()["error"]["kind"], "invalid_input");
    // 書き出し先に写真のファイルを指定しても変えない（使い方の誤り）。
    let all = ok_json(&["search"]);
    let q = id_of(&all, "Q.jpg").to_string();
    let r = run(&["export", &q, "--out", p.to_str().unwrap(), "--json"]);
    assert_eq!(r.code, 2, "{}", r.stdout);
    assert_eq!(r.json()["error"]["kind"], "usage");
    assert_eq!(photo_hashes(), before, "写真のファイルは変わらない");
    let leftovers: Vec<_> = std::fs::read_dir(&env.photos)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains("restore"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    // 今のカタログへの復元は、元のカタログを退避して行う。
    let r = run(&["catalog", "restore", &backup_path, "--to", &cat, "--json"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let displaced = PathBuf::from(r.json()["displaced"].as_str().unwrap());
    assert!(displaced.is_file());
    assert_eq!(ok_json(&["catalog", "info"])["counts"]["variants"], 2);
}

/// 異常終了で `-wal`・`-shm` が残り、カタログの本体だけを失った状態からの復元（指摘 F01）。残った
/// `-wal`（別の DB のページ）を退避しないと、次に開いたときに復元したカタログへ適用されて壊れる。
#[test]
fn restore_moves_a_leftover_wal_aside_when_the_catalog_is_gone() {
    let env = Env::new();
    write_jpeg(
        &env.photos.join("A.jpg"),
        (32, 24),
        1,
        Some("2024-05-01 10:00:00"),
        false,
    );
    write_jpeg(
        &env.photos.join("B.jpg"),
        (32, 24),
        2,
        Some("2024-05-01 11:00:00"),
        false,
    );
    env.json(&["catalog", "init"]);
    assert_eq!(
        env.json(&["import", env.photos.to_str().unwrap()])["added"],
        2
    );
    let backup = env.json(&[
        "catalog",
        "backup",
        "--to",
        env.path("bk").to_str().unwrap(),
    ]);
    let backup_path = backup["path"].as_str().unwrap().to_owned();
    let side = |suffix: &str| {
        let mut s = env.catalog.as_os_str().to_owned();
        s.push(suffix);
        PathBuf::from(s)
    };
    // 確定済みの更新を `-wal` に残したまま異常終了した状態を作る（開いて書き込み、閉じる前に `-wal`・
    // `-shm` を写しておき、閉じた後に戻す。OS によらず同じ状態を作れる）。
    {
        let mut cat = genzo_catalog::Catalog::open(&env.catalog).unwrap();
        let ids: Vec<genzo_model::VariantId> = (1..=2).map(genzo_model::VariantId::new).collect();
        cat.set_rating(&ids, genzo_model::Rating::new(5).unwrap())
            .unwrap();
        assert!(side("-wal").is_file(), "WAL が使われている前提");
        std::fs::copy(side("-wal"), env.path("saved-wal")).unwrap();
        std::fs::copy(side("-shm"), env.path("saved-shm")).unwrap();
        cat.close().unwrap();
    }
    // カタログの本体だけを失う（誤って削除した、同期ツールで消えたなど）。
    std::fs::remove_file(&env.catalog).unwrap();
    for (saved, suffix) in [("saved-wal", "-wal"), ("saved-shm", "-shm")] {
        let _ = std::fs::remove_file(side(suffix));
        std::fs::rename(env.path(saved), side(suffix)).unwrap();
    }
    let cat = env.catalog.to_str().unwrap().to_owned();
    let rs = env.json(&["catalog", "restore", &backup_path, "--to", &cat]);
    assert_eq!(rs["displaced"], Value::Null, "本体はなかった: {rs}");
    let displaced: Vec<PathBuf> = rs["displaced_files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| PathBuf::from(v.as_str().unwrap()))
        .collect();
    assert_eq!(displaced.len(), 2, "{rs}");
    assert!(displaced.iter().all(|p| p.is_file()), "{displaced:?}");
    assert!(!side("-wal").exists(), "古い -wal は退避した");
    assert!(!side("-shm").exists());
    // 復元したカタログは壊れていない（古い -wal が適用されていない）。
    let check = env.json(&["catalog", "check"]);
    assert_eq!(check["ok"], true, "{check}");
    let info = env.json(&["catalog", "info"]);
    assert_eq!(info["counts"]["variants"], 2, "{info}");
}

// ---------------------------------------------------------------------------
// Ctrl+C（K3）
// ---------------------------------------------------------------------------

/// Ctrl+C（SIGINT）の扱い（K3）。Unix だけ: 端末で Ctrl+C を押したときと同じく、`genzo` を別の
/// プロセスグループで起動し、そのグループ全体（`genzo` と、同じグループにいればワーカー）に SIGINT を
/// 送る。Windows の Ctrl+C（コンソールのイベント）はテストから送れないため飛ばす。
///
/// 結果は終了コード・出力の内容・カタログの状態で決め、時間では決めない（[`ctrl_c::WAIT_LIMIT`] は、
/// 止まったまま終わらないテストにしないための上限）。
#[cfg(unix)]
mod ctrl_c {
    use super::*;
    use std::io::{BufRead, BufReader, Read};
    use std::os::unix::process::CommandExt;
    use std::process::Child;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// 待つ上限（過ぎたら失敗にする）。
    pub const WAIT_LIMIT: Duration = Duration::from_secs(120);

    /// 終了コード: Ctrl+C で中断した。
    const EXIT_INTERRUPTED: i32 = genzo_cli::EXIT_INTERRUPTED as i32;

    /// 別のプロセスグループで実行中の `genzo`。
    struct Running {
        child: Child,
        /// 標準エラーの行（読んだ順に届く）。
        lines: mpsc::Receiver<String>,
        /// これまでに受け取った標準エラーの行。
        seen: Vec<String>,
        stdout: Option<std::thread::JoinHandle<String>>,
    }

    impl Running {
        /// `genzo` を別のプロセスグループ（グループの ID は `genzo` のプロセス ID）で起動する（カタログを
        /// 指定し、GPU は使わない。`--quiet` は付けないので、進捗が標準エラーに出る）。
        fn spawn(env: &Env, args: &[&str]) -> Self {
            let catalog = env.catalog.to_string_lossy().into_owned();
            let mut child = Command::new(GENZO)
                .args(args)
                .args(["--catalog", &catalog, "--gpu", "off"])
                .env_remove("GENZO_CATALOG")
                .env_remove("GENZO_DATA_DIR")
                .env_remove("GENZO_LOG")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .process_group(0)
                .spawn()
                .expect("genzo を起動できる");
            let mut out = child.stdout.take().unwrap();
            let stdout = std::thread::spawn(move || {
                let mut s = String::new();
                let _ = out.read_to_string(&mut s);
                s
            });
            let err = child.stderr.take().unwrap();
            let (tx, lines) = mpsc::channel();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines() {
                    let Ok(line) = line else { break };
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
            Self {
                child,
                lines,
                seen: Vec::new(),
                stdout: Some(stdout),
            }
        }

        fn pgid(&self) -> libc::pid_t {
            libc::pid_t::try_from(self.child.id()).unwrap()
        }

        /// 標準エラーに `pred` に合う行が出るまで待つ。
        fn wait_stderr(&mut self, what: &str, pred: impl Fn(&str) -> bool) {
            let deadline = Instant::now() + WAIT_LIMIT;
            loop {
                let Some(rest) = deadline.checked_duration_since(Instant::now()) else {
                    self.fail(&format!("{what}が標準エラーに出ない"));
                };
                match self.lines.recv_timeout(rest) {
                    Ok(line) => {
                        let hit = pred(&line);
                        self.seen.push(line);
                        if hit {
                            return;
                        }
                    }
                    Err(_) => self.fail(&format!("{what}が標準エラーに出ないまま終わった")),
                }
            }
        }

        /// プロセスグループ全体に SIGINT を送る（端末で Ctrl+C を押したときと同じ）。
        fn ctrl_c(&self) {
            // SAFETY: シグナルを送るだけで、メモリを扱わない。
            let r = unsafe { libc::kill(-self.pgid(), libc::SIGINT) };
            assert_eq!(
                r,
                0,
                "SIGINT を送れない: {}",
                std::io::Error::last_os_error()
            );
        }

        /// 終了を待ち、結果を返す（シグナルで終わった場合の終了コードは -1）。
        fn finish(mut self) -> Run {
            let deadline = Instant::now() + WAIT_LIMIT;
            let status = loop {
                if let Some(s) = self.child.try_wait().unwrap() {
                    break s;
                }
                if Instant::now() > deadline {
                    self.fail("終了しない");
                }
                std::thread::sleep(Duration::from_millis(20));
            };
            // 標準エラーの残り（読み取りのスレッドは、パイプが閉じると終わる）。
            while let Some(rest) = deadline.checked_duration_since(Instant::now()) {
                match self.lines.recv_timeout(rest) {
                    Ok(line) => self.seen.push(line),
                    Err(_) => break,
                }
            }
            let stdout = self.stdout.take().unwrap().join().unwrap();
            Run {
                code: status.code().unwrap_or(-1),
                stdout,
                stderr: format!("（終了の状態: {status:?}）\n{}", self.seen.join("\n")),
            }
        }

        /// プロセスグループを強制終了させて失敗にする。
        fn fail(&mut self, why: &str) -> ! {
            self.kill_group();
            panic!("{why}\nstderr:\n{}", self.seen.join("\n"));
        }

        fn kill_group(&mut self) {
            if matches!(self.child.try_wait(), Ok(None)) {
                // SAFETY: シグナルを送るだけ。
                unsafe { libc::kill(-self.pgid(), libc::SIGKILL) };
                let _ = self.child.wait();
            }
        }
    }

    impl Drop for Running {
        /// テストが途中で失敗しても、プロセスを残さない。
        fn drop(&mut self) {
            self.kill_group();
        }
    }

    /// 取り込みの対象の、小さな JPEG を `n` 枚作る。
    fn photos(env: &Env, n: usize) {
        for i in 0..n {
            write_jpeg(
                &env.photos.join(format!("P{i:03}.jpg")),
                (32, 24),
                i as u8,
                None,
                false,
            );
        }
    }

    fn count(v: &Value, key: &str) -> u64 {
        v[key]
            .as_u64()
            .unwrap_or_else(|| panic!("{key} がない: {v}"))
    }

    /// 取り込みの途中の Ctrl+C: 取り込みを取り消し、カタログを正常に閉じてから終了コード 130 で終わる。
    /// ワーカーは巻き込まれない（端末の Ctrl+C はプロセスグループ全体に届くが、ワーカーが死ぬと、処理中の
    /// ファイルが `status = error` で登録されたり、サムネイルの失敗になったりする）。取り込み直すと続きから
    /// 処理する。
    #[test]
    fn ctrl_c_cancels_the_import_and_closes_the_catalog() {
        const N: u64 = 300;
        let env = Env::new();
        photos(&env, N as usize);
        env.json(&["catalog", "init"]);
        let dir = env.photos.to_str().unwrap();
        let mut r = Running::spawn(&env, &["import", dir, "--json"]);
        // 取り込みが始まった（最初の進捗が出た）ところで Ctrl+C を押す。
        r.wait_stderr("取り込みの進捗", |l| l.starts_with("取り込み: "));
        r.ctrl_c();
        let run = r.finish();
        assert_eq!(
            run.code, EXIT_INTERRUPTED,
            "stdout: {}\nstderr: {}",
            run.stdout, run.stderr
        );
        assert!(run.stderr.contains("中断"), "{}", run.stderr);
        // 途中までの結果を JSON で出す。
        let report = run.json();
        assert_eq!(report["cancelled"], true, "{report}");
        assert!(count(&report, "added") < N, "{report}");
        for key in ["errors", "not_registered", "thumbnail_failures"] {
            assert_eq!(
                report[key].as_array().map(Vec::len),
                Some(0),
                "ワーカーは Ctrl+C で終了しない（{key}）: {report}\n{}",
                run.stderr
            );
        }
        // カタログを正常に閉じた（次に開いたときに「正常に終了しなかった」にならない。ロックも外れている）。
        let info = env.json(&["catalog", "info"]);
        assert_eq!(info["previous_shutdown"], "clean", "{info}");
        // 取り込み直すと、続きから処理して全件がそろう。読めないファイルとして登録したものはない。
        let again = env.json(&["import", dir, "--no-previews"]);
        assert_eq!(again["cancelled"], false, "{again}");
        assert_eq!(
            count(&again, "added") + count(&again, "unchanged"),
            N,
            "{again}"
        );
        let all = env.json(&["search", "--limit", "0"]);
        assert_eq!(items(&all).len() as u64, N);
        assert!(
            items(&all).iter().all(|v| v["file_status"] == "ok"),
            "status = error で登録されたファイルがない: {all}"
        );
    }

    /// 取り込みの後のサムネイルの作り直し（バックグラウンドのジョブ）を待っている間の Ctrl+C: 最後まで
    /// 待たずに取り消し、カタログを正常に閉じて終了コード 130 で終わる（作り直せなかったものは、次に
    /// 開いたときに作り直す。genzo-api）。
    #[test]
    fn ctrl_c_while_waiting_for_background_jobs_cancels_them() {
        const N: u64 = 150;
        let env = Env::new();
        photos(&env, N as usize);
        env.json(&["catalog", "init"]);
        let dir = env.photos.to_str().unwrap();
        let mut r = Running::spawn(&env, &["import", dir, "--json"]);
        r.wait_stderr("サムネイルの作り直しの進捗", |l| {
            l.starts_with("サムネイルの作り直し: ")
        });
        r.ctrl_c();
        let run = r.finish();
        assert_eq!(
            run.code, EXIT_INTERRUPTED,
            "stdout: {}\nstderr: {}",
            run.stdout, run.stderr
        );
        // 取り込み自体は終わっていた。
        let report = run.json();
        assert_eq!(report["cancelled"], false, "{report}");
        assert_eq!(count(&report, "added"), N, "{report}");
        // 作り直しは途中でやめた（最後の進捗が全件に届いていない）。
        let last = run
            .stderr
            .lines()
            .filter_map(|l| l.strip_prefix("サムネイルの作り直し: "))
            .filter_map(|s| s.split('/').next()?.parse::<u64>().ok())
            .max()
            .unwrap();
        assert!(last < N, "作り直しを最後まで待った: {}", run.stderr);
        let info = env.json(&["catalog", "info"]);
        assert_eq!(info["previous_shutdown"], "clean", "{info}");
    }

    /// 書き出しの途中の Ctrl+C: 書き出しを取り消し、途中までの結果を出して、カタログを正常に閉じてから
    /// 終了コード 130 で終わる。書き出し先には、書き終えたファイルだけが残る（途中のファイル・一時ファイルを
    /// 残さない）。
    #[test]
    fn ctrl_c_cancels_the_export_and_leaves_only_complete_files() {
        const N: u64 = 120;
        let env = Env::new();
        photos(&env, N as usize);
        env.json(&["catalog", "init"]);
        env.json(&["import", env.photos.to_str().unwrap(), "--no-previews"]);
        let ids: Vec<String> = items(&env.json(&["search", "--limit", "0"]))
            .iter()
            .map(|v| v["variant_id"].to_string())
            .collect();
        assert_eq!(ids.len() as u64, N);
        let out = env.path("out");
        let mut args: Vec<&str> = vec!["export"];
        args.extend(ids.iter().map(String::as_str));
        args.extend(["--out", out.to_str().unwrap(), "--json"]);
        let mut r = Running::spawn(&env, &args);
        r.wait_stderr("書き出しの進捗", |l| l.starts_with("書き出し: "));
        r.ctrl_c();
        let run = r.finish();
        assert_eq!(
            run.code, EXIT_INTERRUPTED,
            "stdout: {}\nstderr: {}",
            run.stdout, run.stderr
        );
        let v = run.json();
        let report = &v["report"];
        assert_eq!(report["cancelled"], true, "{v}");
        assert_eq!(count(report, "failed"), 0, "{v}");
        let written = count(report, "written");
        assert!(written < N, "{v}");
        // 書き出し先には、書き終えたファイルだけがある（どれも読める JPEG）。
        let files: Vec<PathBuf> = std::fs::read_dir(&out)
            .map(|d| d.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default();
        assert_eq!(files.len() as u64, written, "{files:?}");
        for f in &files {
            image::open(f).unwrap_or_else(|e| panic!("{} を読めない: {e}", f.display()));
        }
        let info = env.json(&["catalog", "info"]);
        assert_eq!(info["previous_shutdown"], "clean", "{info}");
    }

    /// カタログを開く前（現像設定の JSON を読んでいる間）の Ctrl+C: 取り消すものがないので、すぐに終了
    /// コード 130 で終わる（2 回目の Ctrl+C と同じ、すぐに終了する経路）。読み取りで止まる状態は、名前付き
    /// パイプ（FIFO）で作る。
    #[test]
    fn ctrl_c_before_opening_the_catalog_exits_at_once() {
        use std::os::unix::fs::OpenOptionsExt;
        let env = Env::new();
        let fifo = env.path("settings.fifo");
        let made = Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success());
        if !made {
            eprintln!("mkfifo を使えないため飛ばす");
            return;
        }
        let mut r = Running::spawn(
            &env,
            &["develop", "set", "1", "--json", fifo.to_str().unwrap()],
        );
        // 書き込み側を開けたら、genzo が読み取り側を開いている（Ctrl+C の扱いは、その前に用意している）。
        // genzo は書き込みを待って止まる。
        let deadline = Instant::now() + WAIT_LIMIT;
        let writer = loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
            {
                Ok(f) => break f,
                Err(e) if e.raw_os_error() == Some(libc::ENXIO) && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => r.fail(&format!("名前付きパイプを開けない: {e}")),
            }
        };
        r.ctrl_c();
        let run = r.finish();
        drop(writer);
        assert_eq!(
            run.code, EXIT_INTERRUPTED,
            "stdout: {}\nstderr: {}",
            run.stdout, run.stderr
        );
        assert!(
            !env.catalog.exists(),
            "カタログは作らない（開く前に終わった）"
        );
    }
}
