// センサー処理のステージ 2・3・5（2 × 2 の簡易処理）（genzo-pipeline の sensor::normalize・
// sensor::wb・sensor::demosaic::half。docs/04_architecture.md の 2.1 節・2.2 節・2.6 節の B0・B1）。
//
// - CFA の色の番号（R = 0, G1 = 1, B = 2, G2 = 3。genzo-raw の CfaColor::index）は、全体での座標の
//   偶奇で引く表（P_CFA の 4 つ。番号 = P_CFA[(y の偶奇) × 2 + (x の偶奇)]）。鏡映は偶奇を保つので、
//   画像の外にはみ出した座標でもそのまま引ける。
// - binding 0・1 の型はカーネルごとに違う（RAW は u16 を 2 つずつ詰めた u32、CFA は f32 が 1 画素
//   1 個、RGB は f32 が 1 画素 3 個）。

@group(0) @binding(0) var<storage, read> src_words: array<u32>;
@group(0) @binding(1) var<storage, read_write> dst_f32: array<f32>;
@group(0) @binding(4) var<storage, read> aux_words: array<u32>;
@group(0) @binding(5) var<storage, read_write> aux_out: array<u32>;

fn cfa_index(x: i32, y: i32) -> u32 {
    return pu(P_CFA + u32(((y & 1) << 1u) | (x & 1)));
}

fn out_index(gid: vec3<u32>) -> u32 {
    return gid.y * pu(P_OUT_W) + gid.x;
}

fn sensor_in_output(gid: vec3<u32>) -> bool {
    return gid.x < pu(P_OUT_W) && gid.y < pu(P_OUT_H);
}

// ---------------------------------------------------------------------------
// ステージ 2: 正規化（sensor::normalize）。入力は RAW 全体、出力は CFA（f32）と飽和の印（aux_out）。
// ---------------------------------------------------------------------------

fn raw_at(sx: u32, sy: u32) -> u32 {
    let i = sy * pu(P_RAW_W) + sx;
    let w = src_words[i >> 1u];
    return (w >> ((i & 1u) * 16u)) & 0xffffu;
}

@compute @workgroup_size(8, 8)
fn normalize_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!sensor_in_output(gid)) {
        return;
    }
    let gx = pi(P_OUT_X) + i32(gid.x);
    let gy = pi(P_OUT_Y) + i32(gid.y);
    let sx = mirror_index(gx, pu(P_RAW_W));
    let sy = mirror_index(gy, pu(P_RAW_H));
    let k = cfa_index(i32(sx), i32(sy));
    let d = f32(raw_at(sx, sy));
    var v = 1.0;
    var s = 1u;
    if (d < pf(P_WHITE)) {
        v = clamp((d - pf(P_BLACK + k)) * pf(P_INV + k), 0.0, 1.0);
        s = 0u;
    }
    let o = out_index(gid);
    dst_f32[o] = sanitize1(v);
    aux_out[o] = s;
}

// ---------------------------------------------------------------------------
// ステージ 3: WB と飽和の切りそろえ（sensor::wb::apply_white_balance）。入力は CFA（f32、binding 0 を
// f32 として読む）と飽和の印（aux_words）。
// ---------------------------------------------------------------------------

fn cfa_in_value(i: u32) -> f32 {
    return bitcast<f32>(src_words[i]);
}

// 入力のタイルの (lx, ly) の近傍（チェビシェフ距離 r。タイルの中だけ）に飽和した画素があるか
// （wb::dilate と同じ範囲）。
fn near_saturated(lx: i32, ly: i32, r: i32) -> bool {
    let w = i32(pu(P_IN_W));
    let h = i32(pu(P_IN_H));
    let x0 = max(lx - r, 0);
    let x1 = min(lx + r + 1, w);
    let y0 = max(ly - r, 0);
    let y1 = min(ly + r + 1, h);
    for (var y = y0; y < y1; y = y + 1) {
        for (var x = x0; x < x1; x = x + 1) {
            if (aux_words[u32(y * w + x)] != 0u) {
                return true;
            }
        }
    }
    return false;
}

@compute @workgroup_size(8, 8)
fn white_balance_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!sensor_in_output(gid)) {
        return;
    }
    let gx = pi(P_OUT_X) + i32(gid.x);
    let gy = pi(P_OUT_Y) + i32(gid.y);
    let lx = gx - pi(P_IN_X);
    let ly = gy - pi(P_IN_Y);
    let li = u32(ly) * pu(P_IN_W) + u32(lx);
    let k = cfa_index(gx, gy);
    let x = cfa_in_value(li) * pf(P_MULT + k);
    let mode = pu(P_CLIP_MODE);
    var clipped = mode == 2u;
    if (mode == 1u) {
        clipped = near_saturated(lx, ly, pi(P_RADIUS));
    }
    var v = x;
    if (clipped) {
        v = min(x, pf(P_CLIP_LEVEL));
    }
    let o = out_index(gid);
    dst_f32[o] = sanitize1(v);
    if (pu(P_FLAGS_OUT) != 0u) {
        aux_out[o] = aux_words[li];
    }
}

// ---------------------------------------------------------------------------
// ステージ 5: 2 × 2 の簡易処理（sensor::demosaic::half）。出力の画素 (bx, by)（半分の格子）は S の
// [2bx, 2bx + 2) × [2by, 2by + 2) から作る: R = R、G = (G1 + G2) × 0.5、B = B。
// ---------------------------------------------------------------------------

@compute @workgroup_size(8, 8)
fn half_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (!sensor_in_output(gid)) {
        return;
    }
    let bx = pi(P_OUT_X) + i32(gid.x);
    let by = pi(P_OUT_Y) + i32(gid.y);
    var v = array<f32, 4>(0.0, 0.0, 0.0, 0.0);
    for (var dy = 0; dy < 2; dy = dy + 1) {
        for (var dx = 0; dx < 2; dx = dx + 1) {
            let gx = 2 * bx + dx;
            let gy = 2 * by + dy;
            let li = u32(gy - pi(P_IN_Y)) * pu(P_IN_W) + u32(gx - pi(P_IN_X));
            v[cfa_index(gx, gy)] = cfa_in_value(li);
        }
    }
    let rgb = sanitize3(vec3<f32>(v[0], (v[1] + v[3]) * 0.5, v[2]));
    let o = out_index(gid) * 3u;
    dst_f32[o] = rgb.x;
    dst_f32[o + 1u] = rgb.y;
    dst_f32[o + 2u] = rgb.z;
}
