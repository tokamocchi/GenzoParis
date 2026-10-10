// ステージ 17c: ヒストグラム（genzo-pipeline の finish::output::Histogram。DEV-26）。
//
// 度数は整数の原子的な加算なので、足す順序・分割によらず CPU 版と同じ（入力が同じなら）。ワーク
// グループごとに共有メモリで数えてから、全体のカウンターに足す。
//
// 出力（binding 1）の並び: [0, 256) R、[256, 512) G、[512, 768) B、[768, 1024) 輝度、
// [1024, 1029) 白飛び（R・G・B・どれか・すべて）、[1029, 1034) 黒つぶれ（同じ）。

@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read_write> hist: array<atomic<u32>>;

const HIST_LEN: u32 = 1034u;
var<workgroup> local_hist: array<atomic<u32>, 1034>;

// 符号化した値の階級（output::histogram_bin: clamp(round(v × 255), 0, 255)。NaN は 0）。
fn histogram_bin(v: f32) -> u32 {
    if (is_nan_f32(v)) {
        return 0u;
    }
    return u32(clamp(round_half_up(v * 255.0), 0.0, 255.0));
}

fn add_clip(base: u32, hit: vec3<bool>) {
    if (hit.x) {
        atomicAdd(&local_hist[base], 1u);
    }
    if (hit.y) {
        atomicAdd(&local_hist[base + 1u], 1u);
    }
    if (hit.z) {
        atomicAdd(&local_hist[base + 2u], 1u);
    }
    if (any(hit)) {
        atomicAdd(&local_hist[base + 3u], 1u);
    }
    if (all(hit)) {
        atomicAdd(&local_hist[base + 4u], 1u);
    }
}

@compute @workgroup_size(16, 16)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    for (var i = li; i < HIST_LEN; i = i + 256u) {
        atomicStore(&local_hist[i], 0u);
    }
    workgroupBarrier();
    if (gid.x < pu(P_OUT_W) && gid.y < pu(P_OUT_H)) {
        let i = (gid.y * pu(P_OUT_W) + gid.x) * 3u;
        let p = vec3<f32>(src[i], src[i + 1u], src[i + 2u]);
        let b = vec3<u32>(histogram_bin(p.x), histogram_bin(p.y), histogram_bin(p.z));
        atomicAdd(&local_hist[b.x], 1u);
        atomicAdd(&local_hist[256u + b.y], 1u);
        atomicAdd(&local_hist[512u + b.z], 1u);
        let y = pf(P_HLUMA) * p.x + pf(P_HLUMA + 1u) * p.y + pf(P_HLUMA + 2u) * p.z;
        atomicAdd(&local_hist[768u + histogram_bin(y)], 1u);
        add_clip(1024u, b == vec3<u32>(255u));
        add_clip(1029u, b == vec3<u32>(0u));
    }
    workgroupBarrier();
    for (var i = li; i < HIST_LEN; i = i + 256u) {
        let v = atomicLoad(&local_hist[i]);
        if (v > 0u) {
            atomicAdd(&hist[i], v);
        }
    }
}
