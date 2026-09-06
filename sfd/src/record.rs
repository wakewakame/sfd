//! hash.json の形式と、その読み書き。
//!
//! 1 行 1 JSON (JSONL) で、1 行目が [`Header`]、以降が 1 ファイル 1 行の [`Record`]。
//! 追記に向いた形なので、中断しても既に書かれたパスを読み飛ばして再開できる。

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// hash.json の形式のバージョン。
///
/// 形式を変えたら上げる。読み込み時に食い違えば拒否するので、古いファイルが
/// 静かに誤解釈されることはない。
pub const FORMAT_VERSION: u32 = 1;

/// hash.json の 1 行目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    pub v: u32,
    /// 探索の起点。[`Record::path`] はここからの相対パスになる。
    pub root: String,
}

/// 1 ファイル分の記録。
///
/// `width` などが `None` の場合はフィールドごと出力しない。エラーで終わった
/// ファイルは [`Record::error`] を持ち、知覚ハッシュを持たない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// [`Header::root`] からの相対パス。
    pub path: String,
    pub bytes: u64,
    pub mtime: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// 動画の尺 (秒)。画像は持たないので、これの有無が画像と動画の区別になる。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// 知覚ハッシュ。画像も「1 フレームの動画」として同じ形で持つ。
    ///
    /// 画像と動画で形が変わらないので、比較する側は常に「ハッシュの集合どうしの
    /// 比較」として書ける。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdq: Option<Vec<Frame>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 1 フレーム分の知覚ハッシュ。
///
/// どのフレームかを位置ではなく `t` (秒) で持つので、将来サンプリング方法を
/// 変えても古い hash.json が静かに誤解釈されない。画像では `t` は 0。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub t: f64,
    pub hash: String,
    pub quality: u8,
}

// ================================================================
// 読み込み

/// 既存の hash.json から読み取った内容。
pub struct Existing {
    pub header: Header,
    /// 既に記録済みのパス。再開時にこれを読み飛ばす。
    pub done: HashSet<String>,
    /// 末尾行が途中で切れていたので切り捨てた。
    pub truncated_tail: bool,
}

#[derive(Debug)]
pub enum ReadError {
    Io(io::Error),
    /// ファイルはあるが hash.json として読めない。
    Malformed(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "{e}"),
            ReadError::Malformed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ReadError {}

impl From<io::Error> for ReadError {
    fn from(e: io::Error) -> Self {
        ReadError::Io(e)
    }
}

/// 既存の hash.json を読む。存在しなければ `None`。
///
/// 強制終了で末尾行が途中まで書かれている場合は、その行を切り捨ててから返す。
/// 途中で切れていられるのは末尾行だけなので、それ以外の壊れた行は破損として扱う。
pub fn read_existing(path: &Path) -> Result<Option<Existing>, ReadError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let lines: Vec<Vec<u8>> = BufReader::new(file).split(b'\n').collect::<io::Result<_>>()?;
    if lines.is_empty() {
        return Err(ReadError::Malformed("空のファイルです".into()));
    }

    let header: Header = serde_json::from_slice(&lines[0])
        .map_err(|e| ReadError::Malformed(format!("1 行目をヘッダとして読めません: {e}")))?;
    if header.v != FORMAT_VERSION {
        return Err(ReadError::Malformed(format!(
            "形式のバージョンが違います (このファイル: {}, このコマンド: {FORMAT_VERSION})",
            header.v
        )));
    }

    let complete = ends_with_newline(path)?;
    let mut done = HashSet::new();
    let mut truncated_tail = false;
    let mut valid_bytes = lines[0].len() + 1;

    for (index, line) in lines.iter().enumerate().skip(1) {
        match serde_json::from_slice::<Record>(line) {
            Ok(record) => {
                done.insert(record.path);
                valid_bytes += line.len() + 1;
            }
            Err(e) => {
                // 末尾が改行で終わっていなければ、書き込み途中で落ちたとみなす。
                if index == lines.len() - 1 && !complete {
                    truncated_tail = true;
                    break;
                }
                return Err(ReadError::Malformed(format!("{} 行目を読めません: {e}", index + 1)));
            }
        }
    }

    if truncated_tail {
        // 壊れた末尾行を落としておかないと、追記した内容まで読めなくなる。
        File::options().write(true).open(path)?.set_len(valid_bytes as u64)?;
    }

    Ok(Some(Existing { header, done, truncated_tail }))
}

/// ファイルが改行で終わっているか。`split` は末尾の改行の有無を区別しないので、
/// 最後の行が完結しているかどうかは元のバイト列を見るしかない。
fn ends_with_newline(path: &Path) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(true);
    }
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

// ================================================================
// 書き込み

/// hash.json への追記。
pub struct Writer {
    file: io::BufWriter<File>,
}

impl Writer {
    /// 新規作成してヘッダを書く。既存の内容は消える。
    pub fn create(path: &Path, root: &str) -> io::Result<Self> {
        let mut writer = Self { file: io::BufWriter::new(File::create(path)?) };
        writer.write_line(&Header { v: FORMAT_VERSION, root: root.to_string() })?;
        writer.flush()?;
        Ok(writer)
    }

    /// 既存ファイルの末尾に追記する。
    pub fn append(path: &Path) -> io::Result<Self> {
        Ok(Self { file: io::BufWriter::new(File::options().append(true).open(path)?) })
    }

    pub fn write(&mut self, record: &Record) -> io::Result<()> {
        self.write_line(record)
    }

    fn write_line<T: Serialize>(&mut self, value: &T) -> io::Result<()> {
        serde_json::to_writer(&mut self.file, value)?;
        self.file.write_all(b"\n")
    }

    /// 途中で落ちても記録が残るよう、区切りのいいところで書き出す。
    pub fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

// ================================================================
// 時刻

/// `SystemTime` を RFC 3339 (UTC) の文字列にする。
///
/// 依存を増やさないための自前実装。1970 年より前は扱わず、その場合はエポックに
/// 丸める (ファイルの mtime としては実質現れない)。
pub fn rfc3339(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, seconds_of_day) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) =
        (seconds_of_day / 3600, seconds_of_day % 3600 / 60, seconds_of_day % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// エポックからの日数を暦の年月日に直す。
///
/// Howard Hinnant の `civil_from_days`。3 月始まりの暦に移すことで閏日を年の
/// 末尾に追い出している。閏年と閏世紀の扱いを素朴に書くと間違えるので、
/// 検証済みのアルゴリズムをそのまま使っている。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11]、0 が 3 月
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32; // [1, 31]
    let month = if month_prime < 10 { month_prime + 3 } else { month_prime - 9 } as u32;

    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(epoch_secs: u64) -> String {
        rfc3339(UNIX_EPOCH + Duration::from_secs(epoch_secs))
    }

    #[test]
    fn formats_known_timestamps() {
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(1), "1970-01-01T00:00:01Z");
        assert_eq!(at(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(at(86_400), "1970-01-02T00:00:00Z");
        assert_eq!(at(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(at(2_147_483_647), "2038-01-19T03:14:07Z");
    }

    /// 閏年まわりは素朴に書くと間違えるところなので、境界を固定しておく。
    /// 期待値は `date -u -j -f "%Y-%m-%dT%H:%M:%SZ" ... +%s` で求めたもの。
    #[test]
    fn handles_leap_years() {
        // 1972 年は 4 で割り切れるので閏年。2/29 が存在する。
        assert_eq!(at(68_169_599), "1972-02-28T23:59:59Z");
        assert_eq!(at(68_169_600), "1972-02-29T00:00:00Z");
        assert_eq!(at(68_256_000), "1972-03-01T00:00:00Z");

        // 2000 年は 400 で割り切れるので閏年。
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(951_868_800), "2000-03-01T00:00:00Z");

        // 2100 年は 100 で割り切れるが 400 では割り切れないので平年。
        // 2/28 の翌日が 3/1 になる (2/29 は存在しない)。
        assert_eq!(at(4_107_542_399), "2100-02-28T23:59:59Z");
        assert_eq!(at(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn clamps_times_before_the_epoch() {
        assert_eq!(rfc3339(UNIX_EPOCH - Duration::from_secs(1)), "1970-01-01T00:00:00Z");
    }

    fn sample(duration: Option<f64>, frames: Vec<Frame>) -> Record {
        Record {
            path: "a/b".into(),
            bytes: 100,
            mtime: "2026-09-02T01:23:45Z".into(),
            sha256: Some("ab".repeat(32)),
            width: Some(1920),
            height: Some(1080),
            duration,
            pdq: Some(frames),
            error: None,
        }
    }

    /// 画像も動画も同じ形なので、読む側は分岐せずに済む。
    #[test]
    fn images_and_videos_share_one_shape() {
        let image = sample(None, vec![Frame { t: 0.0, hash: "cd".repeat(32), quality: 100 }]);
        let video = sample(
            Some(123.4),
            vec![
                Frame { t: 30.85, hash: "cd".repeat(32), quality: 100 },
                Frame { t: 61.7, hash: "ef".repeat(32), quality: 98 },
            ],
        );
        // duration の有無だけが画像と動画の違いになる。
        assert!(image.duration.is_none() && video.duration.is_some());

        for record in [&image, &video] {
            let line = serde_json::to_string(record).unwrap();
            let back: Record = serde_json::from_str(&line).unwrap();
            assert_eq!(back.pdq.unwrap().len(), record.pdq.as_ref().unwrap().len());
        }
    }

    #[test]
    fn absent_fields_are_omitted() {
        let line = serde_json::to_string(&sample(None, Vec::new())).unwrap();
        assert!(!line.contains("duration"), "{line}");
        assert!(!line.contains("error"), "{line}");
    }
}
