//! Torben Mogensen の中央値アルゴリズム。
//!
//! `facebook/ThreatExchange` の `pdq/cpp/hashing/torben.cpp` からの逐語移植。
//! 上流はこのファイルだけを public domain と宣言している
//! (アルゴリズム: Torben Mogensen、実装: N. Devillard)。
//!
//! 入力を破壊せず追加メモリも使わない代わりに、値域を二分探索するので複数回走査する。
//!
//! ここを「ソートして中央 2 つの平均」に置き換えてはいけない。要素数が偶数 (PDQ では
//! 256) のとき、この実装は平均ではなく片側の値そのものを返すことがあり、
//! しきい値が変わってハッシュがずれる。

pub(crate) fn torben(m: &[f32]) -> f32 {
    let n = m.len();
    assert!(n > 0);

    // リファレンスの (n + 1) / 2。
    let half = n.div_ceil(2);

    let mut min = m[0];
    let mut max = m[0];
    for &v in &m[1..] {
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
    }

    let mut guess;
    let mut less;
    let mut equal;
    let mut maxltguess;
    let mut mingtguess;

    loop {
        guess = (min + max) / 2.0;
        less = 0usize;
        let mut greater = 0usize;
        equal = 0usize;
        maxltguess = min;
        mingtguess = max;

        for &v in m {
            if v < guess {
                less += 1;
                if v > maxltguess {
                    maxltguess = v;
                }
            } else if v > guess {
                greater += 1;
                if v < mingtguess {
                    mingtguess = v;
                }
            } else {
                equal += 1;
            }
        }

        if less <= half && greater <= half {
            break;
        } else if less > greater {
            max = maxltguess;
        } else {
            min = mingtguess;
        }
    }

    if less >= half {
        maxltguess
    } else if less + equal >= half {
        guess
    } else {
        mingtguess
    }
}
