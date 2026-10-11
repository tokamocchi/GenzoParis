//! キャッシュキー（docs/04_architecture.md の 4.1 節。レビュー R-09）。
//!
//! 段階 A1 / B の中間キャッシュ、L0（サムネイル）、L1（標準プレビュー）で共通の規則。
//! 設定や入力が変わるとキーが変わるため、古いキャッシュは使われなくなる。

use serde::{Deserialize, Serialize};

use crate::ids::FileId;
use crate::{DevelopSettings, Phase};

/// 描画の品質。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderQuality {
    /// 簡易（ドラッグ中の 2 × 2 の簡易処理など。04 の 2.2 節）。
    Draft,
    /// 最終品質。
    Final,
}

/// キャッシュの種類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheKind {
    /// L0: サムネイル（長辺 320px の JPEG。thumbs.db）。
    L0Thumb,
    /// L1: 標準プレビュー（長辺 2560px の JPEG。ファイル）。
    L1Preview,
    /// 段階 A1 の中間結果（プレビュー解像度の作業色空間の画像）。
    A1Intermediate,
    /// 段階 B のガイド（長辺 512px 程度の低解像度の画像）。
    BGuide,
}

impl CacheKind {
    /// このキャッシュを作るのに必要な段階（develop_hash をどの段階まで取るか）。
    pub const fn phase(self) -> Phase {
        match self {
            CacheKind::L0Thumb | CacheKind::L1Preview => Phase::C,
            CacheKind::A1Intermediate => Phase::A1,
            CacheKind::BGuide => Phase::B,
        }
    }
}

/// キャッシュの色空間（04 の 2.6 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheColorSpace {
    /// B5: Display P3（D65、IEC 61966-2-1 の伝達関数）。L0 / L1 で使う。
    DisplayP3,
    /// B2: リニア Rec.2020（D65）。段階 A1 の中間結果で使う。
    LinearRec2020,
    /// ガイドの値（輝度の log2）。色空間ではないが、区別のために置く。
    Log2Luminance,
}

/// キャッシュの形式（色空間・ファイル形式・形式のバージョン）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheFormat {
    /// キャッシュの種類。
    pub kind: CacheKind,
    /// 色空間。
    pub color_space: CacheColorSpace,
    /// 保存形式のバージョン。保存のしかた（JPEG の品質、データの並びなど）を変えたら上げる。
    pub format_version: u32,
}

/// キャッシュキー（04 の 4.1 節）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheKey {
    /// 元ファイル。
    pub file_id: FileId,
    /// 元ファイルの内容のリビジョン（内容の変化を検知するたびに +1。3.3 節）。
    pub file_revision: u32,
    /// 処理バージョン。
    pub process_version: u32,
    /// 外部データ（カメラ行列・レンズデータ・デコーダ）のハッシュ（[`RenderDeps::hash`](crate::RenderDeps::hash)）。
    pub render_deps_hash: [u8; 32],
    /// その段階までに影響する現像設定だけのハッシュ（[`DevelopSettings::hash_for_phase`]）。
    pub develop_hash: [u8; 32],
    /// 出力の寸法（幅, 高さ）。長辺だけで決まる場合は (長辺, 0) などと使い方を決めて使う。
    pub size: (u32, u32),
    /// 品質。
    pub quality: RenderQuality,
    /// 形式。
    pub format: CacheFormat,
}

/// キーのバイト列の前に付ける、用途とキーの形のバージョンを示す文字列。
/// キーの項目を変えたら末尾の番号を上げる。
const DOMAIN: &[u8] = b"genzo.cache_key.v1\0";

impl CacheKey {
    /// 現像設定からキーを作る。`develop_hash` は `format.kind` の段階までのハッシュ、
    /// `process_version` と `render_deps_hash` は丸めた設定から取る。
    pub fn for_settings(
        file_id: FileId,
        file_revision: u32,
        settings: &DevelopSettings,
        size: (u32, u32),
        quality: RenderQuality,
        format: CacheFormat,
    ) -> Self {
        let normalized = settings.normalized();
        Self {
            file_id,
            file_revision,
            process_version: normalized.process_version,
            render_deps_hash: normalized.render_deps.hash(),
            develop_hash: normalized.hash_for_phase(format.kind.phase()),
            size,
            quality,
            format,
        }
    }

    /// キーのダイジェスト（SHA-256）。項目を決まった順に、決まった形のバイト列にして計算する。
    ///
    /// 数値はリトルエンディアン、列挙は固定の番号で表す（Rust の型の並びや serde の表現が
    /// 変わっても値が変わらないようにするため）。
    pub fn digest(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(128);
        buf.extend_from_slice(DOMAIN);
        buf.extend_from_slice(&self.file_id.get().to_le_bytes());
        buf.extend_from_slice(&self.file_revision.to_le_bytes());
        buf.extend_from_slice(&self.process_version.to_le_bytes());
        buf.extend_from_slice(&self.render_deps_hash);
        buf.extend_from_slice(&self.develop_hash);
        buf.extend_from_slice(&self.size.0.to_le_bytes());
        buf.extend_from_slice(&self.size.1.to_le_bytes());
        buf.push(match self.quality {
            RenderQuality::Draft => 0,
            RenderQuality::Final => 1,
        });
        buf.push(match self.format.kind {
            CacheKind::L0Thumb => 0,
            CacheKind::L1Preview => 1,
            CacheKind::A1Intermediate => 2,
            CacheKind::BGuide => 3,
        });
        buf.push(match self.format.color_space {
            CacheColorSpace::DisplayP3 => 0,
            CacheColorSpace::LinearRec2020 => 1,
            CacheColorSpace::Log2Luminance => 2,
        });
        buf.extend_from_slice(&self.format.format_version.to_le_bytes());
        crate::develop::sha256(&[&buf])
    }

    /// ダイジェストの 16 進数（小文字 64 文字）。L1 のファイル名などに使う。
    pub fn hex(&self) -> String {
        hex::encode(self.digest())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format() -> CacheFormat {
        CacheFormat {
            kind: CacheKind::L1Preview,
            color_space: CacheColorSpace::DisplayP3,
            format_version: 1,
        }
    }

    fn key() -> CacheKey {
        CacheKey::for_settings(
            FileId::new(7),
            1,
            &DevelopSettings::default(),
            (2560, 1707),
            RenderQuality::Final,
            format(),
        )
    }

    #[test]
    fn digest_is_deterministic_and_hex_is_64_chars() {
        assert_eq!(key().digest(), key().digest());
        let hex = key().hex();
        assert_eq!(hex.len(), 64);
        assert!(
            hex.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn every_field_changes_the_digest() {
        let base = key();
        let mut variants = Vec::new();
        let mut k = base;
        k.file_id = FileId::new(8);
        variants.push(k);
        let mut k = base;
        k.file_revision = 2;
        variants.push(k);
        let mut k = base;
        k.process_version = 2;
        variants.push(k);
        let mut k = base;
        k.render_deps_hash[0] ^= 1;
        variants.push(k);
        let mut k = base;
        k.develop_hash[31] ^= 1;
        variants.push(k);
        let mut k = base;
        k.size = (1707, 2560);
        variants.push(k);
        let mut k = base;
        k.quality = RenderQuality::Draft;
        variants.push(k);
        let mut k = base;
        k.format.kind = CacheKind::L0Thumb;
        variants.push(k);
        let mut k = base;
        k.format.color_space = CacheColorSpace::LinearRec2020;
        variants.push(k);
        let mut k = base;
        k.format.format_version = 2;
        variants.push(k);
        let mut digests: Vec<[u8; 32]> = variants.iter().map(CacheKey::digest).collect();
        digests.push(base.digest());
        let n = digests.len();
        digests.sort();
        digests.dedup();
        assert_eq!(digests.len(), n);
    }

    #[test]
    fn for_settings_uses_the_phase_of_the_cache_kind() {
        let mut s = DevelopSettings::default();
        let a1 = |s: &DevelopSettings| {
            CacheKey::for_settings(
                FileId::new(1),
                0,
                s,
                (2560, 1707),
                RenderQuality::Final,
                CacheFormat {
                    kind: CacheKind::A1Intermediate,
                    color_space: CacheColorSpace::LinearRec2020,
                    format_version: 1,
                },
            )
        };
        let before = a1(&s);
        assert_eq!(before.develop_hash, s.hash_for_phase(Phase::A1));
        // 露光量は段階 A1 の中間キャッシュのキーを変えない。
        s.exposure_ev = 1.0;
        assert_eq!(a1(&s).digest(), before.digest());
        // L1 のキーは変わる。
        let l1_before = CacheKey::for_settings(
            FileId::new(1),
            0,
            &DevelopSettings::default(),
            (2560, 1707),
            RenderQuality::Final,
            format(),
        );
        let l1_after = CacheKey::for_settings(
            FileId::new(1),
            0,
            &s,
            (2560, 1707),
            RenderQuality::Final,
            format(),
        );
        assert_ne!(l1_before.digest(), l1_after.digest());
        assert_eq!(l1_after.develop_hash, s.develop_hash());
    }

    #[test]
    fn cache_kind_phases() {
        assert_eq!(CacheKind::L0Thumb.phase(), Phase::C);
        assert_eq!(CacheKind::A1Intermediate.phase(), Phase::A1);
        assert_eq!(CacheKind::BGuide.phase(), Phase::B);
    }
}
