//! 標準エラー出力への進捗表示。
//!
//! 数時間かかることがあるので、無言だと止まっているのか進んでいるのか分からない。
//! ただし端末でないときは何も出さない。リダイレクト先に `\r` が混ざるとログとして
//! 読めなくなるため。

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

/// 進捗表示に使う桁数。行を消すときもこの幅で揃える。
const WIDTH: usize = 78;

/// 表示を更新する間隔。細かく書き換えても読めないうえ、書き込み自体が無駄になる。
const INTERVAL: Duration = Duration::from_millis(200);

pub struct Reporter {
    interactive: bool,
    started: Instant,
    last_update: Instant,
}

impl Reporter {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            interactive: std::io::stderr().is_terminal(),
            started: now,
            // 最初の 1 件目から表示されるよう、更新済みにはしない。
            last_update: now - INTERVAL,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// 総数がまだ分からない段階の表示。
    pub fn counting(&mut self, label: &str, count: usize) {
        if self.due() {
            self.print(format_args!("{label} {count} 件"));
        }
    }

    /// 割合と残り時間つきの表示。
    pub fn update(&mut self, done: usize, total: usize, label: &str) {
        if !self.due() {
            return;
        }
        let percent = if total == 0 { 100.0 } else { done as f64 * 100.0 / total as f64 };
        // 残り時間は「これまでの平均が続く」という前提の粗い見積もり。
        let remaining = if done == 0 {
            String::from("--")
        } else {
            let per_item = self.started.elapsed().as_secs_f64() / done as f64;
            format_duration(per_item * total.saturating_sub(done) as f64)
        };
        let label = tail(label, 34);
        self.print(format_args!("{done}/{total} ({percent:.1}%) 残り約 {remaining}  {label:<34}"));
    }

    /// 書きかけの行を消して行頭に戻る。次の出力に移る前に呼ぶ。
    pub fn finish(&mut self) {
        if self.interactive {
            eprint!("\r{}\r", " ".repeat(WIDTH));
            let _ = std::io::stderr().flush();
        }
    }

    fn due(&mut self) -> bool {
        if !self.interactive || self.last_update.elapsed() < INTERVAL {
            return false;
        }
        self.last_update = Instant::now();
        true
    }

    fn print(&self, args: std::fmt::Arguments<'_>) {
        eprint!("\r{args}");
        let _ = std::io::stderr().flush();
    }
}

impl Default for Reporter {
    fn default() -> Self {
        Self::new()
    }
}

fn format_duration(seconds: f64) -> String {
    match seconds as u64 {
        s if s < 60 => format!("{s}秒"),
        s if s < 3600 => format!("{}分", s / 60),
        s => format!("{}時間{}分", s / 3600, s % 3600 / 60),
    }
}

/// 進捗表示に収まるよう、長いパスは先頭を省く。
fn tail(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    format!("...{}", chars[chars.len() - (width - 3)..].iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 省略後もちょうど `width` 文字に収まる (進捗表示の桁が揺れない)。
    #[test]
    fn shortens_long_paths_from_the_front() {
        assert_eq!(tail("abc", 10), "abc");
        assert_eq!(tail("abcdefghij", 10), "abcdefghij");
        assert_eq!(tail("abcdefghijk", 10), "...efghijk");
        assert_eq!(tail("abcdefghijk", 10).chars().count(), 10);
        // マルチバイトでもバイト境界で切らない。
        assert_eq!(tail("あいうえおかきくけこ", 5), "...けこ");
    }

    #[test]
    fn formats_remaining_time_by_magnitude() {
        assert_eq!(format_duration(0.4), "0秒");
        assert_eq!(format_duration(59.9), "59秒");
        assert_eq!(format_duration(60.0), "1分");
        assert_eq!(format_duration(3599.0), "59分");
        assert_eq!(format_duration(3600.0), "1時間0分");
        assert_eq!(format_duration(7860.0), "2時間11分");
    }
}
