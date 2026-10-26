# AI マスク（DEV-20）: 仕組み・モデルの置き方・確認の手順

被写体・空・人物の自動の選択（AI マスク。[01](01_feature_inventory.md) の DEV-20）の実装（2026-10-26）の説明と、人が
行うこと（モデルの差し替え、実機での確認）の手順です。設計は [04](04_architecture.md) の 2.1 節「ローカル補正」と 7.2 節
「AI 機能を差し込む位置」、決定事項は [decision_log.md](decision_log.md) の No.211〜216、人の判断が要る点は
[implementation_status.md](implementation_status.md) の判断の表 No.209〜213 にあります。

**2026-10-26 から、被写体と人物のモデルと ONNX Runtime を配布物（Windows・macOS）に同梱します**（決定事項ログ No.215。
利用者の指示「ライセンス確認して組み込んで」）。モデルはリポジトリの `crates/genzo-app/models` に置き、ONNX Runtime は配布物を
作るときに Microsoft の公式の配布物から取ります（2.4 節）。**空のモデルは同梱していません**（信頼できる入手先から取れる軽量な
ものが見つからなかった。2.3 節）。ONNX Runtime のテレメトリー（Microsoft への利用の情報の送信）は止めて使います（2.5 節）。

## 1. 仕組み

| 段階 | 場所 | 内容 |
|---|---|---|
| 推論の抽象 | `genzo-ml`（`MlBackend`・`MlSession`・`Tensor`） | 04 の 7.2 節のトレイト。モデルのファイルからセッションを作り、f32 のテンソルを入れて出す |
| ONNX Runtime | `genzo-ml` の `OnnxBackend`（機能 `onnx`。既定で有効） | pyke の `ort` 2.0.0-rc.15。共有ライブラリは**ビルド時に取らず**（既定の機能 `download-binaries` を外した）、実行時に指定したファイルから読み込む（`load-dynamic`）。実行プロバイダーは CPU だけ（CUDA・DirectML・Core ML は未対応。決定事項ログ No.212） |
| モデルの台帳 | `genzo-ml` の `ModelManifest`（`*.genzo-model.json`） | モデルごとの前処理・後処理・種類との対応・出どころとライセンス（2 章）。アプリのコードはモデルの中身を知らない |
| 推論の入口 | `genzo-ml` の `Segmenter`（`ModelLibrary` が実装） | 画像 → 種類の重みのラスター（8 ビット）。モデルのフォルダの台帳を読み、種類ごとに最初に使える台帳を選び、セッションを使い回す |
| コア API | `genzo-api` の `Core::compute_ai_mask`（コマンド `compute_ai_mask`）・`ai_mask_status`・`collect_ai_masks` | 切り抜く前の全体（G の外接矩形）を今の設定で sRGB の 8 ビットに描き（長辺 1024 まで）、推論し、ラスターを PNG にしてカタログに保存し、設定に入れる形（`MaskShape::Ai`）を返す。設定への適用は画面・CLI（色域のスポイトと同じ） |
| 保存 | `genzo-catalog` の `ai_mask`（スキーマの版 14。[catalog_schema.md](catalog_schema.md)） | 内容のアドレス（幅・高さと値の SHA-256）をキーにしたラスター。別の写真に貼り付けた設定からも参照されるので、asset を消しても残す（推論した写真の記録は NULL）。参照のない行は `genzo ai collect` で回収 |
| 設定 | `genzo-model` の `MaskShape::Ai { kind, model, mask: DataRef, width, height, bounds }` | ラスターの参照（ID `ai-mask:<種類>` と SHA-256）と大きさ、覆う範囲の G 座標。ラスター自体は設定に入れない（履歴の 1 件ごとに設定の全体を写すため） |
| 描画 | `genzo-pipeline` の `ProfileResolver::ai_mask` → `ProfileResources::ai_masks` → `local::ai`（ブラシと同じ G 座標の格子 `BrushRaster` に写す） | 描画のたびに推論し直さない。GPU 版はブラシと同じ経路（`local.wgsl` の種類 2）で、シェーダーは変えていない。ラスターが見つからなければ重み 0 で描き、警告する（`SensorWarning::AiMaskMissing`） |
| 画面 | 現像のパネル「マスク」の「＋ AI 被写体 / 空 / 人物」（マスク・部品の両方）と、AI の部品の「作り直す」 | 使える種類だけ押せる。計算の間は「計算しています…」。失敗はトースト |
| CLI | `genzo develop mask add <ID> --ai subject|sky|person`・`add-part … --ai`・`genzo ai status`・`genzo ai collect` | `--models-dir`・`--onnxruntime`（全体のオプション）、環境変数 `GENZO_MODELS_DIR`・`GENZO_ONNXRUNTIME` |

座標: ラスターは作ったときの G 座標（回転・歪曲補正の後、切り抜く前）の全体を覆います。ブラシと同じく、切り抜きを変えても
位置は変わりませんが、回転や歪曲補正を変えると画像に対してずれます（部品の「作り直す」で計算し直す）。

## 2. モデルの置き場と同梱

### 2.1 フォルダ

モデルのフォルダは、次の順で決まります（アプリと CLI で同じ）。

1. `CoreConfig::models_dir`（CLI の `--models-dir`）
2. 環境変数 `GENZO_MODELS_DIR`
3. OS のアプリのデータのフォルダの `models`（プリセットの `presets` と同じ場所。Windows は `%APPDATA%\<識別子>\models`、
   macOS は `~/Library/Application Support/<識別子>/models`。`genzo ai status` で表示）

その後に、配布物に同梱したモデルのフォルダ（`CoreConfig::bundled_models_dir`。アプリが起動時に資源のフォルダの `models` を
設定する。Windows はインストール先の `models`、macOS は `GenzoParis.app/Contents/Resources/models`）を読みます。種類ごとに、
先に読んだフォルダ（利用者のフォルダ）の使える台帳を使うので、利用者が置いたモデルが同梱のものより優先されます。同じ ID の
台帳は、先のものだけを使います。CLI は同梱のフォルダを使いません（配布物に含まれないため）。

ONNX Runtime の共有ライブラリ（Windows `onnxruntime.dll`、macOS `libonnxruntime.dylib`、Linux `libonnxruntime.so`）は、次の順で
探します（最初に見つかったもの）。

1. `CoreConfig::onnxruntime`（CLI の `--onnxruntime`）
2. 環境変数 `GENZO_ONNXRUNTIME`
3. モデルのフォルダ、同梱のモデルのフォルダ（配布物は ONNX Runtime をここに置く）
4. 実行ファイルと同じフォルダ、その `lib`、`../Frameworks`（macOS の `.app`。今の配布物は 3 の同梱のフォルダに置く）

ONNX Runtime はプロセスで 1 度しか読み込めません（`ort` の制約。別の場所を指定しても最初のものを使う）。版は `ort`
2.0.0-rc.15 が使う API の版（`ORT_API_VERSION` 31 → ONNX Runtime 1.31 以上。**AI の理解。要確認**）に対応したものが要ります。
この開発環境では、PyPI の `onnxruntime` 1.31.0 の `libonnxruntime.so.1.31.0` と、GitHub の Releases の公式の配布物
`onnxruntime-linux-x64-1.31.0.tgz` の共有ライブラリで結合テストを通しました（3 章）。

### 2.2 台帳（`<名前>.genzo-model.json`）

モデルのファイルと同じフォルダに、台帳の JSON を置きます（`genzo-ml` の `ModelManifest`）。

```json
{
  "id": "u2netp-2020",
  "name": "U²-Net (small)",
  "file": "u2netp.onnx",
  "sha256": "<ファイルの SHA-256。省略なら確かめない>",
  "license": "Apache-2.0",
  "source": "https://…（入手先）",
  "input": {
    "name": "input",
    "width": 320, "height": 320,
    "layout": "nchw",
    "range": "unit",
    "mean": [0.485, 0.456, 0.406],
    "std": [0.229, 0.224, 0.225],
    "bgr": false
  },
  "output": { "name": "output", "activation": "sigmoid", "layout": "nchw" },
  "kinds": { "subject": { "channel": 0 } },
  "threads": null
}
```

| 項目 | 意味 |
|---|---|
| `id` | 現像設定に記録する ID（1〜100 文字。空白・`/`・`\` なし）。モデルを差し替えたら ID も変える |
| `file` | モデルのファイル（台帳と同じフォルダからの相対パス。`..` 不可） |
| `sha256` | ファイルの SHA-256（小文字 64 文字）。書いてあれば読み込みの前に確かめる（違えば使わない） |
| `input.width`・`height` | モデルの固定の入力の大きさ（画像はこの大きさに縮める。縦横比は保たない） |
| `input.layout` | `nchw`（PyTorch 由来。既定）か `nhwc`（TensorFlow 由来） |
| `input.range` | 画素の値の範囲（`unit`: 0〜1、`byte`: 0〜255、`signed`: −1〜1）。その後に `mean` を引き `std` で割る（1 つか 3 つ。省略なら 0・1） |
| `input.bgr` | チャンネルの順を BGR にする |
| `output.activation` | `none`（すでに確率）・`sigmoid`・`softmax`（チャンネルの軸） |
| `output.layout` | 出力の並び（`[1, C, H, W]` か `[1, H, W, C]`。`[1, H, W]`・`[H, W]` はチャンネル 1） |
| `kinds` | 種類（`subject`・`sky`・`person`）ごとの読み方。`{"channel": c}`（1 チャンネルの値）か `{"classes": [...]}`（クラスの確率の和。活性化は `output.activation` をクラスごとに当て、和が 1 を超えたら 1 に丸める。多クラスのモデル） |
| `threads` | 1 回の推論に使うスレッドの数（省略なら ONNX Runtime の既定） |

台帳が読めない・ファイルがない・SHA-256 が違う台帳は使わず、`genzo ai status` と画面のボタンの説明に理由を出します。
同じ種類を扱える台帳が複数あれば、ファイル名の順で最初のものを使います。

### 2.3 同梱したモデル（2026-10-26。**ライセンスは AI が一次資料で確認。法的な判断は人の確認待ち**）

| 種類 | モデル（台帳の ID） | ファイル（`crates/genzo-app/models`） | 入手元 | ライセンス |
|---|---|---|---|---|
| 被写体 | U²-Net の小型版（`u2netp-subject`） | `u2netp.onnx`（4,574,861 バイト。SHA-256 `309c8469…`）・`u2netp-subject.genzo-model.json` | rembg の配布物 https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx （変更なし）。元のモデルは https://github.com/xuebinqin/U-2-Net | Apache-2.0（U-2-Net のリポジトリの LICENSE。rembg の README に「モデルの重みは rembg の MIT とは別に、それぞれのライセンスに従う」とあり、u2netp の出典として U-2-Net を挙げている） |
| 人物 | DeepLab-v3（MediaPipe。`mediapipe-deeplab-v3-person`） | `deeplab_v3.onnx`（2,782,503 バイト。SHA-256 `d535514b…`）・`deeplab-v3-person.genzo-model.json` | Google の MediaPipe の配布物 `deeplab_v3.tflite`（https://storage.googleapis.com/mediapipe-models/image_segmenter/deeplab_v3/float32/1/deeplab_v3.tflite 、SHA-256 `ff36e24d…`）を、tf2onnx 1.17.0（opset 13）で ONNX に変換したもの | **Apache-2.0 として扱う（ファイル自体にライセンスの表示がない。下の注）** |

確かめたこと（2026-10-26）:

- U-2-Net の LICENSE は Apache License 2.0 の全文（著作権者の欄は空のひな形のまま）。rembg の LICENSE は MIT（Copyright (c) 2020
  Daniel Gatis）。
- DeepLab-v3: MediaPipe のリポジトリ（Apache-2.0）の中で、似た DeepLab-v3 の TFLite（テスト用のデータ）を含むパッケージは
  `licenses = ["notice"]  # Apache 2.0` と宣言している（同じファイルではない。SHA-256 が違う）。DeepLab の元のコード
  （tensorflow/models の research/deeplab）は Apache-2.0。MediaPipe の画像のセグメンテーションの案内のページ（ai.google.dev）は、
  この環境から読めなかった（検索の結果の要約では、DeepLab-v3 の行にモデルカードのリンクがない）。**同梱した `deeplab_v3.tflite` に
  ライセンスを明記した一次資料は見つけていない。**
- 学習データ: u2netp は DUTS-TR、DeepLab-v3 は ImageNet・MS-COCO・PASCAL VOC 2012 で学習したとされる。学習データの利用条件が
  モデルの配布に及ぶかは確かめていない（人・専門家の判断。判断の表 No.213）。
- ONNX への変換（DeepLab-v3）は、同じ版の tf2onnx でも実行のたびにバイト列が変わる（2 回の変換で SHA-256 が違った）。そのため
  配布のビルドで変換せず、変換したファイルをリポジトリに置いた。変換の内容（ラベルの一覧のメタデータが落ちること）は
  `licenses/third-party/ai-models.txt` に変更の表示として書いた（Apache-2.0 の第 4 条）。
- 選ばなかったもの: MediaPipe の selfie segmenter（独自の演算 `Convolution2DTransposeBias` を含み、tf2onnx で変換できなかった）、
  空のモデル（ADE20K などで学習したもの。この環境から届く信頼できる入手先（GitHub の Releases・Google の配布物）に軽量なものが
  見つからなかった）。

動作の確認: 2026-10-26 にこの開発環境（Linux。CPU）で、MediaPipe のサンプル写真 4 枚に CLI からマスクを作り、被写体・人物とも
おおむね意図どおりに選ばれた（人の写っていない写真では人物は何も選ばれない）。推論の時間は 1 回 0.3〜0.4 秒（このコンテナの
CPU。参考値）。DeepLab-v3 の人物の境界は U²-Net より粗い。

モデルを差し替えるときは、2.2 節の台帳を書いて利用者のフォルダに置きます（同梱のものより優先）。同梱のモデルを変えるときは、
`crates/genzo-app/models` のファイルと台帳、`tauri.conf.json` の `bundle.resources`（`scripts/lib/third-party.mjs` の
`BUNDLED_MODEL_RESOURCES`）、`licenses/third-party/ai-models.txt`、[third_party.md](third_party.md) を合わせて変えます
（`genzo-ml` の `tests/bundled_models.rs` と `scripts/lib/third-party.test.mjs` が食い違いを見つける）。

### 2.4 配布物への同梱

| もの | 置き場（配布物の中） | 作り方 |
|---|---|---|
| モデルと台帳（4 ファイル） | 資源のフォルダの `models` | リポジトリの `crates/genzo-app/models` を `tauri.conf.json` の `bundle.resources` で同梱（Linux の .deb にも入る） |
| ONNX Runtime 1.31.0（Windows `onnxruntime.dll`、macOS `libonnxruntime.dylib`） | 資源のフォルダの `models`（macOS も `Contents/Frameworks` ではない） | 配布物を作るとき（`scripts/prepare-worker.mjs` → `scripts/lib/onnxruntime.mjs`）に、GitHub の Releases の公式の配布物（`onnxruntime-win-x64-1.31.0.zip`・`onnxruntime-osx-arm64-1.31.0.tgz`）を取り、固定した SHA-256 と比べ（違えば止める）、共有ライブラリを取り出す。Linux には同梱しない |
| Visual C++ のランタイム（Windows） | インストール先（実行ファイルと同じフォルダ） | `onnxruntime.dll` は `msvcp140.dll`・`msvcp140_1.dll`・`vcruntime140.dll`・`vcruntime140_1.dll` を読み込むので、ワーカーのものと合わせて同梱する（`scripts/lib/worker.mjs`） |
| ライセンスの文書 | `licenses/third-party/ai-models.txt`（モデル）、`licenses/third-party/onnxruntime/`（ONNX Runtime の LICENSE と ThirdPartyNotices.txt） | モデルの文書はリポジトリに置く。ONNX Runtime の文書は公式の配布物から取り出す |

macOS: ONNX Runtime 1.31.0 の dylib は macOS 14.0 以降向け（minos 14.0）。`.app` の最小の版の確認（`GENZO_ALLOW_NEWER_MINOS`。
[packaging.md](packaging.md) の 2.3）に含めます。今の配布（macOS 26 以降向け。決定事項ログ No.74）では影響しません。

配布物の確認（`release.yml`）で、モデル・台帳・ONNX Runtime・ライセンスの文書がインストール先（`.app` の中）にあることと、
ONNX Runtime を読み込めて版が 1.31.0 であること（`scripts/check-onnxruntime.py`）を確かめます。**Windows・macOS の配布物は
まだ作っていません**（2026-10-26。実機での確認も未）。

### 2.5 ONNX Runtime のテレメトリー

Microsoft の公式の配布物の ONNX Runtime は、利用の情報（モデルの生産者・演算子・推論の回数と時間など）を Microsoft に送る
テレメトリーが既定で有効です（配布物の `Privacy.md`。1.31.0 で確認）。`ort` の既定も有効にする側でした。写真に関する情報を
外に出さないため、次の 2 つで止めます（決定事項ログ No.216）。

- アプリ・CLI の `main` の最初で環境変数 `ORT_DISABLE_TELEMETRY=1` を設定する（ONNX Runtime の初期化の前に設定されていれば、
  送信の仕組みそのものを作らない）。
- 推論の環境を作るときに `DisableTelemetryEvents` を呼ぶ（`ort` の `with_telemetry(false)`）。

この開発環境で、推論の間に strace でネットワークの接続（`connect`）が起きないことを見ました。ただし、止めないときに送信が
起きることもこの環境では再現できなかったので、止める処理の効果を実測では確かめていません（ONNX Runtime の文書の方法に
従った）。

## 3. 確認の手順

### 3.1 開発環境（ONNX Runtime なし）

```sh
cargo test -p genzo-model -p genzo-pipeline -p genzo-catalog -p genzo-ml -p genzo-api --test ai_masks
cargo test -p genzo-ml --test bundled_models          # 同梱のモデルの台帳と SHA-256
cargo test -p genzo-cli --test cli ai_masks           # 使えないときの状態とエラー
cd ui && npm test -- mask                              # 画面の流れ（モックの推論）と描画
```

コア API・画面のテストは、`genzo-ml` のテスト用の推論（`FixedSegmenter`。右半分を選ぶ）を `CoreHooks::segmenter` で差し込み、
計算 → 保存 → 設定 → 描画（右半分だけ変わる）→ 履歴・スナップショット・仮想コピーの参照 → 回収 → 消したラスターを指す
設定の警告、を確かめます。

### 3.2 ONNX Runtime の結合テスト

環境変数 `GENZO_ORT_DYLIB` で共有ライブラリを指定したときだけ実行します（なければスキップして知らせる）。モデルは
`crates/genzo-ml/tests/fixtures/mean_threshold.onnx`（`make_mean_threshold.py` で作った、チャンネルの平均のしきい値を
返すだけの小さなもの。学習したモデルではない）。

```sh
GENZO_ORT_DYLIB=/path/to/libonnxruntime.so cargo test -p genzo-ml --test onnx --test bundled_models
GENZO_ORT_DYLIB=/path/to/libonnxruntime.so cargo test -p genzo-cli --test cli ai_masks_are_computed_with_onnx_runtime
```

`bundled_models` は、同梱のモデルで実際に推論します（暗い円の画像で、被写体は円の中心を選び隅を選ばない、人物はほぼ何も
選ばない）。2026-10-26 に公式の配布物 `onnxruntime-linux-x64-1.31.0.tgz` の共有ライブラリで通りました。

2026-10-26 にこの開発環境（Linux）で、PyPI の `onnxruntime` 1.31.0 の `libonnxruntime.so.1.31.0` を指定して両方通りました
（CLI は `develop mask add --ai sky` → `list`・`overlay`・書き出しまで）。Windows・macOS では未確認です。

### 3.3 実機（人が行う）

1. 配布物を入れたアプリで写真を開き、「マスク」の AI の部品が「被写体」「人物」だけ押せること（空は押せない）を確かめる。
2. 「＋ AI 被写体」などを押す。数秒で右の一覧にマスクが足され、O キーの重ねて表示で選択範囲が
   見えること。
3. 露光量などを動かして、範囲の中だけが変わること。切り抜きを変えても位置が変わらないこと。回転を変えると範囲がずれ、
   「作り直す」で直ること。
4. 推論の時間（長辺 1024 の入力。CPU）を計測して [implementation_status.md](implementation_status.md) に記録する（PERF-01 の
   対話の応答の目安に対して長ければ、ジョブ化（進捗の表示）を検討する。判断の表 No.211）。
5. `genzo ai collect` で参照のないラスターが消え、消したラスターを指す設定（別のカタログから貼り付けたなど）が警告付きで
   描けること。

## 4. 制限（2026-10-26）

- 推論は CPU だけ。GPU の実行プロバイダー（CUDA・DirectML・Core ML）は機能フラグを足していない（決定事項ログ No.212）。
- 空のモデルは同梱していない（2.3 節）。利用者がモデルと台帳を置けば使える。
- CLI は同梱のモデルを使わない（`--models-dir` などで指定する）。Linux の .deb はモデルだけを含み、ONNX Runtime は含まない。
- 推論は同期（コマンドの応答まで待つ。画面は計算中の表示だけ）。長いモデルではジョブ化が要る（判断の表 No.211）。
- ラスターの長辺は 1024 まで（`DEFAULT_INPUT_LONG_EDGE`）。モデルの出力（320〜512 が多い）を双線形で広げるので、細部の
  境界はモデルの解像度で決まる。
- 設定を別の写真に貼り付けると、元の写真のラスターをそのまま指す（別の写真の選択範囲にはならない。部品の「作り直す」で
  計算し直す）。
- 現像プリセットには AI マスクの部品を写さない（ラスターはカタログの中にあり、プリセットはカタログをまたいで使うため。マスクの項目を選んでも、AI の部品だけのマスクは除き、ほかの部品は残す）。
- 推論の間もほかのマスクは編集できる。結果は、押したときのマスク・部品を中身で探し直して入れ、見つからなければ（消した・部品を足した）入れずに知らせる。
- カタログのバックアップにはラスターが含まれる。thumbs.db（作り直せるキャッシュ）には置かない。
