-- GenzoParis のカタログ（catalog.db）のスキーマ 版 9: スタック（LIB-12。docs/04_architecture.md の 3.10 節）。
--
-- 表（stack・stack_member）は版 1 からあるが、版 8 まではアプリが使っていなかった。この版で変えるもの:
--
-- - stack を AUTOINCREMENT で作り直す（ID を使い回さない。決定事項ログ No.84 の「ID で参照する表を新しく
--   足す場合は最初から AUTOINCREMENT にする」に合わせる。画面・CLI がスタックを ID で指すため）。あわせて、
--   折りたたみの状態 collapsed（1 = 折りたたむ。既定）を足す。CHECK・AUTOINCREMENT は ALTER TABLE で
--   変えられないため、SQLite の説明書の ALTER TABLE の「12 の手順」で作り直す（版 8 と同じ。マイグレーションは
--   外部キーの確認を止めて 1 つのトランザクションで適用し、コミットの前に foreign_key_check で確かめる。
--   stack_member の REFERENCES stack(id) は名前で参照しているので、作り直した後もそのまま使える）。
-- - トリガー stack_member_after_delete: スタックの写真が抜けたとき（写真の削除・仮想コピーの削除・
--   スタックから外す操作。variant の削除による ON DELETE CASCADE でも動く）に、一番上の写真が抜けたら残りの
--   最初の写真を一番上にし、1 枚以下になったスタックを消す（どの削除の経路でも、写真 1 枚だけのスタックと、
--   一番上の写真のないスタックを残さない）。
-- - 公開ビュー photo_stacks_v1（ORG-07。docs/catalog_schema.md の 3 章）。
--
-- スタックの中の順は (position, variant_id) で、一番上の写真（top_variant_id）は常に最初（アプリは位置を
-- 0 から付け直す）。既存の行（外部のツールで作られたもの）の位置の付け直し・一番上の写真の決め直し・1 枚以下の
-- スタックの削除は、この SQL の後の Rust の処理（schema.rs の migrate_stacks）で行う。

CREATE TABLE stack_new (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    top_variant_id INTEGER REFERENCES variant(id) ON DELETE SET NULL,
    collapsed      INTEGER NOT NULL DEFAULT 1 CHECK (collapsed IN (0, 1))
) STRICT;

INSERT INTO stack_new(id, top_variant_id) SELECT id, top_variant_id FROM stack;

DROP TABLE stack;
ALTER TABLE stack_new RENAME TO stack;

CREATE INDEX stack_top_variant ON stack(top_variant_id);

-- 連番。作り直した表へ id を指定して入れたので SQLite が行を作っているが、名前の付け替えに頼らず、
-- ここで決めた値（残っている行の最大の id）にする（版 4・版 8 と同じ）。
DELETE FROM sqlite_sequence WHERE name IN ('stack', 'stack_new');
INSERT INTO sqlite_sequence(name, seq) SELECT 'stack', coalesce(max(id), 0) FROM stack;

-- スタックの写真が抜けた後の整え（一番上の写真の付け替えと、1 枚以下になったスタックの削除）。
-- variant の削除では、stack.top_variant_id の ON DELETE SET NULL と、この行の ON DELETE CASCADE の
-- どちらが先でもよいよう、一番上の写真が NULL か抜けた写真なら付け替える。スタックを消すと残りの行も
-- ON DELETE CASCADE で消える（再帰のトリガーは既定で無効。動いても何もしない）。
CREATE TRIGGER stack_member_after_delete AFTER DELETE ON stack_member BEGIN
    UPDATE stack
       SET top_variant_id = (
               SELECT m.variant_id FROM stack_member AS m
                WHERE m.stack_id = old.stack_id
                ORDER BY m.position, m.variant_id
                LIMIT 1)
     WHERE id = old.stack_id
       AND (top_variant_id IS NULL OR top_variant_id = old.variant_id);
    DELETE FROM stack
     WHERE id = old.stack_id
       AND (SELECT count(*) FROM stack_member AS m WHERE m.stack_id = old.stack_id) < 2;
END;

-- スタックに入っている写真（1 行 = 1 つの variant）。position はスタックの中の順（1 から。1 が一番上）、
-- stack_size はスタックの写真の数。古い SQLite でも読めるよう、ウィンドウ関数を使わずに数える。
CREATE VIEW photo_stacks_v1 AS
SELECT
    m.variant_id                         AS variant_id,
    m.stack_id                           AS stack_id,
    (SELECT count(*) FROM stack_member AS o
      WHERE o.stack_id = m.stack_id
        AND (o.position < m.position
             OR (o.position = m.position AND o.variant_id < m.variant_id))) + 1
                                         AS position,
    (s.top_variant_id IS m.variant_id)   AS is_top,
    (SELECT count(*) FROM stack_member AS c WHERE c.stack_id = m.stack_id)
                                         AS stack_size
FROM stack_member AS m
JOIN stack AS s ON s.id = m.stack_id;
