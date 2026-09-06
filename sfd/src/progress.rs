//! 標準エラー出力への進捗表示。
//!
//! 数時間かかることがあるので、無言だと止まっているのか進んでいるのか分からない。
//! ただし端末でないときは何も出さない。リダイレクト先に `\r` が混ざるとログとして
//! 読めなくなるため。

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

/// 進捗表示のうち、処理中のパスに割り当てる桁数。
const LABEL_WIDTH: usize = 34;

/// 表示を更新する間隔。細かく書き換えても読めないうえ、書き込み自体が無駄になる。
const INTERVAL: Duration = Duration::from_millis(200);

pub struct Reporter {
    interactive: bool,
    started: Instant,
    last_update: Instant,
    /// 直前に書いた行の桁数。次の行が短いときに、その差だけ空白で消す。
    last_width: usize,
}

impl Reporter {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            interactive: std::io::stderr().is_terminal(),
            started: now,
            // 最初の 1 件目から表示されるよう、更新済みにはしない。
            last_update: now - INTERVAL,
            last_width: 0,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// 残り時間の基準になる時計を、いま始まる作業の開始時刻に合わせる。
    ///
    /// 残り時間は「総経過時間 ÷ 処理済み件数」で見積もっているので、前の工程に
    /// かかった時間が混ざると 1 件あたりが過大になる。探索に 60 秒かかった直後の
    /// 1 件目で「1 件 60 秒」と見積もられ、序盤の表示が桁違いになってしまう。
    pub fn restart(&mut self) {
        self.started = Instant::now();
    }

    /// 総数がまだ分からない段階の表示。
    pub fn counting(&mut self, label: &str, count: usize) {
        if self.due() {
            self.print(&format!("{label} {count}"));
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
        let label = tail(label, LABEL_WIDTH);
        self.print(&format!("{done}/{total} ({percent:.1}%) ~{remaining} left  {label}"));
    }

    /// 書きかけの行を消して行頭に戻る。次の出力に移る前に呼ぶ。
    pub fn finish(&mut self) {
        if self.interactive && self.last_width > 0 {
            eprint!("\r{}\r", " ".repeat(self.last_width));
            let _ = std::io::stderr().flush();
            self.last_width = 0;
        }
    }

    fn due(&mut self) -> bool {
        if !self.interactive || self.last_update.elapsed() < INTERVAL {
            return false;
        }
        self.last_update = Instant::now();
        true
    }

    /// 行頭に戻って書き直す。
    ///
    /// 前の行より短くなるぶんを空白で埋めないと、Windows のコンソールでは右側に
    /// 残像が残る。ANSI の消去シーケンスを使わないのは、VT が有効でない古い
    /// コンソールでもそのまま制御文字が見えてしまうため。
    fn print(&mut self, text: &str) {
        let width = display_width(text);
        eprint!("\r{text}{}", " ".repeat(self.last_width.saturating_sub(width)));
        let _ = std::io::stderr().flush();
        self.last_width = width;
    }
}

impl Default for Reporter {
    fn default() -> Self {
        Self::new()
    }
}

/// 英語の複数形。件数を伴う語がいくつかあるので、そのたびに書かずに済ませる。
pub fn plural(count: usize, singular: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {singular}s")
    }
}

fn format_duration(seconds: f64) -> String {
    match seconds as u64 {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s => format!("{}h{}m", s / 3600, s % 3600 / 60),
    }
}

/// 端末に並べたときの桁数。
///
/// 日本語のパスは 1 文字で 2 桁を占めるので、文字数で数えると幅を見誤って
/// 残像が残る。厳密な実装には Unicode の東アジア文字幅表が要るが、ここでは
/// 進捗表示の桁を揃えるのが目的なので、主要な全角の範囲だけを見る近似で足りる。
fn display_width(text: &str) -> usize {
    text.chars().map(|c| if is_wide(c) { 2 } else { 1 }).sum()
}

fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F      // ハングル字母
        | 0x2E80..=0x303E    // CJK 部首、かな記号
        | 0x3041..=0x33FF    // かな、CJK 互換
        | 0x3400..=0x4DBF    // CJK 拡張 A
        | 0x4E00..=0x9FFF    // CJK 統合漢字
        | 0xA000..=0xA4CF    // イ文字
        | 0xAC00..=0xD7A3    // ハングル音節
        | 0xF900..=0xFAFF    // CJK 互換漢字
        | 0xFE30..=0xFE6F    // CJK 互換形
        | 0xFF00..=0xFF60    // 全角英数
        | 0xFFE0..=0xFFE6    // 全角記号
        | 0x1F300..=0x1F64F  // 絵文字
        | 0x1F900..=0x1F9FF
        | 0x20000..=0x3FFFD  // CJK 拡張 B 以降
    )
}

/// 進捗表示に収まるよう、長いパスは先頭を省く。
///
/// 返す文字列はちょうど `width` 桁になる。短いときは右を空白で埋める。
fn tail(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return format!("{text}{}", " ".repeat(width - display_width(text)));
    }
    // 後ろから桁数を積んで、"..." のぶんを残せるところまで取る。
    let mut kept = String::new();
    let mut used = 0;
    for c in text.chars().rev() {
        let w = if is_wide(c) { 2 } else { 1 };
        if used + w > width - 3 {
            break;
        }
        kept.insert(0, c);
        used += w;
    }
    format!("...{kept}{}", " ".repeat(width - 3 - used))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 省略しても短くても、返る文字列はちょうど `width` 桁になる。
    /// 桁が揺れると、前の行の右側が消えずに残像になる。
    #[test]
    fn pads_and_truncates_to_exactly_the_width() {
        assert_eq!(tail("abc", 10), "abc       ");
        assert_eq!(tail("abcdefghij", 10), "abcdefghij");
        assert_eq!(tail("abcdefghijk", 10), "...efghijk");
        for text in ["a", "abcdefghij", "abcdefghijklmnop", "あいう", "あいうえおかきく"] {
            assert_eq!(display_width(&tail(text, 10)), 10, "{text}");
        }
    }

    /// 日本語は 1 文字 2 桁として数える。文字数で数えると幅を見誤る。
    #[test]
    fn counts_east_asian_characters_as_two_columns() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("あいう"), 6);
        // カタカナ 4 文字が 8 桁、"6.PNG" が 5 桁。
        assert_eq!(display_width("スライド6.PNG"), 4 * 2 + 5);
        // 全角に切り替わる境目でも、はみ出さずに収める。
        assert_eq!(display_width(&tail("あいうえおかきくけこ", 9)), 9);
        assert_eq!(display_width(&tail("あいうえおかきくけこ", 10)), 10);
    }

    /// 1 件のときだけ s を付けない。1 errors と出ていたのを直したところ。
    #[test]
    fn pluralises_only_beyond_one() {
        assert_eq!(plural(0, "file"), "0 files");
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(2, "file"), "2 files");
        assert_eq!(plural(1, "group"), "1 group");
    }

    #[test]
    fn formats_remaining_time_by_magnitude() {
        assert_eq!(format_duration(0.4), "0s");
        assert_eq!(format_duration(59.9), "59s");
        assert_eq!(format_duration(60.0), "1m");
        assert_eq!(format_duration(3599.0), "59m");
        assert_eq!(format_duration(3600.0), "1h0m");
        assert_eq!(format_duration(7860.0), "2h11m");
    }
}
