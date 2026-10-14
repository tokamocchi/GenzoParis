//! 階層キーワード（LIB-09）・コレクション（LIB-10）・スマートコレクション（LIB-11）の操作
//! （docs/04_architecture.md の 3.9 節）。
//!
//! - キーワード・コレクションを変えたら [`Event::CatalogChanged`]（[`CatalogChange::Keywords`]・
//!   [`CatalogChange::Collections`]）で知らせ、表示中の検索結果を作り直す（3.7 節。キーワード・
//!   コレクションでの絞り込みや、スマートコレクションの結果が変わりうるため）。グリッドのセルの内容
//!   （評価など）は変わらないので、写真の付与・メンバーの変更では対象の写真だけ、キーワード・コレクション
//!   そのものの変更（名前・移動・削除）では写真を挙げずに知らせる（`all` は偽）。
//! - 選別の操作（評価など）と違い、条件に合わなくなった写真を次の検索まで残すことはしない（コレクション
//!   から外した写真は、すぐにそのコレクションの表示から消える。Lightroom と同じ）。
//! - スマートコレクションの写真の数・条件の編集中の件数は、検索（[`genzo_catalog::Catalog::count`]）で
//!   数える。50 万件のダミーのカタログでは 1 つの条件で約 0.1〜0.4 秒（PoC-6 の計測。genzo-catalog の
//!   `tests/dummy.rs`）かかるので、数えている間に評価の変更などを待たせないよう、読み取り専用の接続
//!   （WAL なので書き込みと並行して読める。検索結果の作り直しと同じ）で数える（開けなければ本体の接続）。

use genzo_model::{CollectionId, CollectionKind, KeywordId, SmartRule, VariantId};

use crate::core::{Core, Inner};
use crate::error::ApiError;
use crate::events::{CatalogChange, Event};
use crate::types::{
    CollectionInfo, KeywordHit, KeywordInfo, KeywordUsage, SearchFilter, VariantKeyword,
    VariantKeywords,
};

/// 補完の候補の数の上限（**仮置き**: 50。画面に出す数）。
pub const MAX_KEYWORD_HITS: usize = 50;

/// 読み取り専用の接続で読む（開けなければ本体の接続。数えるのに時間のかかる問い合わせ用）。
fn with_reader<T>(
    inner: &Inner,
    f: impl Fn(&genzo_catalog::Catalog) -> genzo_catalog::Result<T>,
) -> Result<T, ApiError> {
    match genzo_catalog::Catalog::open_read_only(&inner.config.catalog_path) {
        Ok(reader) => Ok(f(&reader)?),
        Err(e) => {
            tracing::debug!(error = %e, "読み取り専用の接続を開けないため、本体の接続で数えます");
            inner.with_catalog(|c| f(c))
        }
    }
}

/// キーワード・コレクションを変えた後: 知らせて、表示中の検索結果を作り直す。
fn after_change(inner: &Inner, change: CatalogChange, ids: &[VariantId]) {
    inner.events.emit(Event::CatalogChanged {
        change,
        variant_ids: ids.to_vec(),
        all: false,
    });
    crate::search::refresh(inner, &[]);
}

impl Core {
    // -----------------------------------------------------------------------
    // キーワード（LIB-09）
    // -----------------------------------------------------------------------

    /// すべてのキーワード（名前の順）と、直接付いている写真の数。
    pub fn keyword_tree(&self) -> Result<Vec<KeywordInfo>, ApiError> {
        self.inner.check_open()?;
        Ok(self
            .inner
            .with_catalog(|c| c.keyword_tree())?
            .into_iter()
            .map(KeywordInfo::from)
            .collect())
    }

    /// 名前でキーワードを探す（部分一致。前方一致のものを先に。NFKC ＋ 小文字化で比較。入力欄の補完用）。
    /// `limit` は [`MAX_KEYWORD_HITS`] まで。
    pub fn find_keywords(&self, query: &str, limit: usize) -> Result<Vec<KeywordHit>, ApiError> {
        self.inner.check_open()?;
        let limit = limit.clamp(1, MAX_KEYWORD_HITS);
        self.inner.with_catalog(|c| {
            let mut found = c.find_keywords(query, genzo_catalog::KeywordMatch::Contains)?;
            found.truncate(limit);
            let ids: Vec<KeywordId> = found.iter().map(|k| k.id).collect();
            let mut paths = c.keyword_paths(&ids)?;
            Ok(found
                .into_iter()
                .map(|k| KeywordHit {
                    path: paths.remove(&k.id).unwrap_or_else(|| vec![k.name.clone()]),
                    keyword_id: k.id,
                    name: k.name,
                })
                .collect())
        })
    }

    /// `parent` の下（`None` で最上位）に、階層のパス（`["場所", "京都"]`）でキーワードを確保する
    /// （あればそれを使い、なければ作る）。最後のキーワードを返す。
    pub fn create_keyword(
        &self,
        parent: Option<KeywordId>,
        path: &[String],
    ) -> Result<KeywordId, ApiError> {
        self.inner.check_open()?;
        let id = self
            .inner
            .with_catalog(|c| c.ensure_keyword_path_in(parent, path))?;
        after_change(&self.inner, CatalogChange::Keywords, &[]);
        Ok(id)
    }

    /// キーワードの名前を変える（同じ階層に同じ名前があれば種類 `conflict` のエラー）。
    pub fn rename_keyword(&self, id: KeywordId, name: &str) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner.with_catalog(|c| c.rename_keyword(id, name))?;
        after_change(&self.inner, CatalogChange::Keywords, &[]);
        Ok(())
    }

    /// キーワードを別の親の下へ移す（`None` で最上位。自分・子孫の下へは移せない）。
    pub fn move_keyword(&self, id: KeywordId, parent: Option<KeywordId>) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner.with_catalog(|c| c.move_keyword(id, parent))?;
        after_change(&self.inner, CatalogChange::Keywords, &[]);
        Ok(())
    }

    /// キーワードを削除する（子のキーワードと、写真への付与も）。削除したキーワードの数を返す。
    pub fn delete_keyword(&self, id: KeywordId) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let n = self.inner.with_catalog(|c| c.delete_keyword(id))?;
        after_change(&self.inner, CatalogChange::Keywords, &[]);
        Ok(n)
    }

    /// 書き出すファイルのメタデータにキーワードを書くかを変える。
    pub fn set_keyword_export(&self, id: KeywordId, include: bool) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner
            .with_catalog(|c| c.set_keyword_include_on_export(id, include))?;
        after_change(&self.inner, CatalogChange::Keywords, &[]);
        Ok(())
    }

    /// キーワードの使われ方（削除の確認）。
    pub fn keyword_usage(&self, id: KeywordId) -> Result<KeywordUsage, ApiError> {
        self.inner.check_open()?;
        Ok(self.inner.with_catalog(|c| c.keyword_usage(id))?.into())
    }

    /// 写真にキーワードを付ける（1 つのトランザクション。失敗したら、パスで作ったキーワードも残さない）。
    /// `keyword_ids` は既存のキーワード、`paths` は階層のパス（なければ作る）。新しく付けた
    /// （写真, キーワード）の組の数を返す。
    pub fn add_keywords(
        &self,
        variant_ids: &[VariantId],
        keyword_ids: &[KeywordId],
        paths: &[Vec<String>],
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let (_, n) = self
            .inner
            .with_catalog(|c| c.add_keywords_with_paths(variant_ids, keyword_ids, paths))?;
        after_change(&self.inner, CatalogChange::Keywords, variant_ids);
        Ok(n)
    }

    /// 写真からキーワードを外す（子のキーワードは外さない）。外した組の数を返す。
    pub fn remove_keywords(
        &self,
        variant_ids: &[VariantId],
        keyword_ids: &[KeywordId],
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let n = self
            .inner
            .with_catalog(|c| c.remove_keywords(variant_ids, keyword_ids))?;
        after_change(&self.inner, CatalogChange::Keywords, variant_ids);
        Ok(n)
    }

    /// 写真に付いているキーワードと、それぞれが付いている写真の数（画面のキーワードの欄・一覧の印）。
    pub fn variant_keywords(&self, variant_ids: &[VariantId]) -> Result<VariantKeywords, ApiError> {
        self.inner.check_open()?;
        let mut unique: Vec<VariantId> = variant_ids.to_vec();
        unique.sort();
        unique.dedup();
        self.inner.with_catalog(|c| {
            let found = c.keywords_of_variants(&unique)?;
            let ids: Vec<KeywordId> = found.iter().map(|(k, _)| k.id).collect();
            let mut paths = c.keyword_paths(&ids)?;
            Ok(VariantKeywords {
                variants: unique.len() as u64,
                keywords: found
                    .into_iter()
                    .map(|(k, count)| VariantKeyword {
                        path: paths.remove(&k.id).unwrap_or_else(|| vec![k.name.clone()]),
                        keyword_id: k.id,
                        name: k.name,
                        count,
                    })
                    .collect(),
            })
        })
    }

    // -----------------------------------------------------------------------
    // コレクション（LIB-10・LIB-11）
    // -----------------------------------------------------------------------

    /// すべてのコレクション（名前の順）。`with_counts` なら、スマートコレクションとセットの写真の数も
    /// 検索で数える（手動のコレクションの数は常に入れる）。
    pub fn collections(&self, with_counts: bool) -> Result<Vec<CollectionInfo>, ApiError> {
        self.inner.check_open()?;
        let list = self.inner.with_catalog(|c| c.collections())?;
        let counted: Vec<Option<u64>> = if with_counts {
            let ids: Vec<Option<CollectionId>> = list
                .iter()
                .map(|c| (c.kind != CollectionKind::Manual).then_some(c.id))
                .collect();
            with_reader(&self.inner, |r| {
                Ok(ids
                    .iter()
                    .map(|id| {
                        id.and_then(|id| {
                            r.count(&genzo_catalog::Filter {
                                collection: Some(id),
                                ..Default::default()
                            })
                            .ok()
                        })
                    })
                    .collect())
            })?
        } else {
            vec![None; list.len()]
        };
        Ok(list
            .into_iter()
            .zip(counted)
            .map(|(col, counted)| CollectionInfo {
                count: if col.kind == CollectionKind::Manual {
                    Some(col.member_count)
                } else {
                    counted
                },
                collection_id: col.id,
                parent_id: col.parent_id,
                name: col.name,
                kind: col.kind,
                rule: col.rule,
                rule_error: col.rule_error,
            })
            .collect())
    }

    /// コレクションを作る（`parent` はコレクションセット）。手動のコレクションには `variant_ids` を入れる
    /// （同じトランザクションではない。入れられなければエラーで、作ったコレクションは残る）。
    pub fn create_collection(
        &self,
        parent: Option<CollectionId>,
        name: &str,
        kind: CollectionKind,
        rule: Option<&SmartRule>,
        variant_ids: &[VariantId],
    ) -> Result<CollectionId, ApiError> {
        self.inner.check_open()?;
        if kind != CollectionKind::Manual && !variant_ids.is_empty() {
            return Err(ApiError::InvalidArgument(
                "写真を入れられるのは手動のコレクションだけです".to_owned(),
            ));
        }
        let id = self
            .inner
            .with_catalog(|c| c.create_collection(parent, name, kind, rule))?;
        if !variant_ids.is_empty() {
            let r = self
                .inner
                .with_catalog(|c| c.add_to_collection(id, variant_ids));
            after_change(&self.inner, CatalogChange::Collections, variant_ids);
            r?;
        } else {
            after_change(&self.inner, CatalogChange::Collections, &[]);
        }
        Ok(id)
    }

    /// コレクションの名前を変える。
    pub fn rename_collection(&self, id: CollectionId, name: &str) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner.with_catalog(|c| c.rename_collection(id, name))?;
        after_change(&self.inner, CatalogChange::Collections, &[]);
        Ok(())
    }

    /// コレクションを別のコレクションセットへ移す（`None` で最上位）。
    pub fn move_collection(
        &self,
        id: CollectionId,
        parent: Option<CollectionId>,
    ) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner.with_catalog(|c| c.move_collection(id, parent))?;
        after_change(&self.inner, CatalogChange::Collections, &[]);
        Ok(())
    }

    /// コレクションを削除する（セットなら中のコレクションも。写真は消えない）。削除した数を返す。
    pub fn delete_collection(&self, id: CollectionId) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let n = self.inner.with_catalog(|c| c.delete_collection(id))?;
        after_change(&self.inner, CatalogChange::Collections, &[]);
        Ok(n)
    }

    /// スマートコレクションの条件を変える。
    pub fn set_smart_rule(&self, id: CollectionId, rule: &SmartRule) -> Result<(), ApiError> {
        self.inner.check_open()?;
        self.inner.with_catalog(|c| c.set_smart_rule(id, rule))?;
        after_change(&self.inner, CatalogChange::Collections, &[]);
        Ok(())
    }

    /// 手動のコレクションに写真を入れる。新しく入れた数を返す。
    pub fn add_to_collection(
        &self,
        id: CollectionId,
        variant_ids: &[VariantId],
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let n = self
            .inner
            .with_catalog(|c| c.add_to_collection(id, variant_ids))?;
        after_change(&self.inner, CatalogChange::Collections, variant_ids);
        Ok(n)
    }

    /// 手動のコレクションから写真を外す（写真は消えない）。外した数を返す。
    pub fn remove_from_collection(
        &self,
        id: CollectionId,
        variant_ids: &[VariantId],
    ) -> Result<usize, ApiError> {
        self.inner.check_open()?;
        let n = self
            .inner
            .with_catalog(|c| c.remove_from_collection(id, variant_ids))?;
        after_change(&self.inner, CatalogChange::Collections, variant_ids);
        Ok(n)
    }

    /// 写真が入っている手動のコレクション（名前の順）。
    pub fn variant_collections(
        &self,
        variant_id: VariantId,
    ) -> Result<Vec<CollectionId>, ApiError> {
        self.inner.check_open()?;
        self.inner
            .with_catalog(|c| c.collections_of_variant(variant_id))
    }

    /// 条件に合う写真の数（表示中の検索結果は変えない。スマートコレクションの条件の編集中の件数など）。
    /// 読み取り専用の接続で数える。
    pub fn count(&self, filter: &SearchFilter) -> Result<u64, ApiError> {
        self.inner.check_open()?;
        let filter = filter.to_catalog();
        with_reader(&self.inner, |c| c.count(&filter))
    }
}
