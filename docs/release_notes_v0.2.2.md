GenzoParis 0.2.2（`e9c0942`）の配布物です。

## 配布物

| ファイル | 対象 |
|---|---|
| `GenzoParis_0.2.2_x64-setup.exe` | Windows 11（x64）。ユーザーごとのインストール（NSIS） |
| `GenzoParis_0.2.2_aarch64.dmg` | macOS 26.0 以降（Apple Silicon）。同梱するライブラリが macOS 26.0 向けのため、それより古い macOS では起動しません |
| `bundled-libs-*.txt` | 同梱したライブラリ（LibRaw とその依存、ONNX Runtime、Windows は Visual C++ のランタイム）の一覧と SHA-256 |
| `SHA256SUMS.txt` | 上のファイルの SHA-256 |

- 作成: GitHub Actions の Release ワークフロー（[run 42177458093](https://github.com/tokamocchi/GenzoParis/actions/runs/42177458093)。タグ `v0.2.2` は `main` の `e9c0942` を指します）。両 OS とも、配布物の中のワーカーが同梱の LibRaw で起動できることを、ワークフローの中で確かめています。
- **署名・公証はしていません。** Windows では SmartScreen の警告が、macOS では Gatekeeper の警告が出ることがあります。
- 動画のサムネイル・再生には、各自で入れた FFmpeg を使います（同梱していません）。
- 版 0.2.1 の配布物は公開していません（0.2.0 の次の公開は、この 0.2.2 です）。

## 0.2.0 からの主な変更

- **地図の表示（LIB-18）**: 表示モード「地図」（上の帯のボタン・Shift + M）を追加しました。表示中の写真を、使う位置（付けた位置があればそれ、なければカメラの位置）で地図に並べます。印はズームに合わせてまとめます。地図の範囲での絞り込みと、位置情報の有無での絞り込みもできます（画面の「地図の範囲で絞り込む」と「詳しい条件」の「位置情報」。CLI は `--bounds`・`--located`・`--no-location`）。**オンラインの地図は既定で無効**で、設定で有効にしたときだけ地図を作ります。地図でも、右のパネルは複数選択のまとめての編集になります。
- **取り込み時の現像プリセットの適用（IMP-07）**: 取り込みのダイアログの「取り込み時に当てる現像プリセット」で、取り込む写真にプリセットを当てられます（新しく取り込んだ写真だけ。動画は対象外）。結果の知らせに、当てた数が出ます。CLI は `import --develop-preset <ID か名前>`。取り込みが途中で失敗しても、当てた写真は現像結果から作り直します。
- **画面**: 写真の右クリックのメニューに「削除…」を追加しました。ルーペの写真をダブルクリックすると、グリッドに戻ります。
- **修正**: 地図で、ファイルの読み直し（内容の確認・再リンク）のあとにカメラの位置を読み直すようにしました。地図で写真を選んだときに印を作り直さないようにし、ダブルクリックでルーペを開けるようにしました。
- **配布の仕組み**: タグを選んで Release ワークフローを実行すると、GitHub の Releases に下書きのプレリリースを作る処理を追加しました（配布物の中身には関係しません）。

## ライセンス

- 同梱の LibRaw は LGPL-2.1 です。全文・著作権表示・ソースの入手先と差し替えの方法の案内を、配布物の中（Windows はインストール先の `licenses\LibRaw\`、macOS は `.app` の中）に入れています。
- 同梱の AI のモデル（U²-Net 小型版、MediaPipe の DeepLab-v3）と ONNX Runtime のライセンスの表示も、配布物の `licenses/` に入れています。
- 同梱物の扱いで確認が残っている事項は [docs/packaging.md](https://github.com/tokamocchi/GenzoParis/blob/v0.2.2/docs/packaging.md) の 8 章と、[docs/license_compliance.md](https://github.com/tokamocchi/GenzoParis/blob/v0.2.2/docs/license_compliance.md) の 4 章にあります。
