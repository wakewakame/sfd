//! 2 次元 DCT と、そこから直接得られる回転・反転版。
//!
//! `facebook/ThreatExchange` の `pdq/cpp/hashing/pdqhashing.cpp` からの逐語移植。
//! Copyright (c) Meta Platforms, Inc. and affiliates. BSD 3-Clause (../LICENSE)。

use std::f64::consts::PI;
use std::sync::OnceLock;

/// 16x64 の DCT 行列 (row-major)。
///
/// 64x64 DCT の全出力のうち PDQ が使うのは低周波側の 16x16 だけ、しかも
/// **DC 成分 (0 番) は捨てて 1..=16 番目の周波数**を使う。係数の `(i + 1)` がそれ。
/// リファレンスは倍率を float、cos を double で計算して float に落とすので、
/// ビット一致のためにその順序をそのまま再現している。
fn dct_matrix_64() -> &'static [f32; 16 * 64] {
    static MATRIX: OnceLock<[f32; 16 * 64]> = OnceLock::new();
    MATRIX.get_or_init(|| {
        const NUM_ROWS: usize = 16;
        const NUM_COLS: usize = 64;
        let matrix_scale_factor = (2.0f64 / NUM_COLS as f64).sqrt() as f32;

        let mut matrix = [0.0f32; NUM_ROWS * NUM_COLS];
        for i in 0..NUM_ROWS {
            for j in 0..NUM_COLS {
                let angle = (PI / 2.0 / NUM_COLS as f64) * (i + 1) as f64 * (2 * j + 1) as f64;
                matrix[i * NUM_COLS + j] = (matrix_scale_factor as f64 * angle.cos()) as f32;
            }
        }
        matrix
    })
}

/// 64x64 の画像領域バッファから 16x16 の DCT 係数を得る。`B = D A Dᵀ`。
///
/// 必要な 16x16 しか計算しないので、64x64 の完全な DCT より速い
/// (リファレンスのコメントによれば Lee のアルゴリズムより速かったとのこと)。
pub(crate) fn dct64_to_16(a: &[f32; 4096], t: &mut [f32; 16 * 64], b: &mut [f32; 256]) {
    let d = dct_matrix_64();

    // T = D A
    for i in 0..16 {
        for j in 0..64 {
            let mut sumk = 0.0f32;
            for k in 0..64 {
                sumk += d[i * 64 + k] * a[k * 64 + j];
            }
            t[i * 64 + j] = sumk;
        }
    }

    // B = T Dᵀ
    for i in 0..16 {
        for j in 0..16 {
            let mut sumk = 0.0f32;
            for k in 0..64 {
                sumk += t[i * 64 + k] * d[j * 64 + k];
            }
            b[i * 16 + j] = sumk;
        }
    }
}

// 回転・反転版は画像を作り直さず、DCT 係数の符号反転と転置だけで得られる。
//
// orig      rot90     rot180    rot270
// noxpose   xpose     noxpose   xpose
// + + + +   - + - +   + - + -   - - - -
// + + + +   - + - +   - + - +   + + + +
// + + + +   - + - +   + - + -   - - - -
// + + + +   - + - +   - + - +   + + + +
//
// flipx     flipy     flipplus  flipminus
// noxpose   noxpose   xpose     xpose
// - - - -   - + - +   + + + +   + - + -
// + + + +   - + - +   + + + +   - + - +
// - - - -   - + - +   + + + +   + - + -
// + + + +   - + - +   + + + +   - + - +

pub(crate) fn rotate90(a: &[f32; 256], b: &mut [f32; 256]) {
    for i in 0..16 {
        for j in 0..16 {
            b[j * 16 + i] = if j & 1 == 1 { a[i * 16 + j] } else { -a[i * 16 + j] };
        }
    }
}

pub(crate) fn rotate180(a: &[f32; 256], b: &mut [f32; 256]) {
    for i in 0..16 {
        for j in 0..16 {
            b[i * 16 + j] = if (i + j) & 1 == 1 { -a[i * 16 + j] } else { a[i * 16 + j] };
        }
    }
}

pub(crate) fn rotate270(a: &[f32; 256], b: &mut [f32; 256]) {
    for i in 0..16 {
        for j in 0..16 {
            b[j * 16 + i] = if i & 1 == 1 { a[i * 16 + j] } else { -a[i * 16 + j] };
        }
    }
}

pub(crate) fn flip_x(a: &[f32; 256], b: &mut [f32; 256]) {
    for i in 0..16 {
        for j in 0..16 {
            b[i * 16 + j] = if i & 1 == 1 { a[i * 16 + j] } else { -a[i * 16 + j] };
        }
    }
}

pub(crate) fn flip_y(a: &[f32; 256], b: &mut [f32; 256]) {
    for i in 0..16 {
        for j in 0..16 {
            b[i * 16 + j] = if j & 1 == 1 { a[i * 16 + j] } else { -a[i * 16 + j] };
        }
    }
}

pub(crate) fn flip_plus1(a: &[f32; 256], b: &mut [f32; 256]) {
    for i in 0..16 {
        for j in 0..16 {
            b[j * 16 + i] = a[i * 16 + j];
        }
    }
}

pub(crate) fn flip_minus1(a: &[f32; 256], b: &mut [f32; 256]) {
    for i in 0..16 {
        for j in 0..16 {
            b[j * 16 + i] = if (i + j) & 1 == 1 { -a[i * 16 + j] } else { a[i * 16 + j] };
        }
    }
}
