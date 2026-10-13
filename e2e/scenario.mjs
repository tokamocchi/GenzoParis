// MVP のユーザーストーリーに沿った E2E（Linux の開発環境。e2e/run.sh から起動する）。
//
// 写真のフォルダは環境変数 E2E_PHOTOS（`cargo run -p genzo-testkit --example e2e_fixtures -- <フォルダ>`
// で作り、動画は ffmpeg で足す。README の「E2E」）。画面の操作の結果を、画面とコアの状態の両方で確かめ、
// 各段階の画面を E2E_OUT に保存する。
//
// OS のダイアログ（フォルダの選択）は WebDriver で操作できないため、取り込みはコアのコマンドで行い、
// 書き出し先は書き出しのダイアログが覚える前回の書き出し先（localStorage）に入れておく。

import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

import { allItems, assert, core, invoke, screenshot, step, waitJob } from "./helpers.mjs";
import { KEY, WebDriver, sleep } from "./webdriver.mjs";

const PHOTOS = process.env.E2E_PHOTOS;
if (!PHOTOS || !existsSync(PHOTOS)) throw new Error("E2E_PHOTOS に写真のフォルダを指定してください");
const WORK = process.env.E2E_WORK;
const EXPORT_DIR = join(WORK, "書き出し");
mkdirSync(EXPORT_DIR, { recursive: true });

/** フォルダの中のファイルの SHA-256（元ファイルを変えていないことの確認。DATA-01）。 */
function hashes(dir) {
  const out = {};
  for (const name of readdirSync(dir).sort()) {
    const p = join(dir, name);
    if (statSync(p).isFile()) out[name] = createHash("sha256").update(readFileSync(p)).digest("hex");
  }
  return out;
}

const before = hashes(PHOTOS);
const photoCount = Object.keys(before).length;

const wd = new WebDriver();
await wd.start(process.env.E2E_APP);
const results = [];
let failed = false;

async function run(title, fn) {
  try {
    await step(wd, title, fn);
    results.push({ title, ok: true });
  } catch (e) {
    results.push({ title, ok: false, error: e.message });
    failed = true;
  }
}

/** 文字を含むボタンを押す。 */
async function clickButton(text) {
  const el = await wd.find(...wd.byText(text, "button"));
  await wd.click(el);
}

/** 画面の文字に `text` が出るまで待つ。 */
async function waitText(text, timeoutMs = 20000) {
  await wd.waitFor(async () => (await wd.exec("return document.body.innerText")).includes(text), {
    timeoutMs,
    what: `「${text}」の表示`,
  });
}

/** グリッドのセルの数。 */
async function gridCells() {
  return wd.exec("return document.querySelectorAll('[data-index]').length");
}

/** 現像中の写真の状態（コア）。 */
async function developState() {
  const r = await core(wd, { type: "develop_state" });
  return r.state;
}

/** 名前に `part` を含むセルを押す。 */
async function clickCell(part) {
  const idx = await wd.exec(
    `const cells = [...document.querySelectorAll('[data-index]')];
     const c = cells.find((c) => (c.textContent || '').includes(arguments[0]) || (c.getAttribute('title') || '').includes(arguments[0]));
     if (!c) return null;
     c.scrollIntoView({ block: 'center' });
     return c.getAttribute('data-index');`,
    [part],
  );
  assert(idx !== null, `「${part}」のセルがある`);
  const el = await wd.find("css selector", `[data-index="${idx}"]`);
  await wd.click(el);
  return Number(idx);
}

try {
  await run("1. 最初の画面で既定の場所にカタログを作る", async () => {
    await waitText("カタログを選んでください");
    await screenshot(wd, "welcome");
    await clickButton("作って開く");
    await waitText("グリッド");
    const st = await invoke(wd, "app_status");
    assert(st.catalog && st.catalog.catalog_path.endsWith("catalog.db"), "既定の catalog.db を開いた");
    assert(st.catalog.capabilities?.raw_decoder, `RAW のデコーダがある（${JSON.stringify(st.catalog.worker_error)}）`);
    // ログをアプリのデータのフォルダのファイルに残している（SYS-07。使い捨ての HOME の下）。
    assert(st.logs?.file?.endsWith("app.log"), `ログのファイルがある（${JSON.stringify(st.logs)}）`);
    const logs = await invoke(wd, "log_folder_info");
    assert(logs.files.some((f) => f.name === "app.log" && f.bytes > 0), "app.log に書いている");
  });

  await run("2. フォルダを取り込み、グリッドに並ぶ（IMP-01・VID-01・LIB-01）", async () => {
    const r = await core(wd, { type: "import_folder", path: PHOTOS, recursive: true });
    const job = await waitJob(wd, r.job_id);
    assert(job.state === "succeeded", `取り込みが成功（${JSON.stringify(job.error)}）`);
    await wd.waitFor(async () => (await gridCells()) === photoCount, { what: `${photoCount} 件のセル` });
    // サムネイルが読み込まれるまで待つ（動画は ffmpeg が要る）。
    await wd.waitFor(
      async () =>
        (await wd.exec(
          "return [...document.querySelectorAll('[data-index] img')].filter((i) => i.complete && i.naturalWidth > 0).length",
        )) >= photoCount - 1,
      { what: "サムネイルの表示", timeoutMs: 30000 },
    );
    await screenshot(wd, "grid");
  });

  await run("3. 同じフォルダを取り込み直しても増えない", async () => {
    const r = await core(wd, { type: "import_folder", path: PHOTOS, recursive: true });
    await waitJob(wd, r.job_id);
    await sleep(500);
    const items = await allItems(wd);
    assert(items.length === photoCount, `件数は ${photoCount} のまま（${items.length}）`);
  });

  await run("4. キーボードで評価・フラグを付ける（LIB-04・05）", async () => {
    await clickCell("グラデーション 01");
    await wd.keys("3");
    await wd.keys("p");
    await wd.keys("6");
    await wd.waitFor(
      async () => {
        const v = (await allItems(wd)).find((i) => i.file_name.startsWith("グラデーション 01"));
        return v && v.rating === 3 && v.flag === 1 && v.color_label === "red";
      },
      { what: "評価 3・採用・赤のラベル" },
    );
    await screenshot(wd, "marked");
  });

  await run("5. 評価で絞り込み、解除する（LIB-07）", async () => {
    const sel = await wd.find("css selector", 'select[aria-label="評価"]');
    const opt = await wd.exec(
      "const s = arguments[0]; const o = [...s.options].find((o) => o.textContent.includes('3')); return o ? o.value : null;",
      [{ "element-6066-11e4-a52e-4f735466cecf": sel }],
    );
    assert(opt !== null, "★3 の選択肢がある");
    const option = await wd.find("css selector", `select[aria-label="評価"] option[value="${opt}"]`);
    await wd.click(option);
    await wd.waitFor(async () => (await gridCells()) === 1, { what: "1 件に絞り込み" });
    await screenshot(wd, "filtered");
    const all = await wd.find("css selector", 'select[aria-label="評価"] option[value="0"]');
    await wd.click(all);
    await wd.waitFor(async () => (await gridCells()) === photoCount, { what: "解除して全件" });
  });

  await run("6. ルーペで表示し、前後に送る（LIB-02）", async () => {
    await clickCell("グラデーション 01");
    await wd.keys("e");
    await wd.waitFor(
      async () => wd.exec("return [...document.querySelectorAll('img')].some((i) => i.complete && i.naturalWidth > 600)"),
      { what: "ルーペの画像", timeoutMs: 30000 },
    );
    await screenshot(wd, "loupe");
    await wd.keys(KEY.RIGHT);
    await sleep(500);
    await screenshot(wd, "loupe-next");
  });

  await run("7. RAW を現像の画面で開く（DEV-01・SYS-03）", async () => {
    await wd.keys("g");
    await clickCell("チャート 01");
    await wd.keys("d");
    await wd.waitFor(
      async () => {
        const s = await developState();
        return s && s.source.is_raw;
      },
      { what: "RAW を開く", timeoutMs: 60000 },
    );
    await wd.waitFor(
      async () => wd.exec("return [...document.querySelectorAll('canvas')].some((c) => c.width > 1000)"),
      { what: "プレビューの描画", timeoutMs: 60000 },
    );
    await sleep(500);
    await screenshot(wd, "develop");
  });

  await run("8. 露光量を入力で変える（DEV-04）", async () => {
    const box = await wd.exec(
      `const label = [...document.querySelectorAll('.slider-label')].find((l) => l.textContent.trim() === '露光量');
       return label ? label.parentElement.querySelector('input.slider-value') : null;`,
    );
    assert(box, "露光量の入力欄");
    const el = box["element-6066-11e4-a52e-4f735466cecf"];
    await wd.click(el);
    await wd.exec("arguments[0].select()", [box]);
    await wd.type(el, "1.25");
    await wd.keys(KEY.ENTER);
    await wd.waitFor(async () => Math.abs((await developState()).settings.exposure_ev - 1.25) < 1e-6, {
      what: "露光量 +1.25",
    });
    await sleep(1500);
    await screenshot(wd, "develop-exposure");
  });

  await run("9. スポイトで灰色のパッチを中立にする（DEV-03）", async () => {
    // チャートの 4 段目の 4 列目（Neutral 5）の中央。e2e_fixtures のチャートは 2200×1480、パッチ 320・余白 40。
    const fx = (40 + 3 * 360 + 160) / 2200;
    const fy = (40 + 3 * 360 + 160) / 1480;
    await wd.keys("w");
    const rect = await wd.exec(
      `const c = [...document.querySelectorAll('canvas')].filter((c) => c.width > 1000)
          .sort((a, b) => b.getBoundingClientRect().width - a.getBoundingClientRect().width)[0];
       const r = c.getBoundingClientRect();
       return { x: r.x, y: r.y, w: r.width, h: r.height };`,
    );
    const x = Math.round(rect.x + rect.w * fx);
    const y = Math.round(rect.y + rect.h * fy);
    await wd.req("POST", wd.s("/actions"), {
      actions: [
        {
          type: "pointer",
          id: "mouse",
          parameters: { pointerType: "mouse" },
          actions: [
            { type: "pointerMove", x, y, origin: "viewport" },
            { type: "pointerDown", button: 0 },
            { type: "pointerUp", button: 0 },
          ],
        },
      ],
    });
    await wd.waitFor(async () => typeof (await developState()).settings.white_balance === "object", {
      what: "カスタムの WB",
    });
    const s = await developState();
    const wb = s.settings.white_balance.custom;
    const asShot = s.source.as_shot_white_balance;
    console.log(`  スポイト: ${JSON.stringify(wb)}、撮影時: ${JSON.stringify(asShot)}`);
    assert(
      asShot && Math.abs(wb.temperature_k - asShot.temperature_k) < 150 && Math.abs(wb.tint - asShot.tint) < 5,
      "灰色のパッチは撮影時の WB でほぼ中立なので、撮影時の値に近い",
    );
    await sleep(1500);
    await screenshot(wd, "develop-wb");
  });

  await run("10. 取り消す・やり直す（DEV-27）", async () => {
    await sleep(1200); // 自動保存（1 秒）を待つ。
    await wd.chord([KEY.CTRL], "z");
    await wd.waitFor(async () => (await developState()).settings.white_balance === "as_shot", { what: "WB が撮影時に戻る" });
    await wd.chord([KEY.CTRL, KEY.SHIFT], "z");
    await wd.waitFor(async () => typeof (await developState()).settings.white_balance === "object", { what: "やり直し" });
  });

  await run("11. 切り抜き（1:1）を編集して確定する（DEV-15）", async () => {
    // R → 枠 → 縦横比 → Enter で確定し、Esc で取り消せることを確かめる（縦横比の選択肢は、編集の外で選ぶと
    // その場で保存するので、必ず編集を始めてから選ぶ）。
    const ratio = (g) => ((g.crop.right - g.crop.left) * 2200) / ((g.crop.bottom - g.crop.top) * 1480);
    const blur = () => wd.exec("if (document.activeElement) document.activeElement.blur();");
    const cropShown = () => wd.exec("return !!document.querySelector('svg.dv-crop');");
    const chooseAspect = async (label) => {
      const value = await wd.exec(
        "const s = document.querySelector('select[aria-label=\"縦横比\"]'); const o = [...s.options].find((o) => o.textContent.replace(/\\s/g, '') === arguments[0]); return o ? o.value : null;",
        [label],
      );
      assert(value !== null, `${label} の選択肢`);
      await wd.click(await wd.find("css selector", `select[aria-label="縦横比"] option[value="${value}"]`));
    };
    const before = (await developState()).settings.geometry;

    await blur();
    await wd.keys("r");
    await wd.waitFor(cropShown, { what: "切り抜きの枠（R）" });
    await chooseAspect("1:1");
    await sleep(800);
    assert(
      JSON.stringify((await developState()).settings.geometry.crop) === JSON.stringify(before.crop),
      "確定する前は切り抜きを保存しない",
    );
    await screenshot(wd, "develop-crop");
    await blur();
    await wd.keys(KEY.ENTER);
    await wd.waitFor(async () => Math.abs(ratio((await developState()).settings.geometry) - 1) < 0.02, {
      what: "1:1 の切り抜き（Enter で確定）",
    });
    await wd.waitFor(async () => !(await cropShown()), { what: "枠が閉じる" });
    const committed = (await developState()).settings.geometry;

    // Esc: 編集の途中の変更を捨てる。
    await blur();
    await wd.keys("r");
    await wd.waitFor(cropShown, { what: "切り抜きの枠（2 回目）" });
    await chooseAspect("16:9");
    await sleep(500);
    await blur();
    await wd.keys(KEY.ESC);
    await wd.waitFor(async () => !(await cropShown()), { what: "枠が閉じる（Esc）" });
    await sleep(1200);
    const after = (await developState()).settings.geometry;
    assert(JSON.stringify(after.crop) === JSON.stringify(committed.crop), "Esc で取り消すと切り抜きは変わらない");
    await screenshot(wd, "develop-cropped");
  });

  await run("12. 書き出す（EXP-01・EXP-04。GPS の削除は既定でオン）", async () => {
    await wd.keys("g");
    await wd.chord([KEY.CTRL], "a");
    await wd.exec(
      `localStorage.setItem('genzo.dialogs.export.last', JSON.stringify({
         v: 1,
         defaultColorSpace: 'srgb',
         form: { format: 'jpeg', quality: '90', colorSpace: 'srgb', sizeMode: 'long_edge', longEdge: '1024',
                 removeGps: true, onConflict: 'sequence', destDir: arguments[0] },
       }));`,
      [EXPORT_DIR],
    );
    await wd.chord([KEY.CTRL, KEY.SHIFT], "e");
    await waitText("書き出し先");
    await screenshot(wd, "export-dialog");
    await clickButton("書き出す");
    await wd.waitFor(() => readdirSync(EXPORT_DIR).filter((n) => n.endsWith(".jpg")).length === photoCount - 1, {
      what: `写真 ${photoCount - 1} 枚の JPEG（動画は書き出さない）`,
      timeoutMs: 120000,
    });
    console.log(`  書き出し: ${readdirSync(EXPORT_DIR).join(", ")}`);
  });

  await run("13. 今すぐバックアップする（SYS-04）", async () => {
    await wd.chord([KEY.CTRL], ",");
    await clickButton("バックアップ");
    await clickButton("今すぐバックアップ");
    await wd.waitFor(async () => (await core(wd, { type: "list_backups" })).backups.length >= 1, { what: "バックアップ" });
    await screenshot(wd, "settings-backup");
    await wd.keys(KEY.ESC);
  });

  await run("14. カタログから除去する（FILE-01）", async () => {
    await clickCell("色の帯");
    await wd.keys(KEY.DELETE);
    await waitText("カタログから除去");
    await screenshot(wd, "delete-dialog");
    const btn = await wd.find(
      "xpath",
      "//div[contains(@class,'dialog-actions')]//button[contains(normalize-space(.), 'カタログから除去')]",
    );
    await wd.click(btn);
    await wd.waitFor(async () => (await allItems(wd)).length === photoCount - 1, { what: "1 件の除去" });
  });

  await run("15. 元のファイルを変えていない（DATA-01）", async () => {
    const after = hashes(PHOTOS);
    assert(JSON.stringify(after) === JSON.stringify(before), "取り込み・現像・書き出し・除去の前後でハッシュが同じ");
  });

  await run("16. アプリを終了する（カタログを閉じてから）", async () => {
    await screenshot(wd, "before-quit");
    await invoke(wd, "quit_app", { discardUnsaved: false }).catch(() => undefined);
  });
} finally {
  try {
    await wd.quit();
  } catch {
    // アプリが終わっていればセッションも終わっている。
  }
}

console.log("\n結果:");
for (const r of results) console.log(`${r.ok ? "OK" : "NG"}  ${r.title}${r.ok ? "" : `: ${r.error}`}`);
process.exit(failed ? 1 : 0);
