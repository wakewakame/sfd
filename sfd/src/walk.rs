//! ディレクトリの再帰探索と、対象ファイルの判別。

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 対象ファイルの種類。
///
/// 拡張子で決めている。中身を見て判定する方が確実だが、対象外のファイルまで
/// 1 つずつ ffprobe を起動することになり、大量のファイルを扱う用途に合わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "avif",
    "jxl", "jp2", "ppm", "pgm", "pnm", "tga", "dds",
];

const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "avi", "mkv", "webm", "mpg", "mpeg", "wmv", "flv", "3gp", "3g2", "mts",
    "m2ts", "ts", "ogv", "asf", "rm", "rmvb",
];

/// 拡張子から種類を決める。対象外なら `None`。
pub fn classify(path: &Path) -> Option<Kind> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        Some(Kind::Image)
    } else if VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        Some(Kind::Video)
    } else {
        None
    }
}

/// 探索で見つかった 1 ファイル。
#[derive(Debug, Clone)]
pub struct Found {
    pub path: PathBuf,
    /// 探索の起点からの相対パス。区切りは `/` に正規化してある。
    pub relative: String,
    pub kind: Kind,
    pub bytes: u64,
    pub mtime: SystemTime,
}

/// 探索の結果。
pub struct Walked {
    pub files: Vec<Found>,
    /// 読めなかったディレクトリ。権限がないなど。
    pub unreadable: Vec<(PathBuf, io::Error)>,
}

/// `root` 以下を再帰的に探索して、対象ファイルを集める。
///
/// 読めなかったものは [`Walked::unreadable`] に集めて探索は続ける。1 つ読めない
/// ディレクトリがあるだけで全体が止まっては困るので、この関数自体は失敗しない。
///
/// ディレクトリへのシンボリックリンクは辿らない。辿ると循環しうるし、
/// 同じファイルを二重に数えることにもなるため。
///
/// `on_progress` は見つかった件数が増えるたびに呼ばれる。
pub fn walk(root: &Path, mut on_progress: impl FnMut(usize)) -> Walked {
    let mut walked = Walked { files: Vec::new(), unreadable: Vec::new() };
    let mut stack = vec![root.to_path_buf()];

    while let Some(directory) = stack.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(e) => {
                walked.unreadable.push((directory, e));
                continue;
            }
        };

        // 探索順が実行のたびに変わらないよう、ディレクトリ内は名前順に揃える。
        let mut children: Vec<PathBuf> = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    walked.unreadable.push((directory.clone(), e));
                    continue;
                }
            };
            children.push(entry.path());
        }
        children.sort();

        for path in children {
            // symlink_metadata なのでリンク自体を見る (リンク先は辿らない)。
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(e) => {
                    walked.unreadable.push((path, e));
                    continue;
                }
            };

            if metadata.is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }

            let Some(kind) = classify(&path) else {
                continue;
            };
            let Some(relative) = relative_path(root, &path) else {
                continue;
            };

            walked.files.push(Found {
                relative,
                kind,
                bytes: metadata.len(),
                mtime: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                path,
            });
            on_progress(walked.files.len());
        }
    }

    // スタックで掘っているので順序が入り組む。決定的にするため最後に揃える。
    walked.files.sort_by(|a, b| a.relative.cmp(&b.relative));
    walked
}

/// `root` からの相対パスを `/` 区切りの文字列にする。
///
/// Windows の `\` を `/` に直しておくと、別の OS で hash.json を読んでも
/// パスが一致する。
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for component in relative.components() {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(component.as_os_str().to_str()?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_extension_case_insensitively() {
        assert_eq!(classify(Path::new("a.jpg")), Some(Kind::Image));
        assert_eq!(classify(Path::new("a.JPG")), Some(Kind::Image));
        assert_eq!(classify(Path::new("a.HEIC")), Some(Kind::Image));
        assert_eq!(classify(Path::new("a.mp4")), Some(Kind::Video));
        assert_eq!(classify(Path::new("a.MKV")), Some(Kind::Video));
        assert_eq!(classify(Path::new("a.txt")), None);
        assert_eq!(classify(Path::new("a")), None);
        assert_eq!(classify(Path::new(".jpg")), None); // 拡張子ではなく隠しファイル名
    }

    #[test]
    fn relative_paths_use_forward_slashes() {
        let root = Path::new("/tmp/root");
        assert_eq!(relative_path(root, Path::new("/tmp/root/a.jpg")).unwrap(), "a.jpg");
        assert_eq!(relative_path(root, Path::new("/tmp/root/x/y/a.jpg")).unwrap(), "x/y/a.jpg");
        assert_eq!(relative_path(root, Path::new("/tmp/other/a.jpg")), None);
    }
}
