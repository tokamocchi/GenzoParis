//! 推論に渡す 8 ビットの RGB の画像と、大きさの変更（双線形）。

/// 8 ビットの RGB の画像（行優先。1 画素 3 バイト）。推論の入力（sRGB の表示用の画像）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgb8Image {
    /// 幅（画素）。
    pub width: u32,
    /// 高さ（画素）。
    pub height: u32,
    /// 画素（`width × height × 3`）。
    pub data: Vec<u8>,
}

impl Rgb8Image {
    /// 作る（長さが合わなければ `None`）。
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && data.len() == width as usize * height as usize * 3).then_some(
            Self {
                width,
                height,
                data,
            },
        )
    }

    /// 画素 (x, y) の RGB。
    #[inline]
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.width as usize + x as usize) * 3;
        [self.data[i], self.data[i + 1], self.data[i + 2]]
    }

    /// 大きさを `w × h` に変える（双線形。縦横比は保たない。推論の入力の固定の大きさに合わせる）。
    pub fn resized(&self, w: u32, h: u32) -> Rgb8Image {
        if w == self.width && h == self.height {
            return self.clone();
        }
        let mut out = vec![0u8; w as usize * h as usize * 3];
        for y in 0..h {
            let (y0, y1, ty) = coords(y, h, self.height);
            for x in 0..w {
                let (x0, x1, tx) = coords(x, w, self.width);
                for c in 0..3 {
                    let at = |xx: u32, yy: u32| {
                        f32::from(
                            self.data[(yy as usize * self.width as usize + xx as usize) * 3 + c],
                        )
                    };
                    let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
                    let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
                    let v = top + (bottom - top) * ty;
                    out[(y as usize * w as usize + x as usize) * 3 + c] =
                        v.round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        Rgb8Image {
            width: w,
            height: h,
            data: out,
        }
    }
}

/// 出力の画素 `i`（`n` 個）に対する、入力（`m` 個）の 2 つの位置と補間の係数（画素の中心をそろえる）。
#[inline]
pub(crate) fn coords(i: u32, n: u32, m: u32) -> (u32, u32, f32) {
    let f = ((f64::from(i) + 0.5) * f64::from(m) / f64::from(n) - 0.5).max(0.0);
    let i0 = (f.floor() as u32).min(m - 1);
    let i1 = (i0 + 1).min(m - 1);
    (i0, i1, (f - f64::from(i0)) as f32)
}

/// 0〜1 の面（`w × h`）の大きさを `tw × th` に変える（双線形）。
pub(crate) fn resize_plane(src: &[f32], w: u32, h: u32, tw: u32, th: u32) -> Vec<f32> {
    if w == tw && h == th {
        return src.to_vec();
    }
    let mut out = vec![0.0f32; tw as usize * th as usize];
    for y in 0..th {
        let (y0, y1, ty) = coords(y, th, h);
        for x in 0..tw {
            let (x0, x1, tx) = coords(x, tw, w);
            let at = |xx: u32, yy: u32| src[yy as usize * w as usize + xx as usize];
            let top = at(x0, y0) + (at(x1, y0) - at(x0, y0)) * tx;
            let bottom = at(x0, y1) + (at(x1, y1) - at(x0, y1)) * tx;
            out[y as usize * tw as usize + x as usize] = top + (bottom - top) * ty;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_keeps_constant_images_and_interpolates_edges() {
        let img = Rgb8Image::new(4, 2, vec![100; 4 * 2 * 3]).unwrap();
        let r = img.resized(8, 6);
        assert_eq!((r.width, r.height), (8, 6));
        assert!(r.data.iter().all(|&v| v == 100));
        assert_eq!(img.resized(4, 2), img);
        // 左が 0、右が 255 の 2 画素: 4 画素にすると、間の 2 つは中間。
        let img = Rgb8Image::new(2, 1, vec![0, 0, 0, 255, 255, 255]).unwrap();
        let r = img.resized(4, 1);
        assert_eq!(r.pixel(0, 0), [0, 0, 0]);
        assert_eq!(r.pixel(3, 0), [255, 255, 255]);
        assert!(r.pixel(1, 0)[0] < r.pixel(2, 0)[0]);
        assert_eq!(resize_plane(&[0.0, 1.0], 2, 1, 3, 1), vec![0.0, 0.5, 1.0]);
        assert!(Rgb8Image::new(2, 2, vec![0; 5]).is_none());
    }
}
