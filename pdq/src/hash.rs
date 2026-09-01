//! 256 ビットハッシュ値の表現。
//!
//! `facebook/ThreatExchange` の `pdq/cpp/common/pdqhashtypes.{h,cpp}` からの移植。
//! Copyright (c) Meta Platforms, Inc. and affiliates. BSD 3-Clause (../LICENSE)。

use std::fmt;

/// 256 ビットの PDQ ハッシュ。
///
/// リファレンス実装 (`pdq/cpp/common/pdqhashtypes.h`) の `Hash256` と同じく
/// 16 ビットワード 16 本で保持する。ビット `k` はワード `k >> 4` のビット `k & 15`。
/// 16 進表記は上位ワードから、すなわち `w[15]` から `w[0]` の順に 4 桁ずつ並べる。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Hash256 {
    w: [u16; 16],
}

impl Hash256 {
    /// 全ビット 0 のハッシュ。ハッシュ不能な入力に対して返る値でもある。
    pub const ZERO: Self = Self { w: [0; 16] };

    /// PDQ 公式の「同一画像とみなす」しきい値。256 ビット中 31 ビット。
    pub const MATCH_THRESHOLD: u32 = 31;

    #[inline]
    pub fn get_bit(&self, k: usize) -> bool {
        (self.w[(k & 255) >> 4] >> (k & 15)) & 1 == 1
    }

    #[inline]
    pub fn set_bit(&mut self, k: usize) {
        self.w[(k & 255) >> 4] |= 1 << (k & 15);
    }

    /// 立っているビット数。
    pub fn hamming_norm(&self) -> u32 {
        self.w.iter().map(|x| x.count_ones()).sum()
    }

    /// 2 つのハッシュのハミング距離 (0..=256)。
    ///
    /// 距離が [`Hash256::MATCH_THRESHOLD`] 以下なら同一画像の変種とみなすのが
    /// PDQ の推奨運用。
    pub fn hamming_distance(&self, other: &Self) -> u32 {
        self.w
            .iter()
            .zip(other.w.iter())
            .map(|(a, b)| (a ^ b).count_ones())
            .sum()
    }

    /// リファレンス実装と同じ 64 桁の 16 進表記。[`Display`](fmt::Display) と同じ。
    pub fn to_hex(&self) -> String {
        self.to_string()
    }

    /// [`Hash256::to_hex`] の逆。
    pub fn from_hex(s: &str) -> Result<Self, ParseHashError> {
        if s.len() != 64 {
            return Err(ParseHashError);
        }
        let mut w = [0u16; 16];
        for (i, chunk) in s.as_bytes().chunks_exact(4).enumerate() {
            let text = std::str::from_utf8(chunk).map_err(|_| ParseHashError)?;
            // 16 進表記は w[15] から並ぶので、書き戻す添字は逆順。
            w[15 - i] = u16::from_str_radix(text, 16).map_err(|_| ParseHashError)?;
        }
        Ok(Self { w })
    }

    /// 16 ビットワード列としての生表現。
    pub fn words(&self) -> &[u16; 16] {
        &self.w
    }
}

impl fmt::Display for Hash256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for word in self.w.iter().rev() {
            write!(f, "{word:04x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Hash256 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hash256({self})")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseHashError;

impl fmt::Display for ParseHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PDQ ハッシュは 64 桁の 16 進文字列である必要があります")
    }
}

impl std::error::Error for ParseHashError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// リファレンスの期待値ファイルから取った実在のハッシュで往復する。
    #[test]
    fn hex_roundtrip() {
        let hex = "d8f8f0cee0f4a84f0637022a078f67f0b36e2ed596621e1d33e6339c4e9c9b22";
        assert_eq!(Hash256::from_hex(hex).unwrap().to_hex(), hex);
    }

    /// 16 進表記は上位ワードから並ぶので、ビット 0 は末尾の桁に現れる。
    #[test]
    fn hex_puts_the_lowest_bit_last() {
        let mut hash = Hash256::ZERO;
        hash.set_bit(0);
        assert_eq!(hash.to_hex(), "0".repeat(63) + "1");

        let mut hash = Hash256::ZERO;
        hash.set_bit(255);
        assert_eq!(hash.to_hex(), "8".to_string() + &"0".repeat(63));
    }

    #[test]
    fn rejects_malformed_hex() {
        assert_eq!(Hash256::from_hex(""), Err(ParseHashError));
        assert_eq!(Hash256::from_hex(&"0".repeat(63)), Err(ParseHashError));
        assert_eq!(Hash256::from_hex(&"0".repeat(65)), Err(ParseHashError));
        assert_eq!(Hash256::from_hex(&"z".repeat(64)), Err(ParseHashError));
    }

    #[test]
    fn bit_accessors_agree() {
        let mut hash = Hash256::ZERO;
        assert_eq!(hash.hamming_norm(), 0);

        hash.set_bit(0);
        hash.set_bit(255);
        assert_eq!(hash.hamming_norm(), 2);
        assert!(hash.get_bit(0) && hash.get_bit(255));
        assert!(!hash.get_bit(1) && !hash.get_bit(254));
    }

    #[test]
    fn hamming_distance_counts_differing_bits() {
        let mut a = Hash256::ZERO;
        a.set_bit(0);
        a.set_bit(255);

        assert_eq!(a.hamming_distance(&a), 0);
        assert_eq!(a.hamming_distance(&Hash256::ZERO), 2);

        // 全ビットが違えば 256。
        let all_ones = Hash256::from_hex(&"f".repeat(64)).unwrap();
        assert_eq!(all_ones.hamming_norm(), 256);
        assert_eq!(all_ones.hamming_distance(&Hash256::ZERO), 256);
    }

    /// 全 256 ビットについて、立てたビットが 16 進表記を往復しても保たれる。
    #[test]
    fn every_bit_survives_a_hex_roundtrip() {
        for k in 0..256 {
            let mut hash = Hash256::ZERO;
            hash.set_bit(k);
            let restored = Hash256::from_hex(&hash.to_hex()).unwrap();
            assert_eq!(restored, hash, "ビット {k} が往復で壊れました");
            assert!(restored.get_bit(k));
        }
    }
}
