// Vite の設定（Tauri 2 の WebView で動かす画面）。
// - 開発サーバーは 5173 番（crates/genzo-app/tauri.conf.json の devUrl）。
// - ビルドの出力は ui/dist（genzo-app が埋め込む。frontendDist）。
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    // WebView2（Chromium 系）と macOS 12 以降の WKWebView（Safari 15 以降）。
    target: ["chrome110", "safari15"],
    sourcemap: false,
    // バンドルに入ったパッケージの一覧（名前・版・ライセンス）を JSON で書く。第三者のライセンスの文書
    // （crates/genzo-app/licenses/third-party/npm.txt）に、入ったものがすべて載っているかを
    // scripts/third-party.mjs --vite が確かめる（DIST-05。docs/third_party.md の 1.4 節）。
    license: { fileName: ".vite/license.json" },
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
