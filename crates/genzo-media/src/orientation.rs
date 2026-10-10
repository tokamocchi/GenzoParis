//! EXIF の向き（Orientation）を画素に反映する。
//!
//! PRV-01（04 の 4 章「埋め込み JPEG と RAW 以外の元の JPEG は、ICC プロファイルと向きを反映してから
//! B5 に変換して保存する」）と、RAW 以外の入力の読み込みで使う。
//!
//! EXIF の定義（Exif 2.32 の Orientation タグ）: 値は「記録された画像の 0 行目・0 列目が、表示した
//! ときのどの辺に当たるか」を表す。たとえば 6 は「0 行目が右辺、0 列目が上辺」で、表示するには
//! 時計回りに 90 度回転する。

use genzo_model::Orientation;

use crate::buffer::{DynRgbImage, RgbImage, Sample};

/// 向き `o` の画像（記録された寸法 `w × h`）を表示したときの画素 `(dx, dy)` が、記録された
/// 画像のどの画素 `(sx, sy)` に当たるか。
fn source_coord(o: Orientation, dx: u32, dy: u32, w: u32, h: u32) -> (u32, u32) {
    match o {
        Orientation::Normal => (dx, dy),
        Orientation::FlipHorizontal => (w - 1 - dx, dy),
        Orientation::Rotate180 => (w - 1 - dx, h - 1 - dy),
        Orientation::FlipVertical => (dx, h - 1 - dy),
        Orientation::Transpose => (dy, dx),
        Orientation::Rotate90Cw => (dy, h - 1 - dx),
        Orientation::Transverse => (w - 1 - dy, h - 1 - dx),
        Orientation::Rotate270Cw => (w - 1 - dy, dx),
    }
}

/// 向きを反映した（表示したときの向きの）画像を返す。
///
/// 90 度・270 度の回転を含む向き（5〜8）では幅と高さが入れ替わる。結果の向きは
/// [`Orientation::Normal`]（1）として扱う。
pub fn apply_orientation<T: Sample>(img: &RgbImage<T>, o: Orientation) -> RgbImage<T> {
    if o == Orientation::Normal {
        return img.clone();
    }
    let (w, h) = img.dimensions();
    let (dw, dh) = if o.swaps_dimensions() { (h, w) } else { (w, h) };
    let src = img.as_raw();
    let mut data = Vec::with_capacity(src.len());
    for dy in 0..dh {
        for dx in 0..dw {
            let (sx, sy) = source_coord(o, dx, dy, w, h);
            let i = (sy as usize * w as usize + sx as usize) * 3;
            data.extend_from_slice(&src[i..i + 3]);
        }
    }
    RgbImage::from_raw(dw, dh, data).expect("寸法を入れ替えただけなので画素数は同じ")
}

/// [`apply_orientation`] の 8bit / 16bit 共通版。
pub fn apply_orientation_dyn(img: &DynRgbImage, o: Orientation) -> DynRgbImage {
    match img {
        DynRgbImage::Rgb8(i) => DynRgbImage::Rgb8(apply_orientation(i, o)),
        DynRgbImage::Rgb16(i) => DynRgbImage::Rgb16(apply_orientation(i, o)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::RgbImage8;

    const ALL: [Orientation; 8] = [
        Orientation::Normal,
        Orientation::FlipHorizontal,
        Orientation::Rotate180,
        Orientation::FlipVertical,
        Orientation::Transpose,
        Orientation::Rotate90Cw,
        Orientation::Transverse,
        Orientation::Rotate270Cw,
    ];

    /// 3×2 の画像。画素 (x, y) の R = 10y + x。
    fn sample() -> RgbImage8 {
        RgbImage8::from_fn(3, 2, |x, y| [(10 * y + x) as u8, 0, 0]).unwrap()
    }

    fn reds(img: &RgbImage8) -> Vec<Vec<u8>> {
        (0..img.height())
            .map(|y| {
                (0..img.width())
                    .map(|x| img.pixel(x, y).unwrap()[0])
                    .collect()
            })
            .collect()
    }

    #[test]
    fn each_orientation_matches_hand_computed_result() {
        // 記録された画像:
        //  0  1  2
        // 10 11 12
        let img = sample();
        let expect: [(Orientation, Vec<Vec<u8>>); 8] = [
            (Orientation::Normal, vec![vec![0, 1, 2], vec![10, 11, 12]]),
            (
                Orientation::FlipHorizontal,
                vec![vec![2, 1, 0], vec![12, 11, 10]],
            ),
            (
                Orientation::Rotate180,
                vec![vec![12, 11, 10], vec![2, 1, 0]],
            ),
            (
                Orientation::FlipVertical,
                vec![vec![10, 11, 12], vec![0, 1, 2]],
            ),
            // 5: 0 行目が左辺、0 列目が上辺（転置）。
            (
                Orientation::Transpose,
                vec![vec![0, 10], vec![1, 11], vec![2, 12]],
            ),
            // 6: 時計回りに 90 度。0 行目が右辺に来る。
            (
                Orientation::Rotate90Cw,
                vec![vec![10, 0], vec![11, 1], vec![12, 2]],
            ),
            // 7: 0 行目が右辺、0 列目が下辺。
            (
                Orientation::Transverse,
                vec![vec![12, 2], vec![11, 1], vec![10, 0]],
            ),
            // 8: 反時計回りに 90 度。0 行目が左辺、0 列目が下辺。
            (
                Orientation::Rotate270Cw,
                vec![vec![2, 12], vec![1, 11], vec![0, 10]],
            ),
        ];
        for (o, e) in expect {
            let out = apply_orientation(&img, o);
            assert_eq!(reds(&out), e, "{o:?}");
        }
    }

    #[test]
    fn matches_image_crate_reference() {
        // 独立した実装（image crate の apply_orientation）と一致する。
        let img =
            RgbImage8::from_fn(5, 3, |x, y| [x as u8, y as u8, (x * 7 + y * 3) as u8]).unwrap();
        for o in ALL {
            let ours = apply_orientation(&img, o);
            let mut theirs = image::DynamicImage::ImageRgb8(
                image::RgbImage::from_raw(5, 3, img.as_raw().to_vec()).unwrap(),
            );
            theirs.apply_orientation(image::metadata::Orientation::from_exif(o.to_exif()).unwrap());
            let theirs = theirs.to_rgb8();
            assert_eq!(ours.dimensions(), theirs.dimensions(), "{o:?}");
            assert_eq!(ours.as_raw(), theirs.as_raw().as_slice(), "{o:?}");
        }
    }

    #[test]
    fn inverse_pairs_restore_the_original() {
        let img = RgbImage8::from_fn(4, 3, |x, y| [x as u8, y as u8, 1]).unwrap();
        let rot90 = apply_orientation(&img, Orientation::Rotate90Cw);
        assert_eq!(rot90.dimensions(), (3, 4));
        assert_eq!(apply_orientation(&rot90, Orientation::Rotate270Cw), img);
        for o in [
            Orientation::FlipHorizontal,
            Orientation::Rotate180,
            Orientation::FlipVertical,
            Orientation::Transpose,
            Orientation::Transverse,
        ] {
            // 自分自身が逆変換になる向き。
            assert_eq!(
                apply_orientation(&apply_orientation(&img, o), o),
                img,
                "{o:?}"
            );
        }
    }

    #[test]
    fn dyn_and_single_pixel() {
        let one = RgbImage8::from_raw(1, 1, vec![1, 2, 3]).unwrap();
        for o in ALL {
            assert_eq!(apply_orientation(&one, o), one);
        }
        let d = DynRgbImage::Rgb16(sample().to_rgb16());
        let r = apply_orientation_dyn(&d, Orientation::Rotate90Cw);
        assert_eq!(r.dimensions(), (2, 3));
        assert_eq!(r.bits_per_channel(), 16);
    }
}
