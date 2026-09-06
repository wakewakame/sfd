//! `sha256sum` 風に PDQ 知覚ハッシュを出力するコマンド。
//!
//! 画像・動画のデコードは ffmpeg の子プロセスに任せ、PAM (P7) 形式で受け取る。
//! PAM はヘッダに寸法とチャンネル数を持つので、ffprobe を別に呼ぶ必要がない。
//! この構成のおかげで Rust 側の依存クレートはゼロで、ffmpeg が扱える形式は
//! すべて (HEIC も動画も) そのまま渡せる。
//!
//! エラーは 1 件ずつ記録して次のファイルに進む。1 件でも失敗していれば
//! 終了コードは 1 になるが、処理そのものは中断しない。

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use pdq::{DihedralHashes, Hash256, Hasher, Image};

const USAGE: &str = "\
Usage: phashsum [OPTION]... FILE...

Print the PDQ perceptual hash of each file. Decoding is done by ffmpeg.

Use - as the file name to read an already decoded Netpbm (P5/P6/P7) from stdin.

Options:
  -d, --dihedral       print all 8 hashes, including rotations and flips
  -q, --quality        also print the quality metric (0..=100)
  -j, --json           print one JSON object per line (errors included)
      --full-resolution
                       hash at the original resolution instead of downsampling
                       to 512x512 first. The default matches the reference CLI,
                       so passing this makes the hashes disagree with the
                       published expectations, and it is far slower
      --ss TIME        use the frame at this position of a video (e.g. 00:00:10)
      --ffmpeg PATH    ffmpeg executable to use (or set PHASHSUM_FFMPEG)
  -h, --help           show this help

Output format (default):
  <64 hex digits>  <path>

Errors do not stop the run; processing continues with the next file. The exit
status is 1 if any file failed. With --json, errors are kept as lines carrying
an \"error\" field.
";


struct Options {
    dihedral: bool,
    quality: bool,
    json: bool,
    full_resolution: bool,
    seek: Option<String>,
    ffmpeg: String,
    paths: Vec<PathBuf>,
}

fn main() {
    let opts = match parse_args() {
        Ok(Some(opts)) => opts,
        Ok(None) => return,
        Err(message) => {
            eprintln!("phashsum: {message}");
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    };

    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut hasher = Hasher::new();
    let mut saw_error = false;

    for path in &opts.paths {
        // 個々の書き込みエラーは無視してよい。BufWriter なので実際の失敗は
        // 最後の flush でまとめて表面化する。
        let _ = match hash_one(path, &opts, &mut hasher) {
            Ok(outcome) => write_success(&mut out, path, &outcome, &opts),
            Err(message) => {
                saw_error = true;
                write_failure(&mut out, path, &message, &opts)
            }
        };
    }

    if let Err(e) = out.flush() {
        eprintln!("phashsum: cannot write to stdout: {e}");
        saw_error = true;
    }

    if saw_error {
        std::process::exit(1);
    }
}

// ================================================================
// ハッシュ計算

enum Outcome {
    Single(Hash256, u8),
    Dihedral(Box<DihedralHashes>, u8),
}

fn hash_one(path: &Path, opts: &Options, hasher: &mut Hasher) -> Result<Outcome, String> {
    let mut image = if path.as_os_str() == "-" {
        read_netpbm_from_stdin()
    } else {
        decode_with_ffmpeg(path, opts)
    }?;

    // ハッシュが受け付けるのは 1 チャンネルか 3 チャンネルだけ。
    image.drop_alpha();

    // 既定ではリファレンス CLI と同じ 512x512 の前処理をかける。値が公式の期待値と
    // 揃ううえ、Jarosz フィルタの計算量が入力画素数に比例するため桁違いに速い。
    if !opts.full_resolution {
        pdq::preprocess::reference_downsample(&mut image);
    }

    if opts.dihedral {
        let (hashes, quality) = hasher.dihedral_image(&image).map_err(|e| e.to_string())?;
        Ok(Outcome::Dihedral(Box::new(hashes), quality))
    } else {
        let (hash, quality) = hasher.hash_image(&image).map_err(|e| e.to_string())?;
        Ok(Outcome::Single(hash, quality))
    }
}

/// デコード済みの Netpbm を標準入力から受け取る。
///
/// ffmpeg を 1 度だけ起動して複数フレームを流し込みたい場合や、
/// デコーダを差し替えて検証したい場合に使う。
fn read_netpbm_from_stdin() -> Result<Image, String> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read stdin: {e}"))?;
    pdq::netpbm::parse(bytes).map_err(|e| e.to_string())
}

fn decode_with_ffmpeg(path: &Path, opts: &Options) -> Result<Image, String> {
    let mut command = Command::new(&opts.ffmpeg);
    command.arg("-v").arg("error").arg("-nostdin");
    if let Some(seek) = &opts.seek {
        command.arg("-ss").arg(seek);
    }
    // ストリームも画素形式も明示せず、ffmpeg の既定選択に任せているのは意図的:
    //
    // * iPhone の HEIC は画像がタイルに分割されて数百のストリームとして見える。
    //   -map 0:v:0 を付けると先頭のタイルだけを掴んでしまう。既定選択なら
    //   グリッドを再構成した本来の画像が得られる。
    // * そのグリッド再構成は内部で complex filtergraph を使うため、-vf を
    //   併用すると "Simple and complex filtering cannot be used together" で失敗する。
    //   画素形式は pam エンコーダとの自動交渉に任せれば、グレースケールは
    //   グレースケールのまま (DEPTH 1)、カラーは RGB (DEPTH 3) で出てくる。
    command
        .arg("-i")
        .arg(path)
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("image2pipe")
        .arg("-c:v")
        .arg("pam")
        .arg("-");

    let output = command
        .output()
        .map_err(|e| format!("cannot run ffmpeg ({}): {e}", opts.ffmpeg))?;

    if !output.status.success() {
        return Err(format!("ffmpeg failed: {}", error_message(&output.stderr)));
    }
    if output.stdout.is_empty() {
        return Err("ffmpeg produced no frame".to_string());
    }

    pdq::netpbm::parse(output.stdout).map_err(|e| e.to_string())
}

/// ffmpeg の stderr から、表示に使う 1 行を取り出す。
///
/// 先頭の行を取るのは、そこに根本原因が出るため。後ろの行は「結局何も出力
/// されなかった」といった結果の報告になりがちで、原因が分からない。
/// `[mjpeg @ 0x7f...]` のような接頭辞は実行のたびに変わるポインタを含むので落とす。
///
/// sfd 側の media.rs にも同じ処理がある。pdq は依存クレートを持たない方針で
/// sfd と共有できないため、意図して重複させている。
fn error_message(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .lines()
        .map(strip_context_prefixes)
        .find(|line| !line.is_empty())
        .unwrap_or_else(|| "no details".to_string())
}

fn strip_context_prefixes(line: &str) -> String {
    let mut rest = line.trim();
    while let Some(stripped) = rest.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((_, after)) => rest = after.trim_start(),
            None => break,
        }
    }
    rest.to_string()
}

// ================================================================
// 出力

fn write_success(
    out: &mut impl Write,
    path: &Path,
    outcome: &Outcome,
    opts: &Options,
) -> io::Result<()> {
    let path = path.display().to_string();

    match (outcome, opts.json) {
        (Outcome::Single(hash, quality), true) => writeln!(
            out,
            "{{\"path\":{},\"pdq\":\"{hash}\",\"quality\":{quality}}}",
            json_string(&path)
        ),
        (Outcome::Dihedral(hashes, quality), true) => {
            let variants: Vec<String> = hashes
                .all()
                .iter()
                .map(|(kind, hash)| format!("{}:\"{hash}\"", json_string(kind.name())))
                .collect();
            writeln!(
                out,
                "{{\"path\":{},\"pdq\":{{{}}},\"quality\":{quality}}}",
                json_string(&path),
                variants.join(",")
            )
        }
        (Outcome::Single(hash, quality), false) => {
            writeln!(out, "{hash}{}  {path}", quality_column(*quality, opts))
        }
        (Outcome::Dihedral(hashes, quality), false) => {
            for (kind, hash) in hashes.all() {
                writeln!(
                    out,
                    "{hash}{}  {:<13}{path}",
                    quality_column(*quality, opts),
                    kind.name()
                )?;
            }
            Ok(())
        }
    }
}

fn quality_column(quality: u8, opts: &Options) -> String {
    if opts.quality {
        format!("  {quality:3}")
    } else {
        String::new()
    }
}

fn write_failure(
    out: &mut impl Write,
    path: &Path,
    message: &str,
    opts: &Options,
) -> io::Result<()> {
    let path = path.display().to_string();

    if opts.json {
        // エラーも同じ 1 行 1 JSON で残す。呼び出し側は "error" の有無で判別できる。
        writeln!(out, "{{\"path\":{},\"error\":{}}}", json_string(&path), json_string(message))
    } else {
        // 成功行だけを読む下流を壊さないよう、エラーは標準エラー出力に出す。
        eprintln!("phashsum: {path}: {message}");
        Ok(())
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ================================================================
// 引数

fn parse_args() -> Result<Option<Options>, String> {
    let mut opts = Options {
        dihedral: false,
        quality: false,
        json: false,
        full_resolution: false,
        seek: None,
        ffmpeg: std::env::var("PHASHSUM_FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string()),
        paths: Vec::new(),
    };

    let mut args = std::env::args_os().skip(1);
    let mut no_more_flags = false;

    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy().into_owned();

        // "-" 単体は標準入力を指すファイル名として扱う。
        if no_more_flags || !text.starts_with('-') || text == "-" {
            opts.paths.push(PathBuf::from(arg));
            continue;
        }

        let mut take_value = |name: &str| -> Result<String, String> {
            args.next()
                .map(|v| v.to_string_lossy().into_owned())
                .ok_or_else(|| format!("{name} requires a value"))
        };

        match text.as_str() {
            "--" => no_more_flags = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-d" | "--dihedral" => opts.dihedral = true,
            "-q" | "--quality" => opts.quality = true,
            "-j" | "--json" => opts.json = true,
            "--full-resolution" => opts.full_resolution = true,
            "--ss" => opts.seek = Some(take_value("--ss")?),
            "--ffmpeg" => opts.ffmpeg = take_value("--ffmpeg")?,
            other => return Err(format!("unknown option: {other}")),
        }
    }

    if opts.paths.is_empty() {
        return Err("no files given".to_string());
    }

    Ok(Some(opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// sfd 側の media.rs に同じ処理があり、そちらにも同じテストがある。
    /// 片方だけ直して食い違うことが実際に起きたので、両方で固定しておく。
    #[test]
    fn picks_the_first_meaningful_stderr_line() {
        assert_eq!(error_message(b"a\nb\n"), "a");
        assert_eq!(error_message(b"\n\nb\n"), "b");
        assert_eq!(error_message(b""), "no details");
    }

    /// 実行のたびに変わるポインタを含む接頭辞は落とす。
    #[test]
    fn strips_context_prefixes() {
        assert_eq!(
            error_message(b"[mjpeg @ 0xa1ec4c380] No JPEG data found in image\n"),
            "No JPEG data found in image"
        );
        assert_eq!(
            error_message(b"[vist#0:0/mjpeg @ 0x1] [dec:mjpeg @ 0x2] Error submitting packet\n"),
            "Error submitting packet"
        );
        // 閉じ括弧がなければ、無理に削らずそのまま残す。
        assert_eq!(error_message(b"[unterminated\n"), "[unterminated");
    }
}
