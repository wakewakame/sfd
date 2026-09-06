//! 膨大にある画像・動画ファイルの中から類似のファイルを検出するツール。

mod media;
mod record;
mod scan;
mod walk;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
使い方: sfd <サブコマンド> [オプション]

サブコマンド:
  hash <ディレクトリ> <hash.json>   画像・動画を再帰的に探索してハッシュを計算する
  find <hash.json> <find.json>      hash.json を元に似ているファイルをまとめる

sfd hash のオプション:
  -j, --jobs N       並列数 (既定: CPU 数)
      --no-resume    既存の hash.json を無視して最初からやり直す
      --ffmpeg PATH  ffmpeg の実行ファイル (環境変数 SFD_FFMPEG でも可)
      --ffprobe PATH ffprobe の実行ファイル (環境変数 SFD_FFPROBE でも可)

共通のオプション:
  -h, --help         この使い方を表示する

エラーが起きても処理は止めず、hash.json に \"error\" を持つ行として残す。
進捗は標準エラー出力に出す。
";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("sfd: {message}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1).peekable();

    let subcommand = match args.next() {
        Some(arg) if arg == "-h" || arg == "--help" => {
            print!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        Some(arg) => arg,
        None => {
            print!("{USAGE}");
            return Ok(ExitCode::from(2));
        }
    };

    match subcommand.as_str() {
        "hash" => scan::run(parse_hash_args(args)?),
        "find" => Err("sfd find は未実装です".to_string()),
        other => Err(format!("不明なサブコマンドです: {other}")),
    }
}

/// `sfd hash` の設定。
pub struct HashOptions {
    pub root: PathBuf,
    pub output: PathBuf,
    pub jobs: usize,
    pub resume: bool,
    pub ffmpeg: String,
    pub ffprobe: String,
}

fn parse_hash_args(mut args: impl Iterator<Item = String>) -> Result<HashOptions, String> {
    let mut positional = Vec::new();
    let mut jobs = None;
    let mut resume = true;
    let mut ffmpeg = std::env::var("SFD_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string());
    let mut ffprobe = std::env::var("SFD_FFPROBE").unwrap_or_else(|_| "ffprobe".to_string());

    while let Some(arg) = args.next() {
        let mut take_value = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} には値が必要です"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-j" | "--jobs" => {
                let value = take_value("--jobs")?;
                jobs = Some(
                    value
                        .parse::<usize>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| format!("--jobs には 1 以上の整数を指定してください: {value}"))?,
                );
            }
            "--no-resume" => resume = false,
            "--ffmpeg" => ffmpeg = take_value("--ffmpeg")?,
            "--ffprobe" => ffprobe = take_value("--ffprobe")?,
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("不明なオプションです: {other}"))
            }
            _ => positional.push(arg),
        }
    }

    let [root, output] = positional.as_slice() else {
        return Err("sfd hash <ディレクトリ> <hash.json> の形で指定してください".to_string());
    };

    Ok(HashOptions {
        root: PathBuf::from(root),
        output: PathBuf::from(output),
        jobs: jobs.unwrap_or_else(default_jobs),
        resume,
        ffmpeg,
        ffprobe,
    })
}

fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}
