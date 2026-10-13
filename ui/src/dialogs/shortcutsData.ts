// ショートカットの一覧（UX-02）の内容。登録は App.tsx（全体。キーワード・コレクションの B・Ctrl / ⌘ + K・N、スタック・仮想コピーの
// Ctrl / ⌘ + G・S・Shift + S・Ctrl / ⌘ + ' を含む）・library/Grid.tsx（グリッドの移動・Enter・タイムラインの T）・
// viewer/Loupe.tsx（I・Z・等倍表示の Esc）・viewer/CompareView.tsx（比較表示の ←・→・↑・↓・Z・等倍表示の Esc）・
// viewer/SurveyView.tsx（選別表示の ←・→・/）・viewer/VideoPlayer.tsx（動画のスペース・J・K・L・,・.・M・Home・End）・
// develop/DevelopView.tsx（W・R・Z・J・I・\・Y・
// Shift ＋ Y と、切り抜き・スポイト・等倍表示の間の Esc・Enter）が持つ。比較表示（C）・選別表示（N）を始めるキーは
// App.tsx。ここは表示用の写しなので、キーを変えたらここも直す。

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
      { keys: ["t"], label: "タイムライン表示の切り替え（グリッド。撮影日・イベントの見出しを入れて撮影日時の順に並べる）" },
      { keys: ["e"], label: "ルーペ" },
      { keys: ["c"], label: "比較表示（選択と候補の 2 枚を並べる）" },
      { keys: ["n"], label: "選別表示（選んだ写真を並べる）" },
      { keys: ["d"], label: "現像" },
      { keys: ["Escape"], label: "グリッドに戻る（等倍表示・切り抜き・スポイトの間は、それを終える）" },
      { keys: ["ArrowLeft", "ArrowRight"], label: "前・次の写真（ルーペ・現像）" },
      { keys: ["z"], label: "等倍表示（ルーペ・現像）" },
      { keys: ["i"], label: "撮影情報の表示の切り替え（情報 1 → 情報 2 → なし。ルーペ・現像。項目は設定の「表示」）" },
    ],
  },
  {
    title: "動画（ルーペ）",
    items: [
      { keys: ["Space"], label: "再生・一時停止" },
      { keys: ["l"], label: "前へ再生（押すたびに 1・2・4・8 倍）" },
      { keys: ["j"], label: "後ろへ再生（押すたびに 1・2・4・8 倍）" },
      { keys: ["k"], label: "止める" },
      { keys: [",", "."], label: "1 コマ戻る・進む" },
      { keys: ["m"], label: "消音の切り替え" },
      { keys: ["Home", "End"], label: "先頭・最後へ" },
    ],
  },
  {
    title: "比較表示・選別表示",
    items: [
      { keys: ["ArrowLeft", "ArrowRight"], label: "比較: 候補を前・次の写真にする ／ 選別: 作業中の写真を移す" },
      { keys: ["ArrowUp"], label: "比較: 候補を選択にして、次の写真を候補にする" },
      { keys: ["ArrowDown"], label: "比較: 選択と候補を入れ替える" },
      { keys: ["z"], label: "比較: 等倍表示（2 枚のズーム・パンは同期。下の帯で同期を外せる）" },
      { keys: ["/"], label: "選別: 作業中の写真を表示から外す（写真は消えない）" },
      { keys: ["Escape"], label: "等倍表示を終える ／ グリッドに戻る" },
    ],
  },
  {
    title: "選別",
    items: [
      { keys: ["0", "1", "2", "3", "4", "5"], label: "評価（★0〜5。比較表示・選別表示では作業中の写真に付ける）" },
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
    title: "スタック・仮想コピー",
    items: [
      { keys: ["mod+g"], label: "選んだ写真をスタックにまとめる（一番上は表示中の写真。作ったスタックは折りたたむ）" },
      { keys: ["mod+shift+g"], label: "スタックを解除（写真は消えない）" },
      { keys: ["s"], label: "スタックの展開・折りたたみ（印のクリックでも）" },
      { keys: ["shift+s"], label: "表示中の写真をスタックの一番上にする" },
      { keys: ["mod+'", "mod+shift+7"], label: "仮想コピーを作る（日本語配列は Shift + 7 の「'」。元の写真と同じスタックに入る）" },
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
      { keys: ["F2"], label: "ファイルの名前を変える（グリッド・ルーペ。1 枚なら名前を入れる、複数ならテンプレート）" },
      {
        keys: ["Delete", "Backspace"],
        label:
          "削除（カタログから除去など。仮想コピーだけを選んでいれば仮想コピーの削除）。手動のコレクションを表示している間は、そのコレクションから外す（写真は消えない）",
      },
      { keys: ["mod+,"], label: "設定" },
      { keys: ["shift+?"], label: "ショートカットの一覧" },
    ],
  },
];
