//! 埋め込みのレンズ補正の情報の読み取りのテスト（合成の TIFF・DNG）。

use genzo_model::PhotoMetadata;

use super::*;
use crate::dng::{DngOptions, encode};
use crate::{CfaPattern, RawImage};

/// テスト用の TIFF の値（型と、要素ごとの値）。
#[derive(Clone)]
enum V {
    Short(Vec<u16>),
    SShort(Vec<i16>),
    Long(Vec<u32>),
}

impl V {
    fn typ(&self) -> u16 {
        match self {
            V::Short(_) => 3,
            V::SShort(_) => 8,
            V::Long(_) => 4,
        }
    }
    fn count(&self) -> u32 {
        (match self {
            V::Short(v) => v.len(),
            V::SShort(v) => v.len(),
            V::Long(v) => v.len(),
        }) as u32
    }
    fn bytes(&self, le: bool) -> Vec<u8> {
        match self {
            V::Short(v) => v
                .iter()
                .flat_map(|x| if le { x.to_le_bytes() } else { x.to_be_bytes() })
                .collect(),
            V::SShort(v) => v
                .iter()
                .flat_map(|x| if le { x.to_le_bytes() } else { x.to_be_bytes() })
                .collect(),
            V::Long(v) => v
                .iter()
                .flat_map(|x| if le { x.to_le_bytes() } else { x.to_be_bytes() })
                .collect(),
        }
    }
}

/// 最小の TIFF を作る: IFD0（`ifd0` の項目。`sub` があれば SubIFDs で 1 つの SubIFD を指す）。
/// `SUB_PLACEHOLDER` のタグの値は SubIFD の位置に置き換える。
fn tiff(le: bool, ifd0: &[(u16, V)], sub: Option<&[(u16, V)]>) -> Vec<u8> {
    fn u16b(v: u16, le: bool) -> [u8; 2] {
        if le { v.to_le_bytes() } else { v.to_be_bytes() }
    }
    fn u32b(v: u32, le: bool) -> [u8; 4] {
        if le { v.to_le_bytes() } else { v.to_be_bytes() }
    }
    // IFD を `at` に置いたときのバイト列（値は IFD の直後に置く）。
    fn ifd_bytes(entries: &[(u16, V)], at: u32, le: bool) -> Vec<u8> {
        let mut sorted = entries.to_vec();
        sorted.sort_by_key(|(t, _)| *t);
        let mut table = u16b(sorted.len() as u16, le).to_vec();
        let mut data = Vec::new();
        let data_start = at + 2 + 12 * sorted.len() as u32 + 4;
        for (tag, v) in &sorted {
            table.extend_from_slice(&u16b(*tag, le));
            table.extend_from_slice(&u16b(v.typ(), le));
            table.extend_from_slice(&u32b(v.count(), le));
            let b = v.bytes(le);
            if b.len() <= 4 {
                let mut inline = [0u8; 4];
                inline[..b.len()].copy_from_slice(&b);
                table.extend_from_slice(&inline);
            } else {
                table.extend_from_slice(&u32b(data_start + data.len() as u32, le));
                data.extend_from_slice(&b);
                if data.len() % 2 == 1 {
                    data.push(0);
                }
            }
        }
        table.extend_from_slice(&u32b(0, le));
        table.extend_from_slice(&data);
        table
    }
    let mut out = if le { b"II".to_vec() } else { b"MM".to_vec() };
    out.extend_from_slice(&u16b(42, le));
    out.extend_from_slice(&u32b(8, le));
    // IFD0 の大きさを先に求めて、SubIFD をその後に置く。
    let mut ifd0 = ifd0.to_vec();
    if sub.is_some() {
        ifd0.push((0x014A, V::Long(vec![0])));
    }
    let size0 = ifd_bytes(&ifd0, 8, le).len() as u32;
    let sub_at = 8 + size0 + (size0 % 2);
    if sub.is_some() {
        ifd0.last_mut().unwrap().1 = V::Long(vec![sub_at]);
    }
    out.extend_from_slice(&ifd_bytes(&ifd0, 8, le));
    if let Some(s) = sub {
        out.resize(sub_at as usize, 0);
        out.extend_from_slice(&ifd_bytes(s, sub_at, le));
    }
    out
}

fn sony_sample() -> SonyLensCorrection {
    SonyLensCorrection {
        distortion: (0..16).map(|i| -12 * i as i16).collect(),
        vignetting: (0..16).map(|i| 40 * i as i16).collect(),
        ca_red: (0..16).map(|i| 3 * i as i16 - 20).collect(),
        ca_blue: (0..16).map(|i| -2 * i as i16 + 7).collect(),
    }
}

fn sony_entries(s: &SonyLensCorrection) -> Vec<(u16, V)> {
    sony_tag_values(s)
        .into_iter()
        .filter_map(|(t, v)| v.map(|v| (t, V::SShort(v))))
        .collect()
}

#[test]
fn sony_tags_in_the_raw_sub_ifd_are_read_in_both_byte_orders() {
    let s = sony_sample();
    for le in [true, false] {
        let mut sub = vec![(0x00FE, V::Long(vec![0]))];
        sub.extend(sony_entries(&s));
        let file = tiff(le, &[(0x00FE, V::Long(vec![1]))], Some(&sub));
        let got = read_lens_correction_bytes(&file).unwrap().unwrap();
        assert_eq!(got, LensCorrectionData::SonyArw(s.clone()), "le = {le}");
        got.validate().unwrap();
        assert!(got.has_distortion() && got.has_vignetting() && got.has_chromatic_aberration());
        assert_eq!(got.source_id(), "sony_arw");
        assert_eq!(got.describe(), "Sony の ARW（歪曲・周辺減光・色収差）");
    }
}

#[test]
fn the_raw_sub_ifd_is_preferred_over_ifd0() {
    let s = sony_sample();
    let mut other = s.clone();
    other.distortion = vec![1; 16];
    let mut sub = vec![(0x00FE, V::Long(vec![0]))];
    sub.extend(sony_entries(&s));
    let file = tiff(true, &sony_entries(&other), Some(&sub));
    let got = read_lens_correction_bytes(&file).unwrap().unwrap();
    assert_eq!(got, LensCorrectionData::SonyArw(s));
}

#[test]
fn sony_tags_with_bad_counts_are_ignored_one_by_one() {
    // 歪曲の個数が値の数より多い・周辺減光の個数が上限を超える・色収差の個数が奇数。
    let entries = vec![
        (TAG_SONY_DISTORTION, V::SShort(vec![16, 1, 2, 3])),
        (TAG_SONY_VIGNETTING, V::SShort(vec![17; 18])),
        (
            TAG_SONY_CHROMATIC_ABERRATION,
            V::SShort(vec![5, 1, 2, 3, 4, 5]),
        ),
    ];
    let file = tiff(true, &entries, None);
    let got = read_lens_correction_bytes(&file).unwrap().unwrap();
    assert!(got.is_empty(), "{got:?}");
    // 節点の数が少ない（8 個）のは使える。
    let entries = vec![(
        TAG_SONY_DISTORTION,
        V::SShort((0..9).map(|i| if i == 0 { 8 } else { i }).collect()),
    )];
    let got = read_lens_correction_bytes(&tiff(true, &entries, None))
        .unwrap()
        .unwrap();
    assert!(got.has_distortion() && !got.has_vignetting());
    let LensCorrectionData::SonyArw(s) = got else {
        panic!()
    };
    assert_eq!(s.distortion, (1..9).collect::<Vec<i16>>());
}

#[test]
fn unsigned_short_values_are_read_as_signed() {
    // SHORT（型 3）で書かれていても、値は符号付きの 16 ビットとして読む（ExifTool の int16s）。
    let entries = vec![(TAG_SONY_DISTORTION, V::Short(vec![2, 65535, 65534]))];
    let got = read_lens_correction_bytes(&tiff(true, &entries, None))
        .unwrap()
        .unwrap();
    let LensCorrectionData::SonyArw(s) = got else {
        panic!()
    };
    assert_eq!(s.distortion, vec![-1, -2]);
}

fn dng_sample(planes: usize) -> DngLensCorrection {
    let plane = |k: f64| DngWarpPlane {
        kr: [1.0, -0.02 * k, 0.004, -0.0005],
        kt: [0.0, 0.0],
    };
    DngLensCorrection {
        warp: Some(DngWarpRectilinear {
            planes: (0..planes).map(|i| plane(1.0 + i as f64 * 0.01)).collect(),
            center: [0.5, 0.49],
        }),
        vignette: Some(DngVignetteRadial {
            k: [0.3, -0.05, 0.01, 0.0, 0.0],
            center: [0.5, 0.5],
        }),
    }
}

fn raw_image() -> RawImage {
    RawImage {
        width: 24,
        height: 24,
        cfa: CfaPattern::RGGB,
        data: vec![1000; 24 * 24],
        black_level: [512.0; 4],
        white_level: 16383.0,
        saturation_level: 16383.0,
        default_crop: None,
        as_shot_wb: [2.0, 1.0, 1.5, 1.0],
        cam_xyz: None,
        metadata: PhotoMetadata {
            make: Some("GenzoTest".into()),
            model: Some("Synth-1".into()),
            ..Default::default()
        },
        lens_correction: None,
    }
}

#[test]
fn dng_opcodes_round_trip_through_the_synthetic_dng() {
    for planes in [1, 3] {
        let d = dng_sample(planes);
        let options = DngOptions {
            lens_correction: Some(LensCorrectionData::DngOpcode(d.clone())),
            ..Default::default()
        };
        let file = encode(&raw_image(), &options).unwrap();
        let got = read_lens_correction_bytes(&file).unwrap().unwrap();
        assert_eq!(got, LensCorrectionData::DngOpcode(d), "{planes}");
        got.validate().unwrap();
        assert_eq!(got.has_chromatic_aberration(), planes == 3);
        assert_eq!(got.source_id(), "dng_opcode");
    }
    // 補正の情報のない DNG は None。
    assert_eq!(
        read_lens_correction_bytes(&encode(&raw_image(), &DngOptions::default()).unwrap()).unwrap(),
        None
    );
}

#[test]
fn sony_tags_written_into_a_dng_are_read_back() {
    // darktable が DNG に写すときと同じ IFD0 の置き場所。opcode がなければ Sony のタグを使う。
    let s = sony_sample();
    let mut image = raw_image();
    image.lens_correction = Some(LensCorrectionData::SonyArw(s.clone()));
    let file = encode(&image, &DngOptions::default()).unwrap();
    assert_eq!(
        read_lens_correction_bytes(&file).unwrap(),
        Some(LensCorrectionData::SonyArw(s))
    );
}

#[test]
fn opcode_list_skips_other_opcodes_and_rejects_bad_sizes() {
    let d = dng_sample(1);
    let mut list = encode_opcode_list(&d);
    // 先頭に知らない opcode（ID 9 の GainMap の形だけ）を足す。
    let mut extra = 9u32.to_be_bytes().to_vec();
    extra.extend_from_slice(&0x0103_0000u32.to_be_bytes());
    extra.extend_from_slice(&1u32.to_be_bytes());
    extra.extend_from_slice(&3u32.to_be_bytes());
    extra.extend_from_slice(&[1, 2, 3]);
    let count = u32::from_be_bytes([list[0], list[1], list[2], list[3]]) + 1;
    list.splice(0..4, count.to_be_bytes());
    list.splice(4..4, extra);
    assert_eq!(parse_opcode_list(&list).unwrap(), d);
    // 大きさが列の終わりを超える。
    let mut bad = encode_opcode_list(&d);
    bad.truncate(bad.len() - 4);
    assert!(parse_opcode_list(&bad).is_err());
    // 面の数が 2。
    let mut two = encode_opcode_list(&DngLensCorrection {
        warp: d.warp.clone(),
        vignette: None,
    });
    two[20..24].copy_from_slice(&2u32.to_be_bytes());
    assert!(parse_opcode_list(&two).is_err());
    // 件数が上限を超える。
    let mut many = 0u32.to_be_bytes().to_vec();
    many[..4].copy_from_slice(&(MAX_OPCODES + 1).to_be_bytes());
    assert!(parse_opcode_list(&many).is_err());
}

#[test]
fn files_that_are_not_tiff_or_are_broken() {
    assert_eq!(
        read_lens_correction_bytes(b"not a tiff file").unwrap(),
        None
    );
    assert_eq!(read_lens_correction_bytes(b"II").unwrap(), None);
    // IFD の位置がファイルの外。
    let mut f = b"II*\0".to_vec();
    f.extend_from_slice(&1000u32.to_le_bytes());
    assert!(read_lens_correction_bytes(&f).is_err());
    // IFD0 が自分自身を次の IFD・SubIFD として指す（循環しても止まる）。
    let mut looped = tiff(true, &[(0x00FE, V::Long(vec![0]))], None);
    let n = looped.len();
    // 次の IFD の位置（項目 1 つの IFD の終わりの 4 バイト）を 8 にする。
    looped[n - 4..].copy_from_slice(&8u32.to_le_bytes());
    assert_eq!(read_lens_correction_bytes(&looped).unwrap(), None);
    // 途中で切れた Sony のタグの値。
    let file = tiff(true, &sony_entries(&sony_sample()), None);
    assert!(read_lens_correction_bytes(&file[..file.len() - 10]).is_err());
}

#[test]
fn reading_from_a_file_matches_reading_from_memory() {
    let d = dng_sample(3);
    let options = DngOptions {
        lens_correction: Some(LensCorrectionData::DngOpcode(d.clone())),
        ..Default::default()
    };
    let bytes = encode(&raw_image(), &options).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lens.dng");
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        read_lens_correction(&path).unwrap(),
        Some(LensCorrectionData::DngOpcode(d))
    );
    assert!(read_lens_correction(dir.path().join("none.dng")).is_err());
}

#[test]
fn validation_rejects_bad_values() {
    let mut s = sony_sample();
    s.ca_blue.pop();
    assert!(LensCorrectionData::SonyArw(s).validate().is_err());
    let s = SonyLensCorrection {
        distortion: vec![1],
        ..Default::default()
    };
    assert!(LensCorrectionData::SonyArw(s).validate().is_err());
    let mut d = dng_sample(1);
    d.warp.as_mut().unwrap().planes[0].kr[0] = 0.0;
    assert!(LensCorrectionData::DngOpcode(d).validate().is_err());
    let mut d = dng_sample(1);
    d.vignette.as_mut().unwrap().k[2] = f64::NAN;
    assert!(LensCorrectionData::DngOpcode(d).validate().is_err());
    let mut d = dng_sample(1);
    d.warp.as_mut().unwrap().center = [5.0, 0.5];
    assert!(LensCorrectionData::DngOpcode(d).validate().is_err());
    // 半径方向の関数（取る位置の倍率）が範囲の外（検証の指摘: G の面の係数が小さいと、色収差の倍率 kR / kG が
    // 巨大になり、ステージ 7 の入力の範囲の余白が巨大になっていた）。
    for kr in [
        [1e-5, 0.0, 0.0, 0.0],
        [1.0, -0.8, 0.0, 0.0],
        [1.0, 0.0, 0.0, 3.0],
    ] {
        let mut d = dng_sample(3);
        d.warp.as_mut().unwrap().planes[1].kr = kr;
        let e = LensCorrectionData::DngOpcode(d).validate().unwrap_err();
        assert!(e.to_string().contains("WarpRectilinear"), "{kr:?}: {e}");
    }
    // 面ごとには範囲の中でも、R（B）と G の比（倍率色収差）が大きすぎる。
    let mut d = dng_sample(3);
    d.warp.as_mut().unwrap().planes[0].kr = [1.2, 0.0, 0.0, 0.0];
    d.warp.as_mut().unwrap().planes[1].kr = [1.0, 0.0, 0.0, 0.0];
    let e = LensCorrectionData::DngOpcode(d).validate().unwrap_err();
    assert!(e.to_string().contains("色収差"), "{e}");
    // 実際のレンズ程度の値（1 前後・比は 0.1 % 前後）は受け付ける。
    let mut d = dng_sample(3);
    d.warp.as_mut().unwrap().planes[0].kr = [1.001, -0.05, 0.01, 0.0];
    d.warp.as_mut().unwrap().planes[1].kr = [1.0, -0.05, 0.01, 0.0];
    d.warp.as_mut().unwrap().planes[2].kr = [0.999, -0.05, 0.01, 0.0];
    LensCorrectionData::DngOpcode(d).validate().unwrap();
    // RawImage の検証にも含まれる。
    let mut image = raw_image();
    image.lens_correction = Some(LensCorrectionData::SonyArw(SonyLensCorrection {
        vignetting: vec![0; 40],
        ..Default::default()
    }));
    assert!(matches!(
        image.validate(),
        Err(RawError::InvalidLensCorrection(_))
    ));
}

#[test]
fn json_form_is_tagged_by_kind() {
    let d = LensCorrectionData::SonyArw(SonyLensCorrection {
        distortion: vec![1, 2],
        ..Default::default()
    });
    let json = serde_json::to_string(&d).unwrap();
    assert!(json.starts_with(r#"{"kind":"sony_arw""#), "{json}");
    assert_eq!(
        serde_json::from_str::<LensCorrectionData>(&json).unwrap(),
        d
    );
}
