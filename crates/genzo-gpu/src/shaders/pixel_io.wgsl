// RGB の画像（f32 の R, G, B の並び。行優先）を読み書きするカーネルの共通部分。
//
// - binding 0: 入力のタイル（範囲は P_IN_X・P_IN_Y・P_IN_W・P_IN_H）。
// - binding 1: 出力のタイル（範囲は P_OUT_X・P_OUT_Y・P_OUT_W・P_OUT_H）。
// - 座標は各ステージの格子の全体での座標（genzo-pipeline の Roi と同じ）。

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;

// 出力のタイルの中か。
fn in_output(gid: vec3<u32>) -> bool {
    return gid.x < pu(P_OUT_W) && gid.y < pu(P_OUT_H);
}

// 出力の画素の全体での座標。
fn out_global(gid: vec3<u32>) -> vec2<i32> {
    return vec2<i32>(pi(P_OUT_X) + i32(gid.x), pi(P_OUT_Y) + i32(gid.y));
}

// 入力のタイルの中の番号 (lx, ly) の画素。
fn load_local(lx: u32, ly: u32) -> vec3<f32> {
    let i = (ly * pu(P_IN_W) + lx) * 3u;
    return vec3<f32>(src[i], src[i + 1u], src[i + 2u]);
}

// 全体での座標 g の画素（入力のタイルが含むこと）。
fn load_global(g: vec2<i32>) -> vec3<f32> {
    return load_local(u32(g.x - pi(P_IN_X)), u32(g.y - pi(P_IN_Y)));
}

// 出力のタイルの画素に書く。
fn store_out(gid: vec3<u32>, c: vec3<f32>) {
    let i = (gid.y * pu(P_OUT_W) + gid.x) * 3u;
    dst[i] = c.x;
    dst[i + 1u] = c.y;
    dst[i + 2u] = c.z;
}
