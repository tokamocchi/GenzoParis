// ビルドの対象（target triple）と、Tauri の externalBin のファイル名（純粋な処理）。

/**
 * `rustc -vV` の出力から、ホストの target triple（例: aarch64-apple-darwin）を取り出す。
 * （`rustc --print host-tuple` は Rust 1.84 以降。rust-toolchain.toml の版では使えるが、`-vV` は古い版でも使える）
 *
 * @param {string} text rustc -vV の標準出力
 * @returns {string}
 */
export function parseHostTriple(text) {
  const m = /^host:\s*(\S+)\s*$/m.exec(text);
  if (!m) throw new Error("rustc -vV の出力に host: の行がない");
  return m[1];
}

/**
 * Tauri の externalBin が探すファイル名（`<名前>-<target triple>`、Windows は `.exe` 付き）。
 * Tauri はバンドルするときに `-<target triple>` を除いた名前（genzo-worker / genzo-worker.exe）で置く。
 *
 * @param {string} name externalBin に書いた名前の最後の部分（例: genzo-worker）
 * @param {string} triple target triple
 */
export function sidecarFileName(name, triple) {
  return `${name}-${triple}${triple.includes("windows") ? ".exe" : ""}`;
}

/** target triple から OS の種類を返す（"windows" | "macos" | "linux" | "other"）。 */
export function targetOs(triple) {
  if (triple.includes("windows")) return "windows";
  if (triple.includes("apple-darwin")) return "macos";
  if (triple.includes("linux")) return "linux";
  return "other";
}
