//! フル解像度から 64x64 への縮小。
//!
//! `facebook/ThreatExchange` の `pdq/cpp/downscaling/downscaling.cpp` からの逐語移植。
//! Copyright (c) Meta Platforms, Inc. and affiliates. BSD 3-Clause (../LICENSE)。
//!
//! ここは数式ではなく実装が仕様で、特に [`box_1d`] の 4 フェーズ構造と窓幅の丸めを
//! 変えるとハッシュがずれる。

/// リファレンスの `computeJaroszFilterWindowSize`。
///
/// PDQ は 64x64 に落とすので、1 パスあたりの窓は目標寸法の 2 倍ぶんに相当する幅を取る
/// (X,Y の box filter を 2 回通すため、1 回あたりは半分でよい)。
#[inline]
pub(crate) fn compute_jarosz_filter_window_size(old_dimension: usize, new_dimension: usize) -> usize {
    // リファレンスの (old + 2 * new - 1) / (2 * new)。
    old_dimension.div_ceil(2 * new_dimension)
}

/// 1 次元 box filter。リファレンスの `box1DFloat`。
///
/// 両端では窓が画像からはみ出すぶんだけ幅を狭め、その時点の実効幅で割る。
/// この「フェーズごとに除数が変わる」挙動が PDQ の縮小結果を決めているので、
/// 一般的な box blur で置き換えてはいけない。
fn box_1d(
    invec: &[f32],
    outvec: &mut [f32],
    vector_length: usize,
    stride: usize,
    full_window_size: usize,
) {
    // 7 -> 4, 8 -> 5
    let half_window_size = (full_window_size + 2) / 2;

    let phase_1_nreps = half_window_size - 1;
    let phase_2_nreps = full_window_size - half_window_size + 1;
    // 窓が列/行より長い場合、リファレンスでは負値になりループが回らない。
    // PDQ の縮小経路では full_window_size <= vector_length が常に成り立つ。
    debug_assert!(full_window_size <= vector_length);
    let phase_3_nreps = vector_length.saturating_sub(full_window_size);
    let phase_4_nreps = half_window_size - 1;

    let mut li = 0; // 減算する側 (窓の左端)
    let mut ri = 0; // 加算する側 (窓の右端)
    let mut oi = 0; // 出力位置

    let mut sum = 0.0f32;
    let mut current_window_size = 0u32;

    // フェーズ 1: 加算のみ。出力しない。
    for _ in 0..phase_1_nreps {
        sum += invec[ri];
        current_window_size += 1;
        ri += stride;
    }

    // フェーズ 2: 窓が狭いまま出力を始める。
    for _ in 0..phase_2_nreps {
        sum += invec[ri];
        current_window_size += 1;
        outvec[oi] = sum / current_window_size as f32;
        ri += stride;
        oi += stride;
    }

    // フェーズ 3: 窓幅が一定。加算と減算を両方行う。
    for _ in 0..phase_3_nreps {
        sum += invec[ri];
        sum -= invec[li];
        outvec[oi] = sum / current_window_size as f32;
        li += stride;
        ri += stride;
        oi += stride;
    }

    // フェーズ 4: 右端に到達。窓を縮めながら出力する。
    for _ in 0..phase_4_nreps {
        sum -= invec[li];
        current_window_size -= 1;
        outvec[oi] = sum / current_window_size as f32;
        li += stride;
        oi += stride;
    }
}

fn box_along_rows(input: &[f32], out: &mut [f32], num_rows: usize, num_cols: usize, window_size: usize) {
    for i in 0..num_rows {
        let base = i * num_cols;
        box_1d(
            &input[base..base + num_cols],
            &mut out[base..base + num_cols],
            num_cols,
            1,
            window_size,
        );
    }
}

fn box_along_cols(input: &[f32], out: &mut [f32], num_rows: usize, num_cols: usize, window_size: usize) {
    for j in 0..num_cols {
        box_1d(&input[j..], &mut out[j..], num_rows, num_cols, window_size);
    }
}

/// リファレンスの `jaroszFilterFloat`。
///
/// 行方向・列方向の box filter を `nreps` 回繰り返す。1 往復ごとに
/// `buffer1 -> buffer2 -> buffer1` と往復するので、結果は必ず `buffer1` に残る。
pub(crate) fn jarosz_filter(
    buffer1: &mut [f32],
    buffer2: &mut [f32],
    num_rows: usize,
    num_cols: usize,
    window_size_along_rows: usize,
    window_size_along_cols: usize,
    nreps: usize,
) {
    for _ in 0..nreps {
        box_along_rows(buffer1, buffer2, num_rows, num_cols, window_size_along_rows);
        box_along_cols(buffer2, buffer1, num_rows, num_cols, window_size_along_cols);
    }
}

/// リファレンスの `decimateFloat`。角ではなく各出力セルの中心を取る。
pub(crate) fn decimate_to_64x64(input: &[f32], in_num_rows: usize, in_num_cols: usize, out: &mut [f32; 4096]) {
    for outi in 0..64 {
        let ini = ((outi as f64 + 0.5) * in_num_rows as f64 / 64.0) as usize;
        for outj in 0..64 {
            let inj = ((outj as f64 + 0.5) * in_num_cols as f64 / 64.0) as usize;
            out[outi * 64 + outj] = input[ini * in_num_cols + inj];
        }
    }
}
