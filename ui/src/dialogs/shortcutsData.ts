// ショートカットの一覧（UX-02）の内容。登録は App.tsx（全体）・Grid（グリッドの移動）・現像の画面が持つ。
// ここは表示用の写しなので、キーを変えたらここも直す。

export interface ShortcutItem {
  /** keyboard.ts の書き方（"mod+shift+c" など）。表示は comboLabel で作る。 */
  keys: string[];
  label: string;
}

export interface ShortcutGroup {
  title: string;
  items: ShortcutItem[];
}

export const SHORTCUT_GROUPS: ShortcutGroup[] = [
  {
    title: "表示",
    items: [
      { keys: ["g"], label: "グリッド" },
      { keys: ["e"], label: "ルーペ" },
      { keys: ["d"], label: "現像" },
      { keys: ["Escape"], label: "グリッドに戻る" },
      { keys: ["ArrowLeft", "ArrowRight"], label: "前・次の写真（ルーペ・現像）" },
    ],
  },
  {
    title: "選別",
    items: [
      { keys: ["0", "1", "2", "3", "4", "5"], label: "評価（★0〜5）" },
      { keys: ["p"], label: "採用" },
      { keys: ["x"], label: "不採用" },
      { keys: ["u"], label: "フラグを外す" },
      { keys: ["6", "7", "8", "9"], label: "カラーラベル（赤・黄・緑・青）" },
    ],
  },
  {
    title: "選択（グリッド）",
    items: [
      { keys: ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"], label: "選択を移す" },
      { keys: ["Home", "End"], label: "先頭・最後" },
      { keys: ["PageUp", "PageDown"], label: "1 画面ぶん移す" },
      { keys: ["mod+a"], label: "すべて選ぶ（先頭の 10,000 件まで）" },
      { keys: ["mod+d"], label: "選択を解除" },
    ],
  },
  {
    title: "現像",
    items: [
      { keys: ["w"], label: "ホワイトバランスのスポイト" },
      { keys: ["r"], label: "切り抜き" },
      { keys: ["Enter"], label: "切り抜きを確定" },
      { keys: ["Escape"], label: "切り抜きを取り消す" },
      { keys: ["z"], label: "等倍表示（ルーペ・現像）" },
      { keys: ["j"], label: "白飛び・黒つぶれの表示" },
      { keys: ["mod+z"], label: "取り消す" },
      { keys: ["mod+shift+z", "mod+y"], label: "やり直す" },
      { keys: ["mod+shift+c"], label: "現像設定をコピー" },
      { keys: ["mod+shift+v"], label: "現像設定を貼り付け（選んだ写真へ）" },
    ],
  },
  {
    title: "ファイル・そのほか",
    items: [
      { keys: ["mod+shift+i"], label: "取り込み" },
      { keys: ["mod+shift+e"], label: "書き出し" },
      { keys: ["Delete", "Backspace"], label: "削除（カタログから除去など）" },
      { keys: ["mod+,"], label: "設定" },
      { keys: ["shift+?"], label: "ショートカットの一覧" },
    ],
  },
];
