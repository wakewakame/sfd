//! 膨大にある画像・動画ファイルの中から類似のファイルを検出するツール。

mod find;
mod media;
mod progress;
mod record;
mod scan;
mod walk;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
Usage: sfd <COMMAND> [OPTION]...

Commands:
  hash <DIRECTORY> <hash.json>   walk a directory and hash every image and video
  find <hash.json> <find.json>   group similar files using hash.json

Options for sfd hash:
  -j, --jobs N       number of files to process in parallel (default: CPU count)
      --no-resume    ignore an existing hash.json and start over
      --no-dihedral  do not record the rotated and flipped hashes. hash.json
                     gets smaller, but rotated copies can no longer be found.
                     These can only be produced while hashing, so wanting them
                     later means rescanning every file
      --ffmpeg PATH  ffmpeg executable to use (or set SFD_FFMPEG)
      --ffprobe PATH ffprobe executable to use (or set SFD_FFPROBE)

Options for sfd find:
  -t, --threshold N    largest Hamming distance still considered a match
                       (0..=256, default: 31)
      --min-quality N  skip files whose quality metric is below this (default: 0)

Common options:
  -h, --help         show this help

Errors do not stop the run; they are kept in hash.json as lines carrying an
\"error\" field. Progress is written to stderr.
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
        "find" => find::run(parse_find_args(args)?),
        other => Err(format!("unknown command: {other}")),
    }
}

/// `sfd hash` の設定。
pub struct HashOptions {
    pub root: PathBuf,
    pub output: PathBuf,
    pub jobs: usize,
    pub resume: bool,
    pub dihedral: bool,
    pub ffmpeg: String,
    pub ffprobe: String,
}

fn parse_hash_args(mut args: impl Iterator<Item = String>) -> Result<HashOptions, String> {
    let mut positional = Vec::new();
    let mut jobs = None;
    let mut resume = true;
    let mut dihedral = true;
    let mut ffmpeg = std::env::var("SFD_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string());
    let mut ffprobe = std::env::var("SFD_FFPROBE").unwrap_or_else(|_| "ffprobe".to_string());

    while let Some(arg) = args.next() {
        let mut take_value = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} requires a value"))
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
                        .ok_or_else(|| format!("--jobs takes a positive integer: {value}"))?,
                );
            }
            "--no-resume" => resume = false,
            "--no-dihedral" => dihedral = false,
            "--ffmpeg" => ffmpeg = take_value("--ffmpeg")?,
            "--ffprobe" => ffprobe = take_value("--ffprobe")?,
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option: {other}"))
            }
            _ => positional.push(arg),
        }
    }

    let [root, output] = positional.as_slice() else {
        return Err("expected: sfd hash <DIRECTORY> <hash.json>".to_string());
    };

    Ok(HashOptions {
        root: PathBuf::from(root),
        output: PathBuf::from(output),
        jobs: jobs.unwrap_or_else(default_jobs),
        resume,
        dihedral,
        ffmpeg,
        ffprobe,
    })
}

/// `sfd find` の設定。
pub struct FindOptions {
    pub input: PathBuf,
    pub output: PathBuf,
    pub threshold: u32,
    pub min_quality: u8,
}

fn parse_find_args(mut args: impl Iterator<Item = String>) -> Result<FindOptions, String> {
    let mut positional = Vec::new();
    // PDQ が推奨するしきい値。256 ビット中 31 ビット。
    let mut threshold = 31;
    let mut min_quality = 0;

    while let Some(arg) = args.next() {
        let mut take_value = |name: &str| -> Result<String, String> {
            args.next().ok_or_else(|| format!("{name} requires a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-t" | "--threshold" => {
                let value = take_value("--threshold")?;
                threshold = value
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n <= 256)
                    .ok_or_else(|| format!("--threshold takes a value in 0..=256: {value}"))?;
            }
            "--min-quality" => {
                let value = take_value("--min-quality")?;
                min_quality = value
                    .parse::<u8>()
                    .ok()
                    .filter(|n| *n <= 100)
                    .ok_or_else(|| format!("--min-quality takes a value in 0..=100: {value}"))?;
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option: {other}"))
            }
            _ => positional.push(arg),
        }
    }

    let [input, output] = positional.as_slice() else {
        return Err("expected: sfd find <hash.json> <find.json>".to_string());
    };

    Ok(FindOptions {
        input: PathBuf::from(input),
        output: PathBuf::from(output),
        threshold,
        min_quality,
    })
}

fn default_jobs() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}
