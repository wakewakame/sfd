//! PDQ 知覚ハッシュの pure Rust 実装。
//!
//! [facebook/ThreatExchange](https://github.com/facebook/ThreatExchange/tree/main/pdq)
//! の C++ リファレンス実装からの逐語移植で、公式の回帰テストベクタに対して
//! ビット単位で一致する (`tests/regression.rs`)。依存クレートなし。
//!
//! 移植元は Copyright (c) Meta Platforms, Inc. and affiliates、BSD 3-Clause。
//! どのファイルが何の派生物かは `LICENSE` にまとめてある。
//!
//! ハッシュ計算に加えて、ffmpeg から受け取ったフレームを扱うための最小限の道具
//! ([`netpbm`] パーサと [`preprocess`]) を持つ。sfd がこのクレートを直接使うことを
//! 想定しているので、汎用の画像クレートを引き込まずに済ませるためのもの。
//!
//! ```no_run
//! let image = pdq::Image { width: 640, height: 480, channels: 3, data: vec![0; 640 * 480 * 3] };
//! let (hash, quality) = pdq::Hasher::new().hash_image(&image).unwrap();
//! println!("{hash} quality={quality}");
//! ```
//!
//! # アルゴリズムの流れ
//!
//! 1. RGB を luma に変換する
//! 2. Jarosz フィルタ (box filter 2 往復) でぼかしてから 64x64 に間引く
//! 3. 64x64 に 2 次元 DCT をかけ、低周波側 16x16 を取り出す
//! 4. 256 個の係数の中央値を閾値に 2 値化して 256 ビットを得る
//!
//! 併せて、画像の勾配量から 0..=100 の quality を求める。のっぺりした画像ほど
//! 低くなり、低い値のハッシュは偶然の一致を起こしやすい。
//!
//! # ハッシュ値を他所と突き合わせるなら
//!
//! **このクレートのハッシュ関数は前処理を一切しない。** 一方リファレンス CLI
//! (`pdq-photo-hasher`) は、縦横どちらかが 512 を超える画像を 512x512 の最近傍縮小に
//! かけてからハッシュする。ThreatExchange が公開している期待値はすべてこの前処理
//! 込みの値なので、**それらと一致させたいなら [`preprocess::reference_downsample`]
//! を先に呼ぶこと。** 呼ばないと、実装が正しくても値が 2〜8 ビットずれる。
//!
//! 同梱の `phashsum` コマンドはこの前処理を既定で行う。ライブラリを直接使う側と
//! 挙動が違うので注意。
//!
//! 前処理には速度上の利点もある。手順 2 の計算量は入力画素数に比例するため、
//! 2400 万画素の写真では 363ms が 3.2ms になる。自前の DB 内で完結する用途でも、
//! 全ファイルに同じ前処理を揃えておく限り一貫するので使わない理由はあまりない。

mod dct;
mod downscale;
mod hash;
mod image;
mod median;

pub mod netpbm;
pub mod preprocess;

pub use hash::{Hash256, ParseHashError};
pub use image::Image;

use std::fmt;

/// リファレンス実装の `MIN_HASHABLE_DIM`。これ未満の画像はハッシュしない。
const MIN_HASHABLE_DIM: usize = 5;

/// リファレンス実装の `PDQ_NUM_JAROSZ_XY_PASSES` (tent filter)。
const NUM_JAROSZ_XY_PASSES: usize = 2;

// Wikipedia の標準的な RGB -> 輝度 (YUV の Y) 係数。
const LUMA_FROM_R: f32 = 0.299;
const LUMA_FROM_G: f32 = 0.587;
const LUMA_FROM_B: f32 = 0.114;

/// ハッシュできないチャンネル数の画像を渡された。
///
/// 受け付けるのは 1 (グレースケール) と 3 (RGB) だけ。アルファ付きは
/// [`Image::drop_alpha`] で落としてから渡す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnsupportedChannels(pub usize);

impl fmt::Display for UnsupportedChannels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "対応していないチャンネル数です: {} (1 か 3 のみ)", self.0)
    }
}

impl std::error::Error for UnsupportedChannels {}

/// 回転・反転を加えた 8 通りの二面体変換。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dihedral {
    Original,
    Rotate90,
    Rotate180,
    Rotate270,
    FlipX,
    FlipY,
    FlipPlus1,
    FlipMinus1,
}

impl Dihedral {
    /// 人間が読む用の名前。
    ///
    /// リファレンスの `pdq-photo-hasher --pdqdih --details` が使う略称
    /// (`orig`, `rot90`, `flipp`, `flipm` など) とは別物なので、
    /// その出力と突き合わせるときは変換表が要る (`tests/regression.rs` を参照)。
    pub fn name(self) -> &'static str {
        match self {
            Dihedral::Original => "original",
            Dihedral::Rotate90 => "rotate90",
            Dihedral::Rotate180 => "rotate180",
            Dihedral::Rotate270 => "rotate270",
            Dihedral::FlipX => "flipx",
            Dihedral::FlipY => "flipy",
            Dihedral::FlipPlus1 => "flip-plus-1",
            Dihedral::FlipMinus1 => "flip-minus-1",
        }
    }
}

/// 8 通りの二面体変換それぞれに対応するハッシュ。
///
/// 画像を回転させて計算し直しているわけではなく、DCT 係数の符号反転と転置だけで
/// 得ているので、追加コストはほぼゼロ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DihedralHashes {
    pub original: Hash256,
    pub rotate90: Hash256,
    pub rotate180: Hash256,
    pub rotate270: Hash256,
    pub flip_x: Hash256,
    pub flip_y: Hash256,
    pub flip_plus1: Hash256,
    pub flip_minus1: Hash256,
}

impl DihedralHashes {
    /// リファレンス実装の出力順で全 8 種を返す。
    pub fn all(&self) -> [(Dihedral, Hash256); 8] {
        [
            (Dihedral::Original, self.original),
            (Dihedral::Rotate90, self.rotate90),
            (Dihedral::Rotate180, self.rotate180),
            (Dihedral::Rotate270, self.rotate270),
            (Dihedral::FlipX, self.flip_x),
            (Dihedral::FlipY, self.flip_y),
            (Dihedral::FlipPlus1, self.flip_plus1),
            (Dihedral::FlipMinus1, self.flip_minus1),
        ]
    }

    /// 相手のハッシュに最も近い変換と、その距離を返す。
    ///
    /// 回転や反転された同一画像を拾いたいときに使う。
    pub fn best_match(&self, other: &Hash256) -> (Dihedral, u32) {
        self.all()
            .into_iter()
            .map(|(kind, hash)| (kind, hash.hamming_distance(other)))
            .min_by_key(|&(_, distance)| distance)
            .expect("all() は常に 8 要素を返す")
    }
}

/// 作業バッファを使い回すハッシャ。
///
/// 大量のファイルを処理するときは、画像 1 枚ごとにフル解像度ぶんの `Vec<f32>` を
/// 確保し直さずに済むぶん速い。1 枚しか扱わないなら [`hash_image`] などの
/// 自由関数で十分。
pub struct Hasher {
    /// フル解像度の輝度値。Jarosz フィルタの結果もここに残る。
    luma: Vec<f32>,
    /// Jarosz フィルタが行方向と列方向の間で使う一時領域。
    scratch: Vec<f32>,
    buffer64x64: Box<[f32; 64 * 64]>,
    buffer16x64: Box<[f32; 16 * 64]>,
    buffer16x16: Box<[f32; 16 * 16]>,
    buffer16x16_aux: Box<[f32; 16 * 16]>,
}

impl Default for Hasher {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher {
    pub fn new() -> Self {
        Self {
            luma: Vec::new(),
            scratch: Vec::new(),
            buffer64x64: Box::new([0.0; 64 * 64]),
            buffer16x64: Box::new([0.0; 16 * 64]),
            buffer16x16: Box::new([0.0; 16 * 16]),
            buffer16x16_aux: Box::new([0.0; 16 * 16]),
        }
    }

    /// 画像のハッシュと quality を返す。
    ///
    /// 小さすぎる画像 (縦横どちらかが 5 未満) は [`Hash256::ZERO`] と quality 0 になる。
    /// リファレンス実装がそう振る舞うため。
    pub fn hash_image(&mut self, image: &Image) -> Result<(Hash256, u8), UnsupportedChannels> {
        if is_too_small(image) {
            return Ok((Hash256::ZERO, 0));
        }
        self.fill_luma(image)?;
        // リファレンスの pdqHash256FromFloatLuma に倣って 64x64 入力を素通しする。
        // 純粋な最適化で、通しても通さなくても結果は同じ (64x64 では Jarosz の
        // 窓幅が 1 になり、間引きも恒等写像になるため)。
        let quality = self.downsample_and_dct(image.height, image.width, true);
        Ok((buffer16x16_to_bits(&self.buffer16x16), quality))
    }

    /// 画像の 8 通りの二面体ハッシュと quality を返す。
    pub fn dihedral_image(
        &mut self,
        image: &Image,
    ) -> Result<(DihedralHashes, u8), UnsupportedChannels> {
        if is_too_small(image) {
            return Ok((DihedralHashes::default(), 0));
        }
        self.fill_luma(image)?;
        // リファレンスの pdqDihedralHash256esFromFloatLuma は 64x64 ショートカットを
        // 持たないので、こちらも無効にして経路を揃えておく。結果は変わらない。
        let quality = self.downsample_and_dct(image.height, image.width, false);
        Ok((self.dihedral_bits(), quality))
    }

    // ----------------------------------------------------------------

    /// 画素を輝度値に展開して [`Hasher::luma`] に置き、作業領域を用意する。
    fn fill_luma(&mut self, image: &Image) -> Result<(), UnsupportedChannels> {
        let n = image.height * image.width;
        assert_eq!(
            image.data.len(),
            n * image.channels,
            "画素データの長さが寸法と一致しません"
        );

        self.luma.clear();
        self.luma.reserve(n);
        match image.channels {
            // グレースケールは輝度係数を通さない。リファレンス実装も 1 チャンネル
            // 画像には fillFloatLumaFromGrey という別経路を使っており、
            // RGB に展開してから変換すると係数の丸めのぶんだけ値がずれる。
            1 => self.luma.extend(image.data.iter().map(|&v| v as f32)),
            3 => self.luma.extend(image.data.chunks_exact(3).map(|p| {
                LUMA_FROM_R * p[0] as f32 + LUMA_FROM_G * p[1] as f32 + LUMA_FROM_B * p[2] as f32
            })),
            other => return Err(UnsupportedChannels(other)),
        }

        // scratch は Jarosz フィルタが全要素を書き潰すので、初期化は不要。
        // clear() してから resize すると全域のゼロ埋めが走ってしまうので呼ばない。
        self.scratch.resize(n, 0.0);
        Ok(())
    }

    /// 64x64 に縮小して DCT まで進める。quality を返す。
    fn downsample_and_dct(
        &mut self,
        num_rows: usize,
        num_cols: usize,
        allow_64x64_shortcut: bool,
    ) -> u8 {
        if allow_64x64_shortcut && num_rows == 64 && num_cols == 64 {
            self.buffer64x64.copy_from_slice(&self.luma);
        } else {
            let window_size_along_rows = downscale::compute_jarosz_filter_window_size(num_cols, 64);
            let window_size_along_cols = downscale::compute_jarosz_filter_window_size(num_rows, 64);

            downscale::jarosz_filter(
                &mut self.luma,
                &mut self.scratch,
                num_rows,
                num_cols,
                window_size_along_rows,
                window_size_along_cols,
                NUM_JAROSZ_XY_PASSES,
            );
            downscale::decimate_to_64x64(&self.luma, num_rows, num_cols, &mut self.buffer64x64);
        }

        // quality は画像領域の 64x64 を再利用して求める。
        let quality = image_domain_quality_metric(&self.buffer64x64);

        dct::dct64_to_16(&self.buffer64x64, &mut self.buffer16x64, &mut self.buffer16x16);

        quality
    }

    fn dihedral_bits(&mut self) -> DihedralHashes {
        let original = buffer16x16_to_bits(&self.buffer16x16);

        let mut apply = |transform: fn(&[f32; 256], &mut [f32; 256])| {
            transform(&self.buffer16x16, &mut self.buffer16x16_aux);
            buffer16x16_to_bits(&self.buffer16x16_aux)
        };

        DihedralHashes {
            original,
            rotate90: apply(dct::rotate90),
            rotate180: apply(dct::rotate180),
            rotate270: apply(dct::rotate270),
            flip_x: apply(dct::flip_x),
            flip_y: apply(dct::flip_y),
            flip_plus1: apply(dct::flip_plus1),
            flip_minus1: apply(dct::flip_minus1),
        }
    }
}

#[inline]
fn is_too_small(image: &Image) -> bool {
    image.height < MIN_HASHABLE_DIM || image.width < MIN_HASHABLE_DIM
}

/// 16x16 の DCT 係数を、中央値を閾値にして 256 ビットに落とす。
fn buffer16x16_to_bits(buffer16x16: &[f32; 256]) -> Hash256 {
    let dct_median = median::torben(buffer16x16);

    let mut hash = Hash256::ZERO;
    for i in 0..16 {
        for j in 0..16 {
            if buffer16x16[i * 16 + j] > dct_median {
                hash.set_bit(i * 16 + j);
            }
        }
    }
    hash
}

/// 画像領域の勾配量から求める品質指標。リファレンスの `pdqImageDomainQualityMetric`。
///
/// 定数はすべて経験的に決められたもので、意味を問うても仕方がない。
/// 小さい勾配を大量に数えないよう、整数への切り捨てで量子化しているのが要点。
fn image_domain_quality_metric(buffer64x64: &[f32; 4096]) -> u8 {
    let mut gradient_sum: i32 = 0;

    for i in 0..63 {
        for j in 0..64 {
            let u = buffer64x64[i * 64 + j];
            let v = buffer64x64[(i + 1) * 64 + j];
            let d = ((u - v) * 100.0 / 255.0) as i32;
            gradient_sum += d.abs();
        }
    }
    for i in 0..64 {
        for j in 0..63 {
            let u = buffer64x64[i * 64 + j];
            let v = buffer64x64[i * 64 + j + 1];
            let d = ((u - v) * 100.0 / 255.0) as i32;
            gradient_sum += d.abs();
        }
    }

    // ヒューリスティックな倍率。
    (gradient_sum / 90).min(100) as u8
}

// ================================================================
// 自由関数版。1 枚だけ処理するとき用。

/// 画像のハッシュと quality を返す。
pub fn hash_image(image: &Image) -> Result<(Hash256, u8), UnsupportedChannels> {
    Hasher::new().hash_image(image)
}

/// 画像の 8 通りの二面体ハッシュと quality を返す。
pub fn dihedral_image(image: &Image) -> Result<(DihedralHashes, u8), UnsupportedChannels> {
    Hasher::new().dihedral_image(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(width: usize, height: usize) -> Image {
        let mut data = vec![0u8; width * height * 3];
        for (i, p) in data.chunks_exact_mut(3).enumerate() {
            p[0] = (i % 251) as u8;
            p[1] = (i % 241) as u8;
            p[2] = (i % 239) as u8;
        }
        Image { width, height, channels: 3, data }
    }

    fn flat(width: usize, height: usize, channels: usize, value: u8) -> Image {
        Image { width, height, channels, data: vec![value; width * height * channels] }
    }

    #[test]
    fn too_small_images_hash_to_zero() {
        let (hash, quality) = hash_image(&flat(4, 4, 3, 0)).unwrap();
        assert_eq!(hash, Hash256::ZERO);
        assert_eq!(quality, 0);
    }

    /// 平坦な画像は勾配がないので quality 0 になる。
    #[test]
    fn flat_image_has_zero_quality() {
        let (_, quality) = hash_image(&flat(100, 100, 3, 128)).unwrap();
        assert_eq!(quality, 0);
    }

    #[test]
    fn alpha_channels_are_rejected_until_dropped() {
        let mut image = flat(100, 100, 4, 128);
        assert_eq!(hash_image(&image), Err(UnsupportedChannels(4)));
        image.drop_alpha();
        assert!(hash_image(&image).is_ok());
    }

    /// 二面体版の original は通常のハッシュと一致するはず
    /// (64x64 ショートカットが効かない寸法において)。
    #[test]
    fn dihedral_original_matches_plain_hash() {
        let image = rgb(300, 200);
        let (plain, plain_quality) = hash_image(&image).unwrap();
        let (dihedral, dihedral_quality) = dihedral_image(&image).unwrap();
        assert_eq!(plain, dihedral.original);
        assert_eq!(plain_quality, dihedral_quality);
    }

    /// 64x64 ちょうどの入力でも両経路の結果は一致する。
    ///
    /// [`Hasher::hash_image`] だけが持つ 64x64 ショートカットは純粋な最適化で、
    /// 通る経路が違っても結果は変わらない。64x64 では Jarosz の窓幅が 1 になり、
    /// 間引きも 64 -> 64 の恒等写像になるため。
    #[test]
    fn both_paths_agree_at_exactly_64x64() {
        let image = rgb(64, 64);
        let (plain, _) = hash_image(&image).unwrap();
        let (dihedral, _) = dihedral_image(&image).unwrap();
        assert_eq!(plain, dihedral.original);
    }

    /// バッファを使い回しても、毎回新しく作った場合と同じ結果になる。
    /// 前の画像の残骸が漏れないことの確認。
    #[test]
    fn reused_hasher_matches_fresh_hasher() {
        let large = rgb(300, 200);
        let small = rgb(50, 40);

        let mut hasher = Hasher::new();
        hasher.hash_image(&large).unwrap();
        let reused = hasher.hash_image(&small).unwrap();

        assert_eq!(reused, hash_image(&small).unwrap());
    }

    #[test]
    fn best_match_finds_the_rotated_variant() {
        let (dihedral, _) = dihedral_image(&rgb(300, 200)).unwrap();
        assert_eq!(dihedral.best_match(&dihedral.rotate180), (Dihedral::Rotate180, 0));
    }
}
