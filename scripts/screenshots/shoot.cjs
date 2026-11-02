// README のスクリーンショット（docs/images/*.jpg）を撮る。
//
// 画面（ui/）を Vite の開発サーバーで、Tauri なしのモックの Bridge（ui/screenshot/main.tsx）で描き、Playwright の
// Chromium で操作して撮る。写真は gen_photos.py が作った合成の画像で、実在の写真・カタログは使わない。
// 見た目の確認用で、テストではない（アプリの動作・画質の確認の代わりにはならない）。
//
// 使い方（リポジトリのルートで。Python 3・numpy・Pillow・Playwright が要る）:
//   python3 scripts/screenshots/gen_photos.py <写真のフォルダ> 30
//   npm --prefix ui ci
//   NODE_PATH="$(npm root -g)" node scripts/screenshots/shoot.cjs <写真のフォルダ> [出力のフォルダ（既定 docs/images）]
//
// Playwright は、このリポジトリの依存には入れていない（手元か NODE_PATH で見える場所にあるものを使う）。

const { spawn } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

const ROOT = path.resolve(__dirname, "..", "..");
const PHOTOS = path.resolve(process.argv[2] ?? "");
const OUT = path.resolve(process.argv[3] ?? path.join(ROOT, "docs", "images"));
const PORT = 5174;
const BASE = `http://localhost:${PORT}/screenshot/index.html`;

if (!process.argv[2] || !fs.existsSync(path.join(PHOTOS, "1.jpg"))) {
  console.error("使い方: node scripts/screenshots/shoot.cjs <gen_photos.py の出力のフォルダ> [出力のフォルダ]");
  process.exit(2);
}

async function startVite() {
  // npx を通すと、止めたときに Vite の本体（孫のプロセス）が残るため、node で直接起動する。
  const bin = path.join(ROOT, "ui", "node_modules", "vite", "bin", "vite.js");
  const vite = spawn(process.execPath, [bin, "--port", String(PORT), "--strictPort"], {
    cwd: path.join(ROOT, "ui"),
    stdio: ["ignore", "pipe", "inherit"],
  });
  await new Promise((resolve, reject) => {
    vite.on("exit", (code) => reject(new Error(`Vite が終了しました（${code}）`)));
    vite.stdout.on("data", (d) => {
      if (String(d).includes("ready")) resolve();
    });
  });
  return vite;
}

async function main() {
  fs.mkdirSync(OUT, { recursive: true });
  const vite = await startVite();
  const browser = await chromium.launch();
  try {
    const ctx = await browser.newContext({
      viewport: { width: 1600, height: 1000 },
      deviceScaleFactor: 1,
      locale: "ja-JP",
      colorScheme: "dark",
      // Windows の形の URI（http://genzo.localhost/...。ui/src/api/uri.ts）にして、page.route で応える。
      userAgent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36",
    });
    const page = await ctx.newPage();
    page.on("pageerror", (e) => console.log("画面のエラー:", e.message));
    await page.route("http://genzo.localhost/**", (route) => {
      const m = new URL(route.request().url()).pathname.match(/^\/(thumb|preview)\/(\d+)/);
      if (!m) return route.fulfill({ status: 404, body: "" });
      const file = path.join(PHOTOS, `${m[1] === "thumb" ? "t" : ""}${m[2]}.jpg`);
      return route.fulfill({
        status: 200,
        contentType: "image/jpeg",
        body: fs.readFileSync(file),
        headers: { "access-control-allow-origin": "*" },
      });
    });

    const shot = async (name) => {
      await page.waitForTimeout(1500);
      await page.screenshot({ path: path.join(OUT, `${name}.jpg`), type: "jpeg", quality: 92 });
      console.log("撮影:", name);
    };
    const open = async (query = "") => {
      await page.goto(BASE + query);
      await page.locator(".lib-cell-thumb").nth(29).waitFor();
      await page.waitForTimeout(1000);
    };
    const cell = (i) => page.locator(".lib-cell-thumb").nth(i);

    // ライブラリ（グリッド）
    await open();
    await cell(1).click();
    await shot("library");

    // 現像（基本補正）
    await page.keyboard.press("d");
    await shot("develop");

    // 比較表示（選択と候補の 2 枚）
    await page.keyboard.press("g");
    await page.waitForTimeout(800);
    await cell(6).click();
    await cell(9).click({ modifiers: ["Control"] });
    await page.keyboard.press("c");
    await shot("compare");

    // 選別表示（複数枚を並べる）
    await page.keyboard.press("g");
    await page.waitForTimeout(800);
    for (const [k, i] of [0, 1, 6, 9].entries()) await cell(i).click({ modifiers: k ? ["Control"] : [] });
    await page.keyboard.press("n");
    await shot("survey");

    // 書き出し・取り込みのダイアログ
    await page.keyboard.press("g");
    await page.waitForTimeout(800);
    await page.keyboard.press("Control+Shift+E");
    await shot("export");
    await page.keyboard.press("Escape");
    await page.waitForTimeout(500);
    await page.keyboard.press("Control+Shift+I");
    await shot("import");

    // 現像（トーンカーブ・カラーグレーディング）
    await open("?sections=develop.basic=0,develop.histogram=1,develop.curve=1,develop.grading=1,develop.mixer=0,develop.detail=0");
    await cell(5).click();
    await page.keyboard.press("d");
    await page.waitForTimeout(1000);
    await page.evaluate(() => {
      const head = [...document.querySelectorAll(".section-head")].find((e) => e.textContent.includes("トーンカーブ"));
      head?.scrollIntoView({ block: "start" });
    });
    await shot("develop-color");

    const unknown = await page.evaluate(() => [...new Set(window.__unknown)]);
    if (unknown.length > 0) console.log("モックが応えなかったコマンド:", unknown.join(", "));
  } finally {
    await browser.close();
    vite.kill();
  }
}

main().then(
  () => process.exit(0),
  (e) => {
    console.error(e);
    process.exit(1);
  },
);
