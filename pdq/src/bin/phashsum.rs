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
使い方: phashsum [オプション] ファイル...

PDQ 知覚ハッシュを計算して出力する。デコードには ffmpeg を用いる。

ファイルに - を指定すると、デコード済みの Netpbm (P5/P6/P7) を標準入力から読む。

オプション:
  -d, --dihedral       回転・反転版を含む 8 通りのハッシュを出力する
  -q, --quality        品質指標 (0..=100) も出力する
  -j, --json           1 行 1 JSON で出力する (エラーも同じ行形式で残る)
      --full-resolution
                       512x512 への前処理を行わず、元の解像度のままハッシュする。
                       既定ではリファレンス CLI と同じ前処理をかけており、
                       これを外すと値が公式の期待値と食い違ううえ、非常に遅くなる
      --ss TIME        動画のこの位置のフレームを使う (例: 00:00:10, 12.5)
      --ffmpeg PATH    ffmpeg の実行ファイルを指定する (環境変数 PHASHSUM_FFMPEG でも可)
  -h, --help           この使い方を表示する

出力形式 (既定):
  <64 桁の 16 進>  <パス>

エラーが起きても処理は止めず、次のファイルに進む。1 件でも失敗していれば
終了コードは 1 になる。--json のときはエラーも \"error\" を持つ行として残る。
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
        eprintln!("phashsum: 標準出力への書き込みに失敗しました: {e}");
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
        .map_err(|e| format!("標準入力を読めませんでした: {e}"))?;
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
        .map_err(|e| format!("ffmpeg ({}) を起動できませんでした: {e}", opts.ffmpeg))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.lines().last().unwrap_or("詳細不明").trim();
        return Err(format!("ffmpeg が失敗しました: {detail}"));
    }
    if output.stdout.is_empty() {
        return Err("ffmpeg がフレームを出力しませんでした".to_string());
    }

    pdq::netpbm::parse(output.stdout).map_err(|e| e.to_string())
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
                .ok_or_else(|| format!("{name} には値が必要です"))
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
            other => return Err(format!("不明なオプションです: {other}")),
        }
    }

    if opts.paths.is_empty() {
        return Err("ファイルが指定されていません".to_string());
    }

    Ok(Some(opts))
}
