//! 他の crate の結合テスト用の関数（機能フラグ `test-support`。配布するビルドでは使わない）。
//!
//! 透かしの文字（EXP-03）のテストに使うフォントのファイルを、その場で作る（フォントのファイルはリポジトリに置かない・
//! ダウンロードしない。決定事項ログ No.167）。最小限の TrueType（`head`・`hhea`・`maxp`・`cmap`（形式 12）・`hmtx`・
//! `loca`・`glyf`）で、形は次のとおり（em 1000、アセンダー 800、ディセンダー −200）。
//!
//! | 字形 | 文字 | 形（フォントの単位） | 送り幅 |
//! |---|---|---|---|
//! | 0 `.notdef` | （ない文字） | 枠（外 50〜450 × 0〜700、内 100〜400 × 50〜650 の穴） | 500 |
//! | 1 | `A`・`あ` | 長方形 50〜650 × 0〜700 | 700 |
//! | 2 | `B` | 三角形 (50, 0)・(650, 0)・(350, 700) | 700 |
//! | 3 | 空白 | なし | 300 |
//! | 4 | `O` | 長方形 50〜650 × 0〜700 の中に 200〜500 × 200〜500 の穴 | 700 |
//! | 5 | `Q` | 右上が 2 次の曲線の四角 | 700 |

/// テストのフォントの em（フォントの単位）。
pub const TEST_FONT_UNITS_PER_EM: u16 = 1000;

/// 字形（輪郭の列。点は (x, y, 曲線の上の点か)）。
type Contour = Vec<(i16, i16, bool)>;

fn rect(x0: i16, y0: i16, x1: i16, y1: i16, clockwise: bool) -> Contour {
    // TrueType の外側の輪郭は時計回り（y は上向き）。
    if clockwise {
        vec![
            (x0, y0, true),
            (x0, y1, true),
            (x1, y1, true),
            (x1, y0, true),
        ]
    } else {
        vec![
            (x0, y0, true),
            (x1, y0, true),
            (x1, y1, true),
            (x0, y1, true),
        ]
    }
}

fn glyphs() -> Vec<(Vec<Contour>, u16)> {
    vec![
        (
            vec![rect(50, 0, 450, 700, true), rect(100, 50, 400, 650, false)],
            500,
        ),
        (vec![rect(50, 0, 650, 700, true)], 700),
        (
            vec![vec![(50, 0, true), (350, 700, true), (650, 0, true)]],
            700,
        ),
        (Vec::new(), 300),
        (
            vec![rect(50, 0, 650, 700, true), rect(200, 200, 500, 500, false)],
            700,
        ),
        (
            vec![vec![
                (50, 0, true),
                (50, 700, true),
                (650, 700, false),
                (650, 0, true),
            ]],
            700,
        ),
    ]
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

fn glyph_bytes(contours: &[Contour]) -> Vec<u8> {
    if contours.is_empty() {
        return Vec::new();
    }
    let pts: Vec<&(i16, i16, bool)> = contours.iter().flatten().collect();
    let (x_min, x_max) = (
        pts.iter().map(|p| p.0).min().unwrap(),
        pts.iter().map(|p| p.0).max().unwrap(),
    );
    let (y_min, y_max) = (
        pts.iter().map(|p| p.1).min().unwrap(),
        pts.iter().map(|p| p.1).max().unwrap(),
    );
    let mut v = Vec::new();
    be16(&mut v, contours.len() as u16);
    for x in [x_min, y_min, x_max, y_max] {
        be16(&mut v, x as u16);
    }
    let mut end = 0u16;
    for c in contours {
        end += c.len() as u16;
        be16(&mut v, end - 1);
    }
    be16(&mut v, 0); // 命令の長さ
    for p in &pts {
        v.push(u8::from(p.2)); // 曲線の上の点（ON_CURVE_POINT）。座標は 16bit の差分。
    }
    let (mut px, mut py) = (0i16, 0i16);
    for p in &pts {
        be16(&mut v, (p.0 - px) as u16);
        px = p.0;
    }
    for p in &pts {
        be16(&mut v, (p.1 - py) as u16);
        py = p.1;
    }
    while v.len() % 4 != 0 {
        v.push(0);
    }
    v
}

/// テストのフォント（TrueType の 1 つのフォント）のバイト列。
pub fn test_font() -> Vec<u8> {
    let glyphs = glyphs();
    let n = glyphs.len() as u16;
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    let mut hmtx = Vec::new();
    for (contours, advance) in &glyphs {
        be32(&mut loca, glyf.len() as u32);
        glyf.extend(glyph_bytes(contours));
        be16(&mut hmtx, *advance);
        be16(&mut hmtx, 50);
    }
    be32(&mut loca, glyf.len() as u32);

    let mut head = Vec::new();
    be32(&mut head, 0x0001_0000); // 版 1.0
    be32(&mut head, 0x0001_0000); // フォントの版
    be32(&mut head, 0); // checksumAdjustment
    be32(&mut head, 0x5F0F_3CF5); // magicNumber
    be16(&mut head, 0); // flags
    be16(&mut head, TEST_FONT_UNITS_PER_EM);
    head.extend_from_slice(&[0; 16]); // created・modified
    for x in [50i16, 0, 650, 700] {
        be16(&mut head, x as u16);
    }
    be16(&mut head, 0); // macStyle
    be16(&mut head, 8); // lowestRecPPEM
    be16(&mut head, 2); // fontDirectionHint
    be16(&mut head, 1); // indexToLocFormat（長い形）
    be16(&mut head, 0); // glyphDataFormat

    let mut hhea = Vec::new();
    be32(&mut hhea, 0x0001_0000);
    for x in [800i16, -200, 0] {
        be16(&mut hhea, x as u16); // アセンダー・ディセンダー・行の間
    }
    be16(&mut hhea, 700); // advanceWidthMax
    for x in [50i16, 50, 650, 1, 0, 0, 0, 0, 0, 0, 0] {
        be16(&mut hhea, x as u16);
    }
    be16(&mut hhea, n); // numberOfHMetrics

    let mut maxp = Vec::new();
    be32(&mut maxp, 0x0000_5000);
    be16(&mut maxp, n);

    // cmap: Windows（3）・UCS-4（10）の形式 12。
    let groups: [(u32, u32); 6] = [
        (0x20, 3),
        (0x41, 1),
        (0x42, 2),
        (0x4F, 4),
        (0x51, 5),
        (0x3042, 1),
    ];
    let mut cmap = Vec::new();
    be16(&mut cmap, 0);
    be16(&mut cmap, 1);
    be16(&mut cmap, 3);
    be16(&mut cmap, 10);
    be32(&mut cmap, 12);
    be16(&mut cmap, 12);
    be16(&mut cmap, 0);
    be32(&mut cmap, 16 + 12 * groups.len() as u32);
    be32(&mut cmap, 0);
    be32(&mut cmap, groups.len() as u32);
    for (c, g) in groups {
        be32(&mut cmap, c);
        be32(&mut cmap, c);
        be32(&mut cmap, g);
    }

    // 表の並びは名前の順。
    let tables: [(&[u8; 4], Vec<u8>); 7] = [
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
    ];
    let mut out = Vec::new();
    be32(&mut out, 0x0001_0000);
    be16(&mut out, tables.len() as u16);
    be16(&mut out, 64); // searchRange（2^4 × 16。表は 7 つ）
    be16(&mut out, 2); // entrySelector
    be16(&mut out, 7 * 16 - 64); // rangeShift
    let mut offset = 12 + 16 * tables.len() as u32;
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        be32(&mut out, 0); // チェックサム（skrifa（read-fonts）は確かめない）
        be32(&mut out, offset);
        be32(&mut out, data.len() as u32);
        body.extend_from_slice(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + 16 * tables.len() as u32 + body.len() as u32;
    }
    out.extend(body);
    out
}

/// テストのフォントを `count` 個並べたコレクション（TrueType Collection。.ttc）。
pub fn test_font_collection(count: u32) -> Vec<u8> {
    let font = test_font();
    let header = 12 + 4 * count;
    let mut out = Vec::new();
    out.extend_from_slice(b"ttcf");
    be32(&mut out, 0x0001_0000);
    be32(&mut out, count);
    for i in 0..count {
        be32(&mut out, header + i * font.len() as u32);
    }
    for _ in 0..count {
        // フォントの中の表の位置はファイルの先頭からなので、置いた位置の分だけずらして書き直す。
        let base = out.len() as u32;
        let mut f = font.clone();
        let tables = u16::from_be_bytes([f[4], f[5]]) as usize;
        for t in 0..tables {
            let at = 12 + 16 * t + 8;
            let off = u32::from_be_bytes([f[at], f[at + 1], f[at + 2], f[at + 3]]) + base;
            f[at..at + 4].copy_from_slice(&off.to_be_bytes());
        }
        out.extend(f);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::outline::{DrawSettings, OutlinePen};
    use skrifa::prelude::{LocationRef, Size};
    use skrifa::raw::{CollectionRef, TableProvider};
    use skrifa::{FontRef, GlyphId, MetadataProvider};

    /// 輪郭の要素の数と、制御点を含む外接矩形を数えるペン。
    #[derive(Default)]
    struct Bounds {
        count: usize,
        rect: Option<[f32; 4]>,
    }

    impl Bounds {
        fn add(&mut self, x: f32, y: f32) {
            self.count += 1;
            let r = self.rect.get_or_insert([x, y, x, y]);
            *r = [r[0].min(x), r[1].min(y), r[2].max(x), r[3].max(y)];
        }
    }

    impl OutlinePen for Bounds {
        fn move_to(&mut self, x: f32, y: f32) {
            self.add(x, y);
        }
        fn line_to(&mut self, x: f32, y: f32) {
            self.add(x, y);
        }
        fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
            self.add(cx0, cy0);
            self.add(x, y);
        }
        fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
            self.add(cx0, cy0);
            self.add(cx1, cy1);
            self.add(x, y);
        }
        fn close(&mut self) {}
    }

    fn outline_of(font: &FontRef<'_>, glyph: GlyphId) -> Bounds {
        let mut pen = Bounds::default();
        font.outline_glyphs()
            .get(glyph)
            .unwrap()
            .draw(
                DrawSettings::unhinted(Size::unscaled(), LocationRef::default()),
                &mut pen,
            )
            .unwrap();
        pen
    }

    #[test]
    fn test_font_is_readable() {
        let data = test_font();
        let font = FontRef::new(&data).unwrap();
        assert_eq!(font.head().unwrap().units_per_em(), 1000);
        let m = font.metrics(Size::unscaled(), LocationRef::default());
        assert_eq!((m.ascent, m.descent, m.leading), (800.0, -200.0, 0.0));
        assert_eq!(font.maxp().unwrap().num_glyphs(), 6);
        let cmap = font.charmap();
        let a = cmap.map('A').unwrap();
        assert_eq!(cmap.map('あ'), Some(a));
        assert_eq!(cmap.map('漢'), None);
        let gm = font.glyph_metrics(Size::unscaled(), LocationRef::default());
        assert_eq!(gm.advance_width(a), Some(700.0));
        let r = outline_of(&font, a).rect.unwrap();
        assert_eq!(r, [50.0, 0.0, 650.0, 700.0]);
        // 空白は輪郭がない。
        assert_eq!(outline_of(&font, cmap.map(' ').unwrap()).count, 0);
        // 2 次の曲線（Q）の外接矩形は制御点を含む。
        let q = outline_of(&font, cmap.map('Q').unwrap()).rect.unwrap();
        assert_eq!(q, [50.0, 0.0, 650.0, 700.0]);
        let ttc = test_font_collection(2);
        assert_eq!(CollectionRef::new(&ttc).unwrap().len(), 2);
        for i in 0..2 {
            let f = FontRef::from_index(&ttc, i).unwrap();
            assert_eq!(f.charmap().map('B'), cmap.map('B'));
        }
        assert!(FontRef::from_index(&ttc, 2).is_err());
    }
}
