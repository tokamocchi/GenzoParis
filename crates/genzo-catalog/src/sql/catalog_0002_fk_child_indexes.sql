-- GenzoParis のカタログ（catalog.db）のスキーマ 版 2: 外部キーの子の列の索引。
--
-- 親の行を削除・変更するとき、SQLite は子のテーブルから参照している行を探す
-- （ON DELETE SET NULL / CASCADE の処理と、制約の確認）。子の列に索引がないと、
-- 親の行 1 件ごとに子のテーブル全体を読む（SQLite の説明書「SQLite Foreign Key Support」の
-- 3 節「Required and Suggested Database Indexes」）。
--
-- 版 1 では variant.history_pos（→ history_entry）に索引がなく、履歴を削除するたびに
-- variant の全件を読んでいた。asset の除去（6.4 節）では、ON DELETE CASCADE で消える
-- 履歴 1 件ごとに variant を全件読むため、50 万件のカタログで数百件を除くだけでも
-- 分単位の時間がかかる（2 万件・300 件の除去で 3.2 秒 → 索引ありで 14 ミリ秒。レビューで計測）。
-- stack.top_variant_id（→ variant）も同じ理由で索引を作る（スタックは v1 で使う）。

CREATE INDEX variant_history_pos ON variant(history_pos);
CREATE INDEX stack_top_variant ON stack(top_variant_id);
