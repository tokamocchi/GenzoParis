-- GenzoParis のサムネイル DB（thumbs.db）のスキーマ 版 1。
--
-- docs/04_architecture.md の 4 章・4.1 節。失っても作り直せるデータだけを置く
-- （カタログのバックアップに含めない。synchronous = NORMAL）。
-- カタログとは別の DB なので外部キーは使えない。存在しない variant の行は回収のジョブで削除する。

-- L0: サムネイル（長辺 320px の JPEG）。variant ごとに 1 行。
CREATE TABLE thumb (
    variant_id INTEGER PRIMARY KEY,
    cache_key  TEXT NOT NULL,                 -- キャッシュキーのダイジェスト（16 進数）
    jpeg       BLOB NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

-- L1: 標準プレビューのファイル（previews/ab/cd/<キー>.jpg）の索引。最後に使った順で回収する。
CREATE TABLE preview (
    cache_key    TEXT PRIMARY KEY,
    variant_id   INTEGER,                     -- 参考（回収には使わない）
    size         INTEGER NOT NULL CHECK (size >= 0),
    last_used_at TEXT NOT NULL,               -- 最後に使った日時（UTC）
    use_seq      INTEGER NOT NULL             -- 使った順（同じ時刻でも順序を決めるため）
) STRICT, WITHOUT ROWID;
CREATE INDEX preview_use_seq ON preview(use_seq);
