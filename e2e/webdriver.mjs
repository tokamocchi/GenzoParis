// 最小の WebDriver（W3C）のクライアント（E2E 用。依存を足さず Node の fetch だけで書く）。
//
// tauri-driver（既定の http://127.0.0.1:4444）に接続し、WebKitWebDriver（Linux）経由で
// Tauri のアプリの WebView を操作する。

const ELEMENT = "element-6066-11e4-a52e-4f735466cecc";

export class WebDriver {
  constructor(base = "http://127.0.0.1:4444") {
    this.base = base;
    this.sessionId = null;
  }

  async req(method, path, body) {
    const r = await fetch(`${this.base}${path}`, {
      method,
      headers: { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await r.text();
    let json;
    try {
      json = JSON.parse(text);
    } catch {
      throw new Error(`WebDriver の応答を読めません（${r.status}）: ${text.slice(0, 300)}`);
    }
    if (!r.ok || (json.value && json.value.error)) {
      const v = json.value || {};
      const err = new Error(`${method} ${path}: ${v.error ?? r.status}: ${v.message ?? text.slice(0, 300)}`);
      err.webdriver = v.error;
      throw err;
    }
    return json.value;
  }

  s(path) {
    return `/session/${this.sessionId}${path}`;
  }

  async start(application, env = {}) {
    const v = await this.req("POST", "/session", {
      capabilities: {
        alwaysMatch: {
          "tauri:options": { application, env },
          "webkitgtk:browserOptions": { args: [] },
        },
      },
    });
    this.sessionId = v.sessionId;
    return v;
  }

  async quit() {
    if (!this.sessionId) return;
    try {
      await this.req("DELETE", this.s(""));
    } finally {
      this.sessionId = null;
    }
  }

  async find(using, value, timeoutMs = 10000) {
    const end = Date.now() + timeoutMs;
    let last;
    while (Date.now() < end) {
      try {
        const v = await this.req("POST", this.s("/element"), { using, value });
        return v[ELEMENT];
      } catch (e) {
        last = e;
        await sleep(150);
      }
    }
    throw last ?? new Error(`見つかりません: ${value}`);
  }

  async findAll(using, value) {
    const v = await this.req("POST", this.s("/elements"), { using, value });
    return v.map((e) => e[ELEMENT]);
  }

  /** 表示されている文字を含む要素（XPath）。 */
  byText(text, tag = "*") {
    return ["xpath", `//${tag}[contains(normalize-space(.), ${xpathString(text)})][not(.//${tag}[contains(normalize-space(.), ${xpathString(text)})])]`];
  }

  async click(el) {
    await this.req("POST", this.s(`/element/${el}/click`), {});
  }

  async type(el, text) {
    await this.req("POST", this.s(`/element/${el}/value`), { text });
  }

  async clear(el) {
    await this.req("POST", this.s(`/element/${el}/clear`), {});
  }

  async text(el) {
    return this.req("GET", this.s(`/element/${el}/text`));
  }

  async attr(el, name) {
    return this.req("GET", this.s(`/element/${el}/attribute/${name}`));
  }

  async exec(script, args = []) {
    return this.req("POST", this.s("/execute/sync"), { script, args });
  }

  async execAsync(script, args = []) {
    return this.req("POST", this.s("/execute/async"), { script, args });
  }

  /** キーを押す（W3C の actions。Ctrl などは ""）。 */
  async keys(...seq) {
    const actions = [];
    for (const k of seq) {
      actions.push({ type: "keyDown", value: k }, { type: "keyUp", value: k });
    }
    await this.req("POST", this.s("/actions"), {
      actions: [{ type: "key", id: "keyboard", actions }],
    });
  }

  /** 修飾キーを押しながら key を押す。 */
  async chord(modifiers, key) {
    const actions = [];
    for (const m of modifiers) actions.push({ type: "keyDown", value: m });
    actions.push({ type: "keyDown", value: key }, { type: "keyUp", value: key });
    for (const m of [...modifiers].reverse()) actions.push({ type: "keyUp", value: m });
    await this.req("POST", this.s("/actions"), {
      actions: [{ type: "key", id: "keyboard", actions }],
    });
  }

  async screenshot() {
    return Buffer.from(await this.req("GET", this.s("/screenshot")), "base64");
  }

  async waitFor(fn, { timeoutMs = 20000, intervalMs = 200, what = "条件" } = {}) {
    const end = Date.now() + timeoutMs;
    let lastErr;
    while (Date.now() < end) {
      try {
        const v = await fn();
        if (v) return v;
      } catch (e) {
        lastErr = e;
      }
      await sleep(intervalMs);
    }
    throw new Error(`${what}を待ちきれません${lastErr ? `（${lastErr.message}）` : ""}`);
  }
}

export const KEY = {
  CTRL: "",
  SHIFT: "",
  ALT: "",
  META: "",
  ENTER: "",
  ESC: "",
  LEFT: "",
  UP: "",
  RIGHT: "",
  DOWN: "",
  DELETE: "",
  BACKSPACE: "",
  TAB: "",
};

export function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

function xpathString(s) {
  if (!s.includes("'")) return `'${s}'`;
  if (!s.includes('"')) return `"${s}"`;
  return `concat('${s.split("'").join(`', "'", '`)}')`;
}
