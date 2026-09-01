//! ThreatExchange の公式回帰テストベクタとの突き合わせ。
//!
//! この移植がリファレンス実装とビット単位で一致することを確認する。
//! 画像はリポジトリに同梱せず、ThreatExchange のチェックアウトを指してもらう。
//!
//! ```sh
//! git clone --depth 1 --filter=blob:none --sparse https://github.com/facebook/ThreatExchange.git
//! cd ThreatExchange && git sparse-checkout set pdq && cd ..
//! PDQ_THREATEXCHANGE=$PWD/ThreatExchange cargo test --release
//! ```
//!
//! 環境変数が設定されていない場合はスキップする。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use pdq::{Hash256, Image};

// ================================================================
// 期待値の読み込み

fn read_expected(root: &Path) -> String {
    let expected_path = root.join("pdq/cpp/reg_test/expected/out");
    std::fs::read_to_string(&expected_path)
        .unwrap_or_else(|e| panic!("{} を読めません: {e}", expected_path.display()))
}

/// `key=value,key=value,...` 形式の行を分解する。
///
/// 期待値ファイルには複数の実行結果が混ざっており、同じファイル名が
/// 素のハッシュとしても二面体変種としても登場する。素朴に `hash,quality,path`
/// を拾うと取り違えるので、`--details` 形式の行だけを見る。
fn parse_details_line(line: &str) -> Option<BTreeMap<&str, &str>> {
    if !line.starts_with("hash=") {
        return None;
    }
    line.split(',').map(|field| field.split_once('=')).collect()
}

fn file_name_of(path: &str) -> Option<&str> {
    Path::new(path).file_name()?.to_str()
}

/// `pdq-photo-hasher --details` の出力から、ファイル名 -> (hash, quality) を作る。
fn load_expected(root: &Path) -> BTreeMap<String, (String, u8)> {
    let text = read_expected(root);

    let mut map: BTreeMap<String, (String, u8)> = BTreeMap::new();
    for line in text.lines() {
        let Some(fields) = parse_details_line(line) else {
            continue;
        };
        // 二面体変種の行 (xform=...) は除外する。
        let (Some(hash), Some(quality), Some(path), None) = (
            fields.get("hash"),
            fields.get("quality"),
            fields.get("filename"),
            fields.get("xform"),
        ) else {
            continue;
        };
        let (Ok(quality), Some(name)) = (quality.parse::<u8>(), file_name_of(path)) else {
            continue;
        };

        if let Some((prev_hash, prev_quality)) = map.get(name) {
            assert_eq!(
                (prev_hash.as_str(), *prev_quality),
                (*hash, quality),
                "{name} の期待値が期待値ファイル内で食い違っています"
            );
        }
        map.insert(name.to_string(), (hash.to_string(), quality));
    }
    assert!(!map.is_empty(), "期待値を 1 件も読み取れませんでした");
    map
}

/// `pdq-photo-hasher --pdqdih --details` の出力から、xform 名 -> hash を作る。
fn load_expected_dihedral(root: &Path, image_name: &str) -> BTreeMap<String, String> {
    let text = read_expected(root);

    let mut map = BTreeMap::new();
    for line in text.lines() {
        let Some(fields) = parse_details_line(line) else {
            continue;
        };
        let (Some(hash), Some(xform), Some(path)) =
            (fields.get("hash"), fields.get("xform"), fields.get("filename"))
        else {
            continue;
        };
        if file_name_of(path) != Some(image_name) {
            continue;
        }
        map.insert(xform.to_string(), hash.to_string());
    }
    assert_eq!(map.len(), 8, "{image_name} の二面体期待値が 8 件そろっていません");
    map
}

// ================================================================
// 画像の読み込み

/// 画像をデコードし、リファレンス CLI と同じ前処理をかける。
///
/// PDQ のハッシュ値は入力画素にそのまま依存するので、**どのデコーダを使うかで
/// 数ビット変わりうる**。リファレンスの期待値は ImageMagick (libjpeg) 経由で
/// 生成されているので、デコーダを揃えたいときは `PDQ_DECODER=magick` を指定する。
/// 既定は phashsum と同じ ffmpeg。
///
/// 実際には 512x512 の前処理がデコーダ差を吸収するため、どちらでも期待値と一致する。
fn load(path: &Path) -> Image {
    let decoder = std::env::var("PDQ_DECODER").unwrap_or_else(|_| "ffmpeg".into());

    let output = match decoder.as_str() {
        "ffmpeg" => Command::new(std::env::var("PHASHSUM_FFMPEG").unwrap_or_else(|_| "ffmpeg".into()))
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(path)
            .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "pam", "-"])
            .output(),
        "magick" => Command::new("magick").arg(path).args(["-depth", "8", "ppm:-"]).output(),
        other => panic!("PDQ_DECODER の値が不正です: {other}"),
    }
    .unwrap_or_else(|e| panic!("{decoder} を起動できません: {e}"));

    assert!(
        output.status.success(),
        "{decoder} が {} で失敗しました: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );

    let mut image = pdq::netpbm::parse(output.stdout)
        .unwrap_or_else(|e| panic!("{} の出力を読めません: {e}", path.display()));
    image.drop_alpha();
    pdq::preprocess::reference_downsample(&mut image);
    image
}

fn threatexchange_root() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("PDQ_THREATEXCHANGE")?);
    assert!(
        root.join("pdq/cpp/reg_test/expected/out").exists(),
        "PDQ_THREATEXCHANGE={} に pdq/cpp/reg_test/expected/out がありません",
        root.display()
    );
    Some(root)
}

fn collect_images(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_images(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg")) {
            out.push(path);
        }
    }
}

// ================================================================
// テスト

#[test]
fn matches_reference_hashes() {
    let Some(root) = threatexchange_root() else {
        eprintln!("PDQ_THREATEXCHANGE が未設定のためスキップします");
        return;
    };

    let expected = load_expected(&root);

    let mut images = Vec::new();
    collect_images(&root.join("pdq/data/reg-test-input"), &mut images);
    images.sort();
    assert!(!images.is_empty(), "テスト画像が見つかりません");

    let mut hasher = pdq::Hasher::new();
    let mut checked = 0;
    let mut failures = Vec::new();

    for path in &images {
        let name = path.file_name().unwrap().to_str().unwrap();
        let Some((expected_hash, expected_quality)) = expected.get(name) else {
            continue;
        };

        let (hash, quality) = hasher.hash_image(&load(path)).unwrap();

        checked += 1;
        if hash.to_hex() != *expected_hash || quality != *expected_quality {
            failures.push(format!(
                "{name}\n  期待: {expected_hash} quality={expected_quality}\n  実際: {hash} quality={quality}\n  距離: {} ビット",
                Hash256::from_hex(expected_hash).unwrap().hamming_distance(&hash)
            ));
        }
    }

    assert!(checked > 0, "照合できた画像が 1 枚もありませんでした");
    assert!(
        failures.is_empty(),
        "{} / {checked} 枚が不一致:\n{}",
        failures.len(),
        failures.join("\n")
    );
    eprintln!("{checked} 枚すべてリファレンスと一致しました");
}

/// 二面体版も公式の期待値と一致することを確認する。
#[test]
fn dihedral_matches_reference() {
    let Some(root) = threatexchange_root() else {
        eprintln!("PDQ_THREATEXCHANGE が未設定のためスキップします");
        return;
    };

    const IMAGE: &str = "bridge-1-original.jpg";
    let expected = load_expected_dihedral(&root, IMAGE);

    let dir = root.join("pdq/data/reg-test-input/dih");
    let (hashes, _) = pdq::dihedral_image(&load(&dir.join(IMAGE))).unwrap();

    // 変換名はリファレンスの --details 出力の略称に合わせる
    // (Dihedral::name() が返す名前とは別物)。
    let pairs = [
        ("orig", hashes.original),
        ("rot90", hashes.rotate90),
        ("rot180", hashes.rotate180),
        ("rot270", hashes.rotate270),
        ("flipx", hashes.flip_x),
        ("flipy", hashes.flip_y),
        ("flipp", hashes.flip_plus1),
        ("flipm", hashes.flip_minus1),
    ];

    let mut failures = Vec::new();
    for (xform, derived) in pairs {
        let expected_hash = expected
            .get(xform)
            .unwrap_or_else(|| panic!("xform={xform} の期待値がありません"));
        if derived.to_hex() != *expected_hash {
            failures.push(format!("xform={xform}\n  期待: {expected_hash}\n  実際: {derived}"));
        }
    }
    assert!(failures.is_empty(), "二面体版が不一致:\n{}", failures.join("\n"));

    // 実際に回転させた画像のハッシュとも近いはず (JPEG 再圧縮ぶんだけずれる)。
    let (rotated_hash, _) = pdq::hash_image(&load(&dir.join("bridge-2-rotate-90.jpg"))).unwrap();
    let distance = hashes.rotate90.hamming_distance(&rotated_hash);
    assert!(
        distance <= Hash256::MATCH_THRESHOLD,
        "導出した rot90 と実際に回転した画像のハッシュが離れすぎています: {distance} ビット"
    );
}
