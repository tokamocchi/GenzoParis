// E2E の仕組みの確認（アプリが起動し、画面が出ること）。
import { writeFileSync } from "node:fs";
import { join } from "node:path";

import { WebDriver, sleep } from "./webdriver.mjs";

const wd = new WebDriver();
await wd.start(process.env.E2E_APP);
try {
  await wd.waitFor(async () => (await wd.exec("return document.body.innerText")).length > 0, {
    what: "画面の表示",
  });
  await sleep(500);
  console.log(await wd.exec("return document.body.innerText"));
  writeFileSync(join(process.env.E2E_OUT, "smoke.png"), await wd.screenshot());
} finally {
  await wd.quit();
}
