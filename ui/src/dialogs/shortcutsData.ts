// ショートカットの一覧（UX-02）の内容。登録は App.tsx（全体。キーワード・コレクションの B・Ctrl / ⌘ + K・N を含む）・library/Grid.tsx（グリッドの移動・Enter）・
// viewer/Loupe.tsx（I・Z・等倍表示の Esc）・develop/DevelopView.tsx（W・R・Z・J・I・\・Y・
// Shift ＋ Y と、切り抜き・スポイト・等倍表示の間の Esc・Enter）が持つ。ここは表示用の写しなので、キーを変えたらここも直す。

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
      { keys: ["Escape"], label: "グリッドに戻る（等倍表示・切り抜き・スポイトの間は、それを終える）" },
      { keys: ["ArrowLeft", "ArrowRight"], label: "前・次の写真（ルーペ・現像）" },
      { keys: ["z"], label: "等倍表示（ルーペ・現像）" },
      { keys: ["i"], label: "撮影情報の表示の切り替え（情報 1 → 情報 2 → なし。ルーペ・現像。項目は設定の「表示」）" },
    ],
  },
  {
    title: "選別",
    items: [
      { keys: ["0", "1", "2", "3", "4", "5"], label: "評価（★0〜5）" },
      { keys: ["p"], label: "採用" },
      { keys: ["x"], label: "不採用" },
      { keys: ["u"], label: "フラグを外す" },
      { keys: ["6", "7", "8", "9"], label: "カラーラベル（赤・黄・緑・青。すべて同じラベルなら外す）" },
    ],
  },
  {
    title: "キーワード・コレクション",
    items: [
      { keys: ["b"], label: "ターゲットコレクションに入れる（すべて入っていれば外す。なければ入れる先を選ぶ）" },
      { keys: ["mod+k"], label: "キーワードの欄へ（グリッド・ルーペ。「場所 > 京都, 家族」の形で入れて Enter）" },
      { keys: ["mod+n"], label: "コレクションを作る（操作の対象の写真を入れられる）" },
    ],
  },
  {
    title: "選択（グリッド）",
    items: [
      { keys: ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"], label: "選択を移す" },
      {
        keys: ["shift+ArrowLeft", "shift+ArrowRight", "shift+ArrowUp", "shift+ArrowDown"],
        label: "範囲を選ぶ",
      },
      { keys: ["Home", "End"], label: "先頭・最後" },
      { keys: ["PageUp", "PageDown"], label: "1 画面ぶん移す" },
      { keys: ["Enter"], label: "ルーペで開く" },
      { keys: ["mod+a"], label: "すべて選ぶ（先頭の 10,000 件まで）" },
      { keys: ["mod+d"], label: "選択を表示中の 1 枚だけにする" },
    ],
  },
  {
    title: "現像",
    items: [
      { keys: ["w"], label: "ホワイトバランスのスポイト" },
      { keys: ["r"], label: "切り抜き（もう一度押すと確定）" },
      { keys: ["Enter"], label: "切り抜きを確定" },
      { keys: ["Escape"], label: "切り抜き・スポイトを取り消す" },
      { keys: ["j"], label: "白飛び・黒つぶれの表示" },
      { keys: ["\\"], label: "ビフォー / アフター: ビフォーだけ（もう一度押すとアフター）" },
      { keys: ["y"], label: "ビフォー / アフター: 左右に並べる" },
      { keys: ["shift+y"], label: "ビフォー / アフター: 分割（左半分がビフォー）" },
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
      {
        keys: ["Delete", "Backspace"],
        label: "削除（カタログから除去など）。手動のコレクションを表示している間は、そのコレクションから外す（写真は消えない）",
      },
      { keys: ["mod+,"], label: "設定" },
      { keys: ["shift+?"], label: "ショートカットの一覧" },
    ],
  },
];
