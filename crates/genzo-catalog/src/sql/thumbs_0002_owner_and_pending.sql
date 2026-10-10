-- GenzoParis のサムネイル DB（thumbs.db）のスキーマ 版 2: キャッシュの持ち主と、作り直し待ちの印。
--
-- - cache_meta: キャッシュを作ったカタログの世代（カタログの app_state の cache_generation）など。
--   復元の後（ID が再利用される）や、データのフォルダを残したまま別のカタログを作った後に、前の
--   写真のサムネイル・プレビューを使わないため、世代が違えばキャッシュを捨てる（コア API）。
-- - regen_pending: 現像設定を保存した後、現像結果から L0 / L1 を作り直す前の variant（作り直しの
--   依頼は終了で取り消されるため、印を残して次の起動で作り直す）。

CREATE TABLE cache_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE regen_pending (
    variant_id INTEGER PRIMARY KEY,
    marked_at  TEXT NOT NULL
) STRICT;
