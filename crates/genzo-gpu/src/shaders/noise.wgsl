// 決定的なノイズ（genzo-pipeline の finish::output::hash32・dither_noise）。ステージ 17b のディザリング
// （quantize.wgsl）とステージ 16 の粒子（detail.wgsl）が使う。整数の演算だけなので CPU 版とビット単位で同じ。

// lowbias32（output::hash32。乗算はラップする）。
fn hash32(x0: u32) -> u32 {
    var x = x0;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    x = x ^ (x >> 16u);
    return x;
}

// 三角分布のノイズ（output::dither_noise）。
fn dither_noise(x: i32, y: i32, c: u32, seed: u32) -> f32 {
    let h = hash32(bitcast<u32>(x) ^ hash32(bitcast<u32>(y) ^ hash32(c ^ seed)));
    let u1 = f32(hash32(h) >> 8u) * DITHER_SCALE;
    let u2 = f32(hash32(h ^ 0x9e3779b9u) >> 8u) * DITHER_SCALE;
    return u1 + u2 - 1.0;
}
