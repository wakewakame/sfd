//! `sfd hash` の本体。探索・ハッシュ計算・書き出しをまとめる。

use std::collections::HashMap;
use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::media::{self, Tools};
use crate::record::{self, Frame, Record};
use crate::walk::{self, Found, Kind};
use crate::HashOptions;

pub fn run(options: HashOptions) -> Result<ExitCode, String> {
    if !options.root.is_dir() {
        return Err(format!("ディレクトリではありません: {}", options.root.display()));
    }
    let root_label = options.root.display().to_string();

    // 既に記録済みのパスを拾う。--no-resume なら最初からやり直す。
    let existing = if options.resume {
        record::read_existing(&options.output)
            .map_err(|e| format!("{} を読めません: {e}", options.output.display()))?
    } else {
        None
    };

    if let Some(existing) = &existing {
        if existing.header.root != root_label {
            return Err(format!(
                "{} は別のディレクトリ ({}) の結果です。\n\
                 同じディレクトリを指定するか、--no-resume で最初からやり直してください",
                options.output.display(),
                existing.header.root
            ));
        }
        if existing.truncated_tail {
            eprintln!("sfd: 中断時に書きかけだった末尾の 1 行を切り捨てました");
        }
    }

    // 端末でなければ行を上書きする進捗を出さない。リダイレクト先に \r が
    // 混ざるとログとして読めなくなるため。
    let interactive = std::io::stderr().is_terminal();

    // 探索。件数が確定してからでないと進捗を割合で出せない。
    let walked = walk::walk(&options.root, |count| {
        if interactive && count % 1000 == 0 {
            eprint!("\r探索中... {count} 件");
        }
    });
    if interactive {
        clear_line();
    }
    eprintln!("探索完了: {} 件", walked.files.len());

    let done = existing.as_ref().map(|e| &e.done);
    let found_count = walked.files.len();
    let todo: Vec<Found> = walked
        .files
        .into_iter()
        .filter(|found| done.is_none_or(|done| !done.contains(&found.relative)))
        .collect();

    let skipped = found_count - todo.len();
    if skipped > 0 {
        eprintln!("うち {skipped} 件は記録済みなので飛ばします");
    }
    if todo.is_empty() && walked.unreadable.is_empty() {
        eprintln!("処理するファイルはありません");
        return Ok(ExitCode::SUCCESS);
    }

    let mut writer = match existing {
        Some(_) => record::Writer::append(&options.output),
        None => record::Writer::create(&options.output, &root_label),
    }
    .map_err(|e| format!("{} を開けません: {e}", options.output.display()))?;

    // 読めなかったものを先に書き出す。件数が少なく、ハッシュ計算より先に
    // 目に入った方が気づきやすい。
    for unreadable in &walked.unreadable {
        eprintln!("sfd: {}: {}", unreadable.relative, unreadable.error);
        let record = Record {
            path: unreadable.relative.clone(),
            bytes: None,
            mtime: unreadable.mtime.map(record::rfc3339),
            sha256: None,
            width: None,
            height: None,
            duration: None,
            pdq: None,
            error: Some(unreadable.error.clone()),
        };
        writer
            .write(&record)
            .map_err(|e| format!("{} に書き出せません: {e}", options.output.display()))?;
    }

    let tools = Tools { ffmpeg: options.ffmpeg.clone(), ffprobe: options.ffprobe.clone() };
    let summary = hash_all(&todo, &tools, options.jobs, &mut writer, options.dihedral, interactive)?;

    writer.flush().map_err(|e| format!("{} に書き出せません: {e}", options.output.display()))?;

    // 探索時に読めなかったものもエラーとして数える。数えないと、ディレクトリが
    // まるごと欠けているのに終了コードが 0 になり、誰も気づけない。
    let errors = summary.errors + walked.unreadable.len();
    eprintln!(
        "完了: {} 件 (エラー {errors} 件、内容が同一で再利用 {} 件) {:.1} 秒",
        summary.total + walked.unreadable.len(),
        summary.reused,
        summary.elapsed.as_secs_f64()
    );

    Ok(if errors > 0 { ExitCode::from(1) } else { ExitCode::SUCCESS })
}

struct Summary {
    total: usize,
    errors: usize,
    reused: usize,
    elapsed: Duration,
}

/// 計算結果のうち、内容が同一なら使い回せる部分。
#[derive(Clone)]
struct Computed {
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<f64>,
    pdq: Option<Vec<Frame>>,
    error: Option<String>,
}

fn hash_all(
    todo: &[Found],
    tools: &Tools,
    jobs: usize,
    writer: &mut record::Writer,
    dihedral: bool,
    interactive: bool,
) -> Result<Summary, String> {
    let started = Instant::now();
    let next = AtomicUsize::new(0);
    // 内容が同一のファイルは知覚ハッシュを計算し直さない。復号は 1 枚 470ms
    // かかるのに対し sha256 は 34MB で 17ms なので、完全重複が多いほど効く。
    let cache: Mutex<HashMap<String, Computed>> = Mutex::new(HashMap::new());
    let (sender, receiver) = mpsc::channel::<(Record, bool)>();

    let mut summary = Summary { total: 0, errors: 0, reused: 0, elapsed: Duration::ZERO };
    let mut write_error = None;

    std::thread::scope(|scope| {
        for _ in 0..jobs {
            let sender = sender.clone();
            let (next, cache) = (&next, &cache);
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(found) = todo.get(index) else { break };
                    let outcome = process(found, tools, dihedral, cache);
                    if sender.send(outcome).is_err() {
                        break; // 受け手が落ちた
                    }
                }
            });
        }
        // 全ワーカーが終わったときに受信ループが抜けられるよう、元の送信端は捨てる。
        drop(sender);

        let mut last_progress = Instant::now();
        for (record, reused) in receiver {
            summary.total += 1;
            if record.error.is_some() {
                summary.errors += 1;
            }
            if reused {
                summary.reused += 1;
            }

            // 途中で落ちても再開できるよう、1 件ごとに書き出す。
            if let Err(e) = writer.write(&record).and_then(|()| writer.flush()) {
                write_error = Some(format!("書き出しに失敗しました: {e}"));
                break;
            }

            if interactive && last_progress.elapsed() >= Duration::from_millis(200) {
                progress(summary.total, todo.len(), started.elapsed(), &record.path);
                last_progress = Instant::now();
            }
        }
    });

    if interactive {
        clear_line();
    }

    if let Some(e) = write_error {
        return Err(e);
    }

    summary.elapsed = started.elapsed();
    Ok(summary)
}

/// 進捗表示に使う桁数。行を消すときもこの幅で揃える。
const PROGRESS_WIDTH: usize = 78;

/// 書きかけの進捗行を消して行頭に戻る。
fn clear_line() {
    eprint!("\r{}\r", " ".repeat(PROGRESS_WIDTH));
}

fn progress(done: usize, total: usize, elapsed: Duration, current: &str) {
    let percent = if total == 0 { 100.0 } else { done as f64 * 100.0 / total as f64 };
    // 残り時間は「これまでの平均が続く」という前提の粗い見積もり。
    let remaining = if done == 0 {
        String::from("--")
    } else {
        let per_file = elapsed.as_secs_f64() / done as f64;
        format!("{:.0}s", per_file * (total - done) as f64)
    };
    let name = tail(current, 34);
    eprint!("\r{done}/{total} ({percent:.1}%) 残り約 {remaining}  {name:<34}");
    let _ = std::io::stderr().flush();
}

/// 進捗表示に収まるよう、長いパスは先頭を省く。
fn tail(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    format!("...{}", chars[chars.len() - (width - 3)..].iter().collect::<String>())
}

/// 1 ファイルを処理する。返り値の `bool` は計算結果を使い回したか。
fn process(
    found: &Found,
    tools: &Tools,
    dihedral: bool,
    cache: &Mutex<HashMap<String, Computed>>,
) -> (Record, bool) {
    let mut record = Record {
        path: found.relative.clone(),
        bytes: Some(found.bytes),
        mtime: Some(record::rfc3339(found.mtime)),
        sha256: None,
        width: None,
        height: None,
        duration: None,
        pdq: None,
        error: None,
    };

    let sha256 = match sha256_file(&found.path) {
        Ok(sha256) => sha256,
        Err(e) => {
            record.error = Some(format!("読み込みに失敗しました: {e}"));
            return (record, false);
        }
    };
    record.sha256 = Some(sha256.clone());

    let cached = cache.lock().ok().and_then(|cache| cache.get(&sha256).cloned());
    let (computed, reused) = match cached {
        Some(computed) => (computed, true),
        None => {
            let computed = compute(found, tools, dihedral);
            if let Ok(mut cache) = cache.lock() {
                cache.insert(sha256, computed.clone());
            }
            (computed, false)
        }
    };

    record.width = computed.width;
    record.height = computed.height;
    record.duration = computed.duration;
    record.pdq = computed.pdq;
    record.error = computed.error;
    (record, reused)
}

fn compute(found: &Found, tools: &Tools, dihedral: bool) -> Computed {
    let mut computed =
        Computed { width: None, height: None, duration: None, pdq: None, error: None };

    match found.kind {
        Kind::Image => match tools.decode_image(&found.path) {
            Ok(image) => {
                computed.width = Some(image.width as u32);
                computed.height = Some(image.height as u32);
                // 画像も「1 フレームの動画」として同じ形で持つ。
                match hash_frame(image, 0.0, dihedral) {
                    Ok(frame) => computed.pdq = Some(vec![frame]),
                    Err(e) => computed.error = Some(e),
                }
            }
            Err(e) => computed.error = Some(e),
        },
        Kind::Video => {
            let duration = match tools.probe_duration(&found.path) {
                Ok(duration) => duration,
                Err(e) => {
                    computed.error = Some(e);
                    return computed;
                }
            };
            computed.duration = Some(duration);

            let mut frames = Vec::new();
            for at in media::sample_times(duration) {
                match tools.decode_frame_at(&found.path, at) {
                    Ok(image) => {
                        computed.width.get_or_insert(image.width as u32);
                        computed.height.get_or_insert(image.height as u32);
                        match hash_frame(image, at, dihedral) {
                            Ok(frame) => frames.push(frame),
                            Err(e) => {
                                computed.error = Some(e);
                                return computed;
                            }
                        }
                    }
                    Err(e) => {
                        computed.error = Some(format!("{at:.1} 秒地点: {e}"));
                        return computed;
                    }
                }
            }
            computed.pdq = Some(frames);
        }
    }

    computed
}

fn hash_frame(mut image: pdq::Image, at: f64, dihedral: bool) -> Result<Frame, String> {
    // リファレンス CLI と同じ 512x512 の前処理。公式の期待値と揃ううえ、
    // Jarosz フィルタの計算量が入力画素数に比例するため桁違いに速い。
    pdq::preprocess::reference_downsample(&mut image);

    if !dihedral {
        let (hash, quality) = pdq::hash_image(&image).map_err(|e| e.to_string())?;
        return Ok(Frame { t: at, hash: hash.to_hex(), dihedral: None, quality });
    }

    // 回転・反転版は画像を変換し直すのではなく DCT 係数から導くので、
    // 追加コストはほぼゼロ。ただし計算時にしか作れないため、後から
    // 欲しくなると全ファイルの再スキャンになる。
    let (hashes, quality) = pdq::dihedral_image(&image).map_err(|e| e.to_string())?;
    let all = hashes.all();
    Ok(Frame {
        t: at,
        hash: all[0].1.to_hex(),
        dihedral: Some(all[1..].iter().map(|(_, hash)| hash.to_hex()).collect()),
        quality,
    })
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
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
    fn hashes_a_known_file() {
        let dir = std::env::temp_dir().join(format!("sfd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty");
        std::fs::write(&path, b"").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        std::fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
