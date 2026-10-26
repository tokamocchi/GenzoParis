-- GenzoParis のカタログ（catalog.db）のスキーマ 版 14: AI マスクのラスター（DEV-20）。
-- docs/04_architecture.md の 2.1 節「ローカル補正」・7.2 節。
--
-- - ai_mask: AI マスク（被写体・空・人物の自動の選択）の推論で作った重みのラスター。現像設定の `MaskShape::Ai` は
--   このラスターを内容の SHA-256 で参照する（設定の JSON にはラスターを入れない。履歴の 1 件ごとに設定の全体を
--   写すため）。描画のたびに推論し直さない（04 の 7.2 節）。
--   - 内容のアドレス（sha256 が主キー）なので、同じラスターは 1 行。仮想コピー・スナップショット・履歴・設定の
--     貼り付けは同じ参照を写す。
--   - asset の削除で消える（ON DELETE CASCADE）。参照のなくなった行は、カタログの保守で回収する
--     （genzo-catalog の ai_mask.rs の delete_unreferenced_ai_masks。asset を消すまでは残る）。
--   - data はラスター（幅 × 高さの 8 ビットの値、行優先）を encoding の形で符号化したもの（`raw`: そのまま、
--     `png`: 8 ビットのグレースケールの PNG）。sha256 は符号化する前の値の SHA-256。
--   - バックアップ（catalog.db の VACUUM INTO）に含まれる。thumbs.db（作り直せるキャッシュ）には置かない。
--
-- 既存の設定・表は変えない。

CREATE TABLE ai_mask (
    sha256     TEXT PRIMARY KEY CHECK (length(sha256) = 64),            -- ラスターの内容の SHA-256（小文字の 16 進数）
    asset_id   INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,  -- 推論した写真
    kind       TEXT NOT NULL CHECK (kind IN ('subject', 'sky', 'person')),  -- 種類
    model      TEXT NOT NULL,                                            -- 推論に使ったモデルの ID
    width      INTEGER NOT NULL CHECK (width >= 1),                      -- 幅（画素）
    height     INTEGER NOT NULL CHECK (height >= 1),                     -- 高さ（画素）
    encoding   TEXT NOT NULL CHECK (encoding IN ('raw', 'png')),         -- data の符号化
    data       BLOB NOT NULL,                                            -- ラスター
    created_at TEXT NOT NULL                                             -- 作った日時（UTC）
) STRICT;

CREATE INDEX ai_mask_asset ON ai_mask(asset_id);
