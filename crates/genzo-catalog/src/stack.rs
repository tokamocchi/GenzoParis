//! スタック（LIB-12。docs/04_architecture.md の 3.10 節）。
//!
//! - 連写などの関連する写真（variant）を 1 つにまとめる。variant は高々 1 つのスタックに属する
//!   （`stack_member.variant_id` の `UNIQUE`）。スタックは 2 枚以上で、写真が抜けて 1 枚以下になったら
//!   消える（トリガー `stack_member_after_delete`。写真の削除・仮想コピーの削除の `ON DELETE CASCADE` でも
//!   動くので、どの削除の経路でも 1 枚だけのスタックを残さない）。
//! - スタックの中の順は `(position, variant_id)`。一番上の写真（`stack.top_variant_id`）は常に最初で、この
//!   モジュールの操作は位置を 0 から付け直す（一番上の写真が抜けたときは、トリガーが残りの最初の写真を一番上に
//!   する）。
//! - 折りたたみの状態（`stack.collapsed`）はカタログに保存する（開き直しても同じ）。作ったスタックは
//!   折りたたむ（仮想コピーの作成で作る・広げるスタックは展開する。`develop.rs`）。
//! - 作る（[`Catalog::create_stack`]）ときに、既にスタックに入っている写真が含まれていれば、そのスタックの
//!   写真もまとめて 1 つのスタックにする（Lightroom の「スタックにまとめる」と同じ）。新しいスタックの順は
//!   撮影日時の順（撮影日時のないものは最後、同じなら variant の ID の順）で、一番上の写真は指定したもの
//!   （なければ最初のもの）。
//! - 検索結果のまとめ方（[`StackLayout::group`]。グリッド・フィルムストリップの表示）: 条件に一致した写真の
//!   うち、同じスタックのものを 1 か所にまとめる。置く位置は、一番上の写真が一致していればその位置、
//!   一致していなければ最初に一致した写真の位置。折りたたんだスタックはその 1 枚だけ（代表）を出し、展開した
//!   スタックは一致した写真をスタックの中の順に並べる。一致しない写真は出さない（絞り込みで写真が消えない
//!   ように、代表は一致した写真から選ぶ）。
//! - 撮影時刻の間隔による自動スタック（[`Catalog::auto_stack`]）: 条件に合う写真・動画のうち、スタックに
//!   入っていない、撮影日時の分かるものを撮影日時の順に並べ、前の写真との間隔が指定の時間以下のものを続けて
//!   1 つのスタックにする（2 枚以上のものだけ）。

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;
use std::time::Duration;

use genzo_model::{StackId, VariantId};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::catalog::Catalog;
use crate::error::{CatalogError, Result};
use crate::search::{Filter, Sort, build_search_sql};
use crate::util::{ids_to_json, parse_db_utc};

/// 自動スタックの間隔の上限（**仮置き**: 1 時間。Lightroom Classic の「撮影時間による自動スタック」の範囲の
/// 上限と同じと AI は理解している。要確認）。
pub const MAX_AUTO_STACK_GAP: Duration = Duration::from_secs(3600);

/// スタック（[`Catalog::stack`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackInfo {
    /// ID。
    pub id: StackId,
    /// 一番上の写真（`members` の最初）。
    pub top_variant_id: VariantId,
    /// 折りたたんでいるか。
    pub collapsed: bool,
    /// 写真（スタックの中の順。最初が一番上）。
    pub members: Vec<VariantId>,
}

/// グリッドのセルに出すスタックの印（[`crate::VariantSummary::stack`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackBadge {
    /// スタック。
    pub stack_id: StackId,
    /// スタックの中の位置（1 から。1 が一番上）。
    pub position: u32,
    /// スタックの写真の数。
    pub count: u32,
    /// 折りたたんでいるか。
    pub collapsed: bool,
}

/// 検索結果をスタックでまとめたもの（[`StackLayout::group`]）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GroupedIds {
    /// 表示する順の variant。
    pub ids: Vec<VariantId>,
    /// 折りたたんだスタックの中に隠した写真の数（条件に一致したもののうち、代表でないもの）。
    pub hidden: u64,
}

/// 整数の ID の表に使う速いハッシュ（rustc-hash 1.x の `FxHasher` と同じ、回転と乗算だけのもの）。ID は
/// カタログの行の番号で外から細工されないので、衝突を狙う入力への強さ（標準の SipHash）は要らない。50 万件の
/// 検索結果を引くので、標準のハッシュより速いものを使う（docs/04_architecture.md の 3.10 節の性能）。
#[derive(Debug, Clone, Copy, Default)]
struct IdHasher(u64);

impl IdHasher {
    fn add(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }
}

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.add(u64::from(b));
        }
    }

    fn write_i64(&mut self, n: i64) {
        self.add(n as u64);
    }

    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }
}

/// [`IdHasher`] を使う表。
type IdMap<K, V> = HashMap<K, V, BuildHasherDefault<IdHasher>>;

/// カタログの変化の印（[`Catalog::stack_layout`] が覚えたものを使えるか）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChangeMark {
    data_version: i64,
    total_changes: u64,
}

/// 接続ごとに覚えておく、読んだスタックの所属と、読んだときのカタログの変化の印。
///
/// 50 万件のダミーのカタログでスタックの所属（185,034 枚）を読むのに約 0.1 秒かかる（04 の 3.10 節）ため、
/// 検索のたびに読み直さない。接続は 1 つのスレッドで使う（`Connection` は `Sync` でない）ので `RefCell`。
#[derive(Debug, Default)]
pub(crate) struct StackLayoutCache(RefCell<Option<(ChangeMark, Arc<StackLayout>)>>);

impl StackLayoutCache {
    fn get(&self, mark: ChangeMark) -> Option<Arc<StackLayout>> {
        match &*self.0.borrow() {
            Some((m, layout)) if *m == mark => Some(Arc::clone(layout)),
            _ => None,
        }
    }

    fn set(&self, mark: ChangeMark, layout: Arc<StackLayout>) {
        *self.0.borrow_mut() = Some((mark, layout));
    }
}

/// スタックの所属（[`Catalog::stack_layout`]）。検索結果を [`StackLayout::group`] でまとめるのに使う。
///
/// スタックに入っている写真の数だけの大きさ（入っていない写真は持たない）。スタックがなければ空で、
/// [`StackLayout::group`] は渡した順のまま返す。
#[derive(Debug, Clone, Default)]
pub struct StackLayout {
    /// variant → (スタックの番号（`stacks` の添字）, 位置)。
    members: IdMap<VariantId, (u32, i64)>,
    /// スタックの一番上の写真と、折りたたんでいるか（番号の順）。
    stacks: Vec<(Option<VariantId>, bool)>,
}

impl StackLayout {
    /// スタックの一覧から作る（テスト・計測用。位置はメンバーの並びの順）。
    pub fn from_stacks(stacks: &[StackInfo]) -> Self {
        let mut l = Self::default();
        for s in stacks {
            let index = l.stacks.len() as u32;
            l.stacks.push((Some(s.top_variant_id), s.collapsed));
            for (i, &v) in s.members.iter().enumerate() {
                l.members.insert(v, (index, i as i64));
            }
        }
        l
    }

    /// スタックがないか。
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// スタックに入っている写真の数。
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// 並べた検索結果 `ids` を、スタックでまとめる（モジュールの doc の「検索結果のまとめ方」）。
    /// `expand_all` なら、折りたたんだスタックも展開したものとして並べる。
    ///
    /// `ids` の件数を n として O(n)（写真ごとにハッシュ表を 1 回引く。展開するスタックの写真は数えて
    /// 振り分け、スタックごとに位置の順に並べる）。スタックがなければ `ids` をそのまま返す。
    pub fn group(&self, ids: Vec<VariantId>, expand_all: bool) -> GroupedIds {
        if self.members.is_empty() {
            return GroupedIds { ids, hidden: 0 };
        }
        const NONE: u32 = u32::MAX;
        let n_stacks = self.stacks.len();
        // 写真ごとのスタックの番号（スタックに入っていなければ NONE）。
        let mut slot: Vec<u32> = Vec::with_capacity(ids.len());
        // スタックごとの、置く位置（一番上の写真が一致していればその位置、していなければ最初に一致した写真の
        // 位置）・一番上の写真が一致したか・一致した写真の数。
        let mut anchor: Vec<usize> = vec![usize::MAX; n_stacks];
        let mut top_seen: Vec<bool> = vec![false; n_stacks];
        let mut count: Vec<u32> = vec![0; n_stacks];
        let collapsed = |k: usize| !expand_all && self.stacks[k].1;
        // 展開するスタックの写真（検索結果の中の位置・スタックの中の位置）。
        let mut shown: Vec<(usize, i64)> = Vec::new();
        for (i, &v) in ids.iter().enumerate() {
            let Some(&(s, pos)) = self.members.get(&v) else {
                slot.push(NONE);
                continue;
            };
            slot.push(s);
            let k = s as usize;
            if !collapsed(k) {
                shown.push((i, pos));
            }
            if anchor[k] == usize::MAX {
                anchor[k] = i;
            }
            if !top_seen[k] && self.stacks[k].0 == Some(v) {
                anchor[k] = i;
                top_seen[k] = true;
            }
            count[k] += 1;
        }
        if count.iter().all(|&c| c == 0) {
            return GroupedIds { ids, hidden: 0 };
        }
        // 展開するスタックの写真を、スタックごとの区画に振り分ける（区画の始まりは数の累積）。
        let mut start: Vec<usize> = vec![0; n_stacks + 1];
        for k in 0..n_stacks {
            let c = if collapsed(k) { 0 } else { count[k] as usize };
            start[k + 1] = start[k] + c;
        }
        let mut placed: Vec<(i64, VariantId)> = vec![(0, VariantId::new(0)); start[n_stacks]];
        let mut fill: Vec<usize> = start[..n_stacks].to_vec();
        for &(i, pos) in &shown {
            let k = slot[i] as usize;
            placed[fill[k]] = (pos, ids[i]);
            fill[k] += 1;
        }
        let mut out = Vec::with_capacity(ids.len());
        let mut hidden = 0u64;
        for (i, &v) in ids.iter().enumerate() {
            let s = slot[i];
            if s == NONE {
                out.push(v);
                continue;
            }
            let k = s as usize;
            if anchor[k] != i {
                continue;
            }
            if collapsed(k) {
                out.push(v);
                hidden += u64::from(count[k] - 1);
            } else {
                let part = &mut placed[start[k]..start[k + 1]];
                part.sort_unstable();
                out.extend(part.iter().map(|&(_, id)| id));
            }
        }
        GroupedIds { ids: out, hidden }
    }
}

/// 自動スタックの結果（[`Catalog::auto_stack`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AutoStackReport {
    /// 作った（`dry_run` なら作る）スタックの数。
    pub stacks: u64,
    /// 作ったスタックに入れた写真の数。
    pub photos: u64,
    /// 対象にした写真（条件に一致し、スタックに入っておらず、撮影日時の分かるもの）の数。
    pub considered: u64,
    /// すでにスタックに入っていたので対象にしなかった写真の数。
    pub skipped_in_stacks: u64,
    /// 撮影日時が分からないので対象にしなかった写真の数。
    pub skipped_without_time: u64,
}

/// `ids` を撮影日時の順（分からないものは最後、同じなら ID の順）に並べる（重複は 1 つにする）。存在しない
/// variant があれば [`CatalogError::NotFound`]。
fn order_by_capture(conn: &Connection, ids: &[VariantId]) -> Result<Vec<VariantId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT v.id, a.captured_at_utc FROM json_each(?1) j
         JOIN variant v ON v.id = j.value
         JOIN asset a ON a.id = v.asset_id",
    )?;
    let mut rows: Vec<(i64, Option<String>)> = stmt
        .query_map([ids_to_json(ids)], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let found: HashSet<i64> = rows.iter().map(|r| r.0).collect();
    if let Some(missing) = ids.iter().find(|v| !found.contains(&v.get())) {
        return Err(CatalogError::NotFound(format!("variant {missing}")));
    }
    rows.sort_by(|a, b| {
        let t = match (&a.1, &b.1) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };
        t.then(a.0.cmp(&b.0))
    });
    rows.dedup_by_key(|r| r.0);
    Ok(rows.into_iter().map(|r| VariantId::new(r.0)).collect())
}

/// variant が入っているスタック。
pub(crate) fn stack_of(conn: &Connection, variant: VariantId) -> Result<Option<StackId>> {
    Ok(conn
        .prepare_cached("SELECT stack_id FROM stack_member WHERE variant_id = ?1")?
        .query_row([variant.get()], |r| r.get::<_, i64>(0))
        .optional()?
        .map(StackId::new))
}

/// スタックの写真（スタックの中の順）。
pub(crate) fn members_of(conn: &Connection, stack: StackId) -> Result<Vec<VariantId>> {
    let mut stmt = conn.prepare_cached(
        "SELECT variant_id FROM stack_member WHERE stack_id = ?1 ORDER BY position, variant_id",
    )?;
    let rows = stmt.query_map([stack.get()], |r| r.get::<_, i64>(0))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(VariantId::new(r?));
    }
    Ok(out)
}

/// スタックの写真の順を `order` にする（位置を 0 から付け直し、最初を一番上にする）。`order` のうち、まだ
/// メンバーでないものは足す（ほかのスタックから外してから呼ぶこと）。メンバーの行は消さない（消すとトリガーが
/// スタックを消しうる）。
pub(crate) fn write_order(conn: &Connection, stack: StackId, order: &[VariantId]) -> Result<()> {
    let mut upsert = conn.prepare_cached(
        "INSERT INTO stack_member(stack_id, variant_id, position) VALUES (?1, ?2, ?3)
         ON CONFLICT(stack_id, variant_id) DO UPDATE SET position = excluded.position",
    )?;
    for (pos, v) in order.iter().enumerate() {
        upsert.execute(params![stack.get(), v.get(), pos as i64])?;
    }
    if let Some(top) = order.first() {
        conn.prepare_cached("UPDATE stack SET top_variant_id = ?2 WHERE id = ?1")?
            .execute(params![stack.get(), top.get()])?;
    }
    Ok(())
}

/// 新しいスタックを作り、写真を `order` の順に入れる（写真はどのスタックにも入っていないこと）。
pub(crate) fn insert_stack(
    conn: &Connection,
    order: &[VariantId],
    collapsed: bool,
) -> Result<StackId> {
    conn.prepare_cached("INSERT INTO stack(top_variant_id, collapsed) VALUES (?1, ?2)")?
        .execute(params![order.first().map(|v| v.get()), collapsed])?;
    let id = StackId::new(conn.last_insert_rowid());
    write_order(conn, id, order)?;
    Ok(id)
}

/// スタックを読む（なければ `None`）。
fn read_stack(conn: &Connection, id: StackId) -> Result<Option<StackInfo>> {
    let row: Option<(Option<i64>, bool)> = conn
        .prepare_cached("SELECT top_variant_id, collapsed FROM stack WHERE id = ?1")?
        .query_row([id.get()], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((top, collapsed)) = row else {
        return Ok(None);
    };
    let members = members_of(conn, id)?;
    let top = top
        .map(VariantId::new)
        .or_else(|| members.first().copied())
        .ok_or_else(|| CatalogError::Corrupt(format!("スタック {id} に写真がありません")))?;
    Ok(Some(StackInfo {
        id,
        top_variant_id: top,
        collapsed,
        members,
    }))
}

impl Catalog {
    /// 写真をまとめて 1 つのスタックにする（Ctrl / ⌘ + G）。既にスタックに入っている写真が含まれていれば、
    /// そのスタックの写真もまとめる（前のスタックは消える）。順は撮影日時の順で、一番上の写真は `top`
    /// （なければ最初のもの）。作ったスタックは折りたたむ。
    ///
    /// 渡した写真が（重複を除いて）2 枚未満（スタックに入っている写真 1 枚だけでも、まとめ直さない）、`top` が
    /// 含まれない場合は [`CatalogError::InvalidInput`]、存在しない variant があれば [`CatalogError::NotFound`]
    /// （何も変えない）。
    pub fn create_stack(
        &mut self,
        variant_ids: &[VariantId],
        top: Option<VariantId>,
    ) -> Result<StackId> {
        let tx = self.conn.transaction()?;
        let wanted = order_by_capture(&tx, variant_ids)?;
        if wanted.len() < 2 {
            return Err(CatalogError::InvalidInput(
                "スタックにするには 2 枚以上の写真を選んでください".to_owned(),
            ));
        }
        let mut old: Vec<StackId> = Vec::new();
        let mut all: Vec<VariantId> = wanted.clone();
        for &v in &wanted {
            if let Some(s) = stack_of(&tx, v)?
                && !old.contains(&s)
            {
                old.push(s);
                all.extend(members_of(&tx, s)?);
            }
        }
        let mut order = order_by_capture(&tx, &all)?;
        if let Some(t) = top {
            let i = order.iter().position(|&v| v == t).ok_or_else(|| {
                CatalogError::InvalidInput(format!(
                    "一番上にする写真 {t} がスタックにする写真に含まれていません"
                ))
            })?;
            let t = order.remove(i);
            order.insert(0, t);
        }
        for s in &old {
            tx.prepare_cached("DELETE FROM stack WHERE id = ?1")?
                .execute([s.get()])?;
        }
        let id = insert_stack(&tx, &order, true)?;
        tx.commit()?;
        Ok(id)
    }

    /// スタックに写真を足す（スタックの最後に、撮影日時の順で）。ほかのスタックに入っている写真は、そちらから
    /// 外して移す（外したスタックが 1 枚になれば消える）。新しく足した数を返す。
    pub fn add_to_stack(&mut self, stack: StackId, variant_ids: &[VariantId]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let mut order = match read_stack(&tx, stack)? {
            Some(s) => s.members,
            None => return Err(CatalogError::NotFound(format!("スタック {stack}"))),
        };
        let current: HashSet<VariantId> = order.iter().copied().collect();
        let added: Vec<VariantId> = order_by_capture(&tx, variant_ids)?
            .into_iter()
            .filter(|v| !current.contains(v))
            .collect();
        for &v in &added {
            tx.prepare_cached("DELETE FROM stack_member WHERE variant_id = ?1")?
                .execute([v.get()])?;
        }
        order.extend(&added);
        write_order(&tx, stack, &order)?;
        tx.commit()?;
        Ok(added.len())
    }

    /// 写真をスタックから外す（写真は消えない）。外した数を返す。一番上の写真を外したら残りの最初の写真が
    /// 一番上になり、1 枚以下になったスタックは消える（トリガー）。
    pub fn remove_from_stack(&mut self, variant_ids: &[VariantId]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx
            .prepare_cached(
                "DELETE FROM stack_member WHERE variant_id IN (SELECT value FROM json_each(?1))",
            )?
            .execute([ids_to_json(variant_ids)])?;
        tx.commit()?;
        Ok(n)
    }

    /// スタックを解除する（写真は消えない）。解除したスタックの数を返す（存在しないものは数えない）。
    pub fn unstack(&mut self, stacks: &[StackId]) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = tx
            .prepare_cached("DELETE FROM stack WHERE id IN (SELECT value FROM json_each(?1))")?
            .execute([ids_to_json(stacks)])?;
        tx.commit()?;
        Ok(n)
    }

    /// 写真をそのスタックの一番上にする（ほかの写真の順は変えない）。スタックに入っていなければ
    /// [`CatalogError::InvalidInput`]。
    pub fn set_stack_top(&mut self, variant: VariantId) -> Result<StackId> {
        let tx = self.conn.transaction()?;
        let stack = stack_of(&tx, variant)?.ok_or_else(|| {
            CatalogError::InvalidInput(format!("写真 {variant} はスタックに入っていません"))
        })?;
        let mut order = members_of(&tx, stack)?;
        order.retain(|&v| v != variant);
        order.insert(0, variant);
        write_order(&tx, stack, &order)?;
        tx.commit()?;
        Ok(stack)
    }

    /// スタックの折りたたみの状態を変える（`stacks` が `None` ならすべてのスタック）。状態の変わった
    /// スタックの数を返す。
    pub fn set_stacks_collapsed(
        &mut self,
        stacks: Option<&[StackId]>,
        collapsed: bool,
    ) -> Result<usize> {
        let tx = self.conn.transaction()?;
        let n = match stacks {
            Some(ids) => tx
                .prepare_cached(
                    "UPDATE stack SET collapsed = ?2
                     WHERE id IN (SELECT value FROM json_each(?1)) AND collapsed != ?2",
                )?
                .execute(params![ids_to_json(ids), collapsed])?,
            None => tx
                .prepare_cached("UPDATE stack SET collapsed = ?1 WHERE collapsed != ?1")?
                .execute([collapsed])?,
        };
        tx.commit()?;
        Ok(n)
    }

    /// スタックを読む。なければ [`CatalogError::NotFound`]。
    pub fn stack(&self, id: StackId) -> Result<StackInfo> {
        read_stack(&self.conn, id)?.ok_or_else(|| CatalogError::NotFound(format!("スタック {id}")))
    }

    /// 写真が入っているスタック（重ならないように、最初に現れた順。スタックに入っていない写真は飛ばす。
    /// 存在しない variant も飛ばす）。
    pub fn stacks_of_variants(&self, variant_ids: &[VariantId]) -> Result<Vec<StackInfo>> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for &v in variant_ids {
            if let Some(s) = stack_of(&self.conn, v)?
                && seen.insert(s)
                && let Some(info) = read_stack(&self.conn, s)?
            {
                out.push(info);
            }
        }
        Ok(out)
    }

    /// スタックの数。
    pub fn stack_count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT count(*) FROM stack", [], |r| r.get(0))?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    /// スタックの所属（検索結果を [`StackLayout::group`] でまとめるため）。スタックがなければ表を読まずに
    /// 空を返す。読む量はスタックに入っている写真の数に比例する。
    ///
    /// 読んだものはこの接続に覚えておき、カタログが変わっていなければ読み直さない（`StackLayoutCache`。
    /// 書き込みのない間の検索（絞り込み・並べ替え・フォルダを変えた検索）は読み直さない）。
    pub fn stack_layout(&self) -> Result<Arc<StackLayout>> {
        // 読む前に印を取る（読んでいる間にほかの接続が書き込めば、次に読み直す）。
        let mark = self.change_mark()?;
        if let Some(layout) = self.stack_cache.get(mark) {
            return Ok(layout);
        }
        let layout = Arc::new(self.load_stack_layout()?);
        self.stack_cache.set(mark, Arc::clone(&layout));
        Ok(layout)
    }

    /// カタログの変化の印（ほかの接続の書き込みで変わる `PRAGMA data_version` と、この接続で変えた行の数の
    /// 累計 `sqlite3_total_changes`。トリガー・外部キーの連鎖で変えた行も数える）。どちらかが変われば、
    /// どの表を変えたかによらず、覚えたものを使わない。
    fn change_mark(&self) -> Result<ChangeMark> {
        let data_version: i64 = self
            .conn
            .query_row("PRAGMA data_version", [], |r| r.get(0))?;
        Ok(ChangeMark {
            data_version,
            total_changes: self.conn.total_changes(),
        })
    }

    /// スタックの所属を読む（[`Catalog::stack_layout`]）。
    fn load_stack_layout(&self) -> Result<StackLayout> {
        let any: bool = self
            .conn
            .query_row("SELECT EXISTS(SELECT 1 FROM stack)", [], |r| r.get(0))?;
        if !any {
            return Ok(StackLayout::default());
        }
        let mut layout = StackLayout::default();
        // スタックの ID → 番号（`StackLayout::stacks` の添字）。
        let mut index: IdMap<i64, u32> = IdMap::default();
        {
            let mut stmt = self
                .conn
                .prepare_cached("SELECT id, top_variant_id, collapsed FROM stack")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?;
            for r in rows {
                let (id, top, collapsed) = r?;
                index.insert(id, layout.stacks.len() as u32);
                layout.stacks.push((top.map(VariantId::new), collapsed));
            }
        }
        let mut stmt = self
            .conn
            .prepare_cached("SELECT variant_id, stack_id, position FROM stack_member")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for r in rows {
            let (v, s, pos) = r?;
            // 外部キーでスタックは必ずある（ないものは飛ばす）。
            if let Some(&k) = index.get(&s) {
                layout.members.insert(VariantId::new(v), (k, pos));
            }
        }
        Ok(layout)
    }

    /// 条件で検索し、スタックでまとめる（[`Catalog::search`] ＋ [`StackLayout::group`]）。
    pub fn search_stacked(
        &self,
        filter: &Filter,
        sort: &Sort,
        expand_all: bool,
    ) -> Result<GroupedIds> {
        let ids = self.search(filter, sort)?;
        Ok(self.stack_layout()?.group(ids, expand_all))
    }

    /// 撮影時刻の間隔で自動的にスタックを作る（モジュールの doc）。`gap` は [`MAX_AUTO_STACK_GAP`] まで。
    /// `dry_run` なら数えるだけで何も変えない（画面の見込みの表示）。作るスタックは折りたたみ、一番上の写真は
    /// 撮影日時の最も早いもの。すべて 1 つのトランザクションで作る。
    pub fn auto_stack(
        &mut self,
        filter: &Filter,
        gap: Duration,
        dry_run: bool,
    ) -> Result<AutoStackReport> {
        if gap > MAX_AUTO_STACK_GAP {
            return Err(CatalogError::InvalidInput(format!(
                "間隔は {} 秒までです",
                MAX_AUTO_STACK_GAP.as_secs()
            )));
        }
        let gap_ms = i64::try_from(gap.as_millis()).unwrap_or(i64::MAX);
        let q = build_search_sql(&self.conn, filter, &Sort::default())?;
        let sql = format!(
            "SELECT q.id, a.captured_at_utc,
                    EXISTS(SELECT 1 FROM stack_member m WHERE m.variant_id = q.id)
             FROM ({}) q
             JOIN variant v ON v.id = q.id
             JOIN asset a ON a.id = v.asset_id",
            q.sql
        );
        let mut report = AutoStackReport::default();
        let mut timed: Vec<(i64, i64)> = Vec::new();
        {
            let mut stmt = self.conn.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(q.values.iter()))?;
            while let Some(row) = rows.next()? {
                let id: i64 = row.get(0)?;
                let utc: Option<String> = row.get(1)?;
                let stacked: bool = row.get(2)?;
                if stacked {
                    report.skipped_in_stacks += 1;
                    continue;
                }
                let Some(utc) = utc else {
                    report.skipped_without_time += 1;
                    continue;
                };
                timed.push((parse_db_utc(&utc)?.timestamp_millis(), id));
            }
        }
        timed.sort_unstable();
        report.considered = timed.len() as u64;
        let mut runs: Vec<Vec<VariantId>> = Vec::new();
        let mut run: Vec<VariantId> = Vec::new();
        let mut last: Option<i64> = None;
        for &(t, id) in &timed {
            if last.is_some_and(|l| t.saturating_sub(l) > gap_ms) {
                if run.len() >= 2 {
                    runs.push(std::mem::take(&mut run));
                } else {
                    run.clear();
                }
            }
            run.push(VariantId::new(id));
            last = Some(t);
        }
        if run.len() >= 2 {
            runs.push(run);
        }
        report.stacks = runs.len() as u64;
        report.photos = runs.iter().map(|r| r.len() as u64).sum();
        if dry_run || runs.is_empty() {
            return Ok(report);
        }
        let tx = self.conn.transaction()?;
        for r in &runs {
            insert_stack(&tx, r, true)?;
        }
        tx.commit()?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(n: i64) -> VariantId {
        VariantId::new(n)
    }

    /// スタック 1（10・11・12。一番上 10）と 2（20・21。一番上 21）の所属。
    fn layout(collapsed1: bool, collapsed2: bool) -> StackLayout {
        StackLayout::from_stacks(&[
            StackInfo {
                id: StackId::new(1),
                top_variant_id: v(10),
                collapsed: collapsed1,
                members: vec![v(10), v(11), v(12)],
            },
            StackInfo {
                id: StackId::new(2),
                top_variant_id: v(21),
                collapsed: collapsed2,
                members: vec![v(21), v(20)],
            },
        ])
    }

    fn ids(list: &[i64]) -> Vec<VariantId> {
        list.iter().map(|&n| v(n)).collect()
    }

    #[test]
    fn collapsed_stacks_show_one_representative_at_the_top_position() {
        let l = layout(true, true);
        // 並べ替えで一番上の写真（10）が 11 より後ろにあっても、スタックは一番上の写真の位置に 1 枚だけ出す。
        let g = l.group(ids(&[1, 11, 2, 10, 12, 20, 3, 21]), false);
        assert_eq!(g.ids, ids(&[1, 2, 10, 3, 21]));
        assert_eq!(g.hidden, 3);
    }

    #[test]
    fn representative_is_the_first_match_when_the_top_does_not_match() {
        let l = layout(true, true);
        // 一番上の写真（10）が条件に合わない: 最初に一致した写真（12）を出す。
        let g = l.group(ids(&[12, 1, 11]), false);
        assert_eq!(g.ids, ids(&[12, 1]));
        assert_eq!(g.hidden, 1);
    }

    #[test]
    fn expanded_stacks_keep_members_together_in_stack_order() {
        let l = layout(false, true);
        let g = l.group(ids(&[12, 1, 10, 20, 2, 11, 21]), false);
        // スタック 1 は一番上の写真（10）の位置に、スタックの中の順（10・11・12）で並ぶ。
        assert_eq!(g.ids, ids(&[1, 10, 11, 12, 2, 21]));
        assert_eq!(g.hidden, 1);
        // すべて展開する指定では、折りたたんだスタックも並べる。
        let g = l.group(ids(&[12, 1, 10, 20, 2, 11, 21]), true);
        assert_eq!(g.ids, ids(&[1, 10, 11, 12, 2, 21, 20]));
        assert_eq!(g.hidden, 0);
    }

    /// 規則をそのまま書いた遅い版（スタックごとに一致した写真を集め、置く位置に並べる）。
    fn group_naive(stacks: &[StackInfo], ids: &[VariantId], expand_all: bool) -> GroupedIds {
        let mut out = Vec::new();
        let mut hidden = 0u64;
        for (i, &v) in ids.iter().enumerate() {
            let Some(s) = stacks.iter().find(|s| s.members.contains(&v)) else {
                out.push(v);
                continue;
            };
            let matched: Vec<usize> = (0..ids.len())
                .filter(|&j| s.members.contains(&ids[j]))
                .collect();
            let anchor = matched
                .iter()
                .copied()
                .find(|&j| ids[j] == s.top_variant_id)
                .unwrap_or(matched[0]);
            if anchor != i {
                continue;
            }
            if s.collapsed && !expand_all {
                out.push(v);
                hidden += (matched.len() - 1) as u64;
            } else {
                out.extend(s.members.iter().filter(|m| ids.contains(m)).copied());
            }
        }
        GroupedIds { ids: out, hidden }
    }

    #[test]
    fn grouping_matches_the_rule_written_plainly() {
        // 決まった疑似乱数（線形合同法）で、スタック・検索結果の順・一致する写真を変えて比べる。
        let mut seed = 12_345u64;
        let mut next = |m: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (seed >> 33) % m
        };
        for round in 0..200 {
            let n = 1 + next(40) as i64;
            let mut stacks = Vec::new();
            let mut free: Vec<i64> = (1..=n).collect();
            let mut sid = 1;
            while free.len() >= 2 && next(3) > 0 {
                let size = 2 + next(4.min(free.len() as u64 - 1)) as usize;
                let members: Vec<VariantId> = (0..size)
                    .map(|_| v(free.remove(next(free.len() as u64) as usize)))
                    .collect();
                stacks.push(StackInfo {
                    id: StackId::new(sid),
                    top_variant_id: members[0],
                    collapsed: next(2) == 0,
                    members,
                });
                sid += 1;
            }
            // 検索結果: 一部の写真だけを、ばらばらの順で。
            let mut all: Vec<i64> = (1..=n).collect();
            let mut result = Vec::new();
            while !all.is_empty() {
                let x = all.remove(next(all.len() as u64) as usize);
                if next(4) > 0 {
                    result.push(v(x));
                }
            }
            let layout = StackLayout::from_stacks(&stacks);
            for expand in [false, true] {
                assert_eq!(
                    layout.group(result.clone(), expand),
                    group_naive(&stacks, &result, expand),
                    "round {round} expand {expand}"
                );
            }
        }
    }

    #[test]
    fn without_stacks_the_order_is_unchanged() {
        let l = StackLayout::default();
        assert!(l.is_empty());
        let g = l.group(ids(&[3, 1, 2]), false);
        assert_eq!(g.ids, ids(&[3, 1, 2]));
        assert_eq!(g.hidden, 0);
        // スタックの写真を含まない結果もそのまま。
        let g = layout(true, true).group(ids(&[3, 1, 2]), false);
        assert_eq!(g.ids, ids(&[3, 1, 2]));
    }
}
