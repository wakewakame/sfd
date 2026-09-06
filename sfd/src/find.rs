//! `sfd find` の本体。hash.json から似ているファイルをまとめる。
//!
//! 先頭のファイルから順に、しきい値以内にある他のファイルを列挙する。一度どこかの
//! グループに現れたファイルは、以降のグループには現れない。
//!
//! 推移閉包 (連結成分) を取らないのは、「似ている」が推移律を満たさないため。
//! 少しずつ切り詰めた 9 枚で試すと、隣どうしは 10〜24 ビットしか離れていないのに
//! 両端は 90 ビット離れており、連結成分にするとその 2 枚が同じグループに入る。
//! それでは「どれを残すか」を判断できない。先頭からの 1 ホップだけを見ることで、
//! グループ内の全員が先頭から一定距離以内であることを保証している。

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use serde::Serialize;

use crate::progress::Reporter;
use crate::record::{self, Record, DIHEDRAL_NAMES};
use crate::FindOptions;

/// 出力の形式のバージョン。
const FORMAT_VERSION: u32 = 1;

pub fn run(options: FindOptions) -> Result<ExitCode, String> {
    let records = record::read_all(&options.input)
        .map_err(|e| format!("{} を読めません: {e}", options.input.display()))?;

    let total = records.len();
    let mut entries: Vec<Entry> = records
        .into_iter()
        .filter_map(|record| Entry::from_record(record, options.min_quality))
        .collect();
    eprintln!("比較対象: {} 件 (hash.json の {total} 件のうち)", entries.len());

    // 走査順で結果が変わる方式なので、順序を固定しないと実行のたびに出力が変わる。
    // 大きいものを先頭に持ってくると、残す候補がグループの先頭に来る。
    entries.sort_by(|a, b| {
        b.bytes
            .cmp(&a.bytes)
            .then(b.pixels().cmp(&a.pixels()))
            .then(a.path.cmp(&b.path))
    });

    let mut reporter = Reporter::new();
    let groups = group(&entries, options.threshold, &mut reporter);
    reporter.finish();
    let grouped: usize = groups.iter().map(|g| 1 + g.similar.len()).sum();

    write_output(&options.output, options.threshold, &groups)
        .map_err(|e| format!("{} に書き出せません: {e}", options.output.display()))?;

    eprintln!(
        "似ているファイル: {grouped} 件が {} グループ {:.1} 秒",
        groups.len(),
        reporter.elapsed().as_secs_f64()
    );
    Ok(ExitCode::SUCCESS)
}

/// hash.json と同じく 1 行 1 JSON で書く。
fn write_output(path: &Path, threshold: u32, groups: &[Group<'_>]) -> std::io::Result<()> {
    let mut out = std::io::BufWriter::new(std::fs::File::create(path)?);
    let header = Header { v: FORMAT_VERSION, threshold };
    serde_json::to_writer(&mut out, &header)?;
    out.write_all(b"\n")?;
    for group in groups {
        serde_json::to_writer(&mut out, group)?;
        out.write_all(b"\n")?;
    }
    out.flush()
}

// ================================================================
// 出力の形

#[derive(Serialize)]
struct Header {
    v: u32,
    threshold: u32,
}

/// 1 グループ。`path` に似ているファイルが `similar` に並ぶ。
#[derive(Serialize)]
struct Group<'a> {
    path: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    similar: Vec<Similar<'a>>,
}

/// グループの先頭に似ていたファイル。
///
/// 解像度やサイズをファイルごとに開き直さずに比較できるよう、判断材料は
/// ここにも持たせる。
#[derive(Serialize)]
struct Similar<'a> {
    path: &'a str,
    /// フレームごとの距離のうち最も大きいもの。全フレームがこの値以内。
    distance: u32,
    /// sha256 が一致する。見た目を比べるまでもなく片方を消してよい。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    identical: bool,
    /// 元の向きではなく回転・反転させた状態で一致した場合、その変換名。
    #[serde(skip_serializing_if = "Option::is_none")]
    transform: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
}

// ================================================================
// 比較対象

/// 256 ビットのハッシュ。比較を速くするため 64 ビット 4 本で持つ。
type Bits = [u64; 4];

struct FrameHashes {
    /// 元の向き。相手と突き合わせるのはこちら。
    original: Bits,
    /// 元の向きと回転・反転版。自分の側だけ変種を試せば足りる。
    variants: Vec<Bits>,
}

struct Entry {
    path: String,
    bytes: Option<u64>,
    width: Option<u32>,
    height: Option<u32>,
    sha256: Option<String>,
    /// 画像と動画は比較しない。`duration` の有無で決まる。
    is_video: bool,
    frames: Vec<FrameHashes>,
}

impl Entry {
    fn from_record(record: Record, min_quality: u8) -> Option<Self> {
        if record.error.is_some() {
            return None;
        }
        let pdq = record.pdq?;
        if pdq.is_empty() {
            return None;
        }
        // のっぺりした画像は偶然の一致を起こしやすいので、必要なら足切りする。
        if pdq.iter().any(|frame| frame.quality < min_quality) {
            return None;
        }

        let mut frames = Vec::with_capacity(pdq.len());
        for frame in &pdq {
            let original = parse(&frame.hash)?;
            let mut variants = vec![original];
            for hash in frame.dihedral.iter().flatten() {
                variants.push(parse(hash)?);
            }
            frames.push(FrameHashes { original, variants });
        }

        Some(Entry {
            path: record.path,
            bytes: record.bytes,
            width: record.width,
            height: record.height,
            sha256: record.sha256,
            is_video: record.duration.is_some(),
            frames,
        })
    }

    fn pixels(&self) -> u64 {
        self.width.unwrap_or(0) as u64 * self.height.unwrap_or(0) as u64
    }
}

fn parse(hex: &str) -> Option<Bits> {
    let hash = pdq::Hash256::from_hex(hex).ok()?;
    let words = hash.words();
    Some(std::array::from_fn(|i| {
        (0..4).fold(0u64, |bits, j| bits | (words[i * 4 + j] as u64) << (j * 16))
    }))
}

#[inline]
fn distance(a: &Bits, b: &Bits) -> u32 {
    (a[0] ^ b[0]).count_ones()
        + (a[1] ^ b[1]).count_ones()
        + (a[2] ^ b[2]).count_ones()
        + (a[3] ^ b[3]).count_ones()
}

/// 一致したときの、距離と変換。
struct Match {
    distance: u32,
    transform: Option<&'static str>,
}

/// `a` と `b` が似ているか判定する。
///
/// 動画は 3 箇所すべてが、対応する位置どうしで一致することを要求する。1 箇所でも
/// 一致すれば可とすると、黒画面や白画面がありふれているぶん偽陽性が増える。
///
/// 回転・反転は **1 つの変換で全フレームが揃うこと**を要求する。フレームごとに
/// 別々の変換を許すと、実際には起こりえない組み合わせ (1 枚目は 90 度回転、
/// 2 枚目は左右反転) で一致してしまい、各フレームが 8 回ずつ独立に試行するぶん
/// 偽陽性が増える。
fn compare(a: &Entry, b: &Entry, threshold: u32) -> Option<Match> {
    if a.is_video != b.is_video || a.frames.len() != b.frames.len() {
        return None;
    }

    let variants = a.frames.iter().map(|frame| frame.variants.len()).min()?;
    let mut best: Option<Match> = None;

    for at in 0..variants {
        // 自分の変種を相手の元の向きにぶつける。両方の変種を総当たりする必要はない。
        let mut worst = 0;
        let matched = a.frames.iter().zip(&b.frames).all(|(frame_a, frame_b)| {
            let d = distance(&frame_a.variants[at], &frame_b.original);
            worst = worst.max(d);
            d <= threshold
        });
        if !matched {
            continue;
        }
        // 同じ距離なら元の向きを優先する (at = 0 から見ているので自然にそうなる)。
        if best.as_ref().is_none_or(|found| worst < found.distance) {
            let transform = (at > 0).then(|| DIHEDRAL_NAMES[at - 1]);
            best = Some(Match { distance: worst, transform });
        }
    }

    best
}

/// 貪欲法でグループを作る。先頭から順に、まだどのグループにも入っていない
/// ファイルのうち、しきい値以内のものを集める。
fn group<'a>(
    entries: &'a [Entry],
    threshold: u32,
    reporter: &mut Reporter,
) -> Vec<Group<'a>> {
    let mut taken = vec![false; entries.len()];
    let mut groups = Vec::new();

    for head in 0..entries.len() {
        reporter.update(head, entries.len(), &entries[head].path);
        if taken[head] {
            continue;
        }
        taken[head] = true;

        let mut similar = Vec::new();
        for other in head + 1..entries.len() {
            if taken[other] {
                continue;
            }
            let Some(matched) = compare(&entries[head], &entries[other], threshold) else {
                continue;
            };
            taken[other] = true;
            similar.push(Similar {
                path: &entries[other].path,
                distance: matched.distance,
                identical: entries[head].sha256.is_some()
                    && entries[head].sha256 == entries[other].sha256,
                transform: matched.transform,
                bytes: entries[other].bytes,
                width: entries[other].width,
                height: entries[other].height,
            });
        }

        // 似たファイルが 1 つもないものは出力しない。
        if similar.is_empty() {
            continue;
        }
        groups.push(Group {
            path: &entries[head].path,
            bytes: entries[head].bytes,
            width: entries[head].width,
            height: entries[head].height,
            similar,
        });
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 指定したビット数だけ元と違うハッシュを作る。
    fn bits(differing: u32) -> Bits {
        let mut out = [0u64; 4];
        for i in 0..differing {
            out[(i / 64) as usize] |= 1 << (i % 64);
        }
        out
    }

    fn entry(path: &str, is_video: bool, frames: Vec<Bits>) -> Entry {
        Entry {
            path: path.into(),
            bytes: Some(100),
            width: Some(10),
            height: Some(10),
            sha256: None,
            is_video,
            frames: frames
                .into_iter()
                .map(|original| FrameHashes { original, variants: vec![original] })
                .collect(),
        }
    }

    #[test]
    fn images_and_videos_are_never_compared() {
        let image = entry("a.jpg", false, vec![bits(0)]);
        let video = entry("b.mp4", true, vec![bits(0)]);
        assert!(compare(&image, &video, 31).is_none());
    }

    /// フレーム数が違うものは比較できない。将来サンプリング方法を変えたときに
    /// 古い記録と新しい記録が混ざっても、黙って誤った比較をしないため。
    #[test]
    fn differing_frame_counts_are_not_compared() {
        let a = entry("a.mp4", true, vec![bits(0), bits(0), bits(0)]);
        let b = entry("b.mp4", true, vec![bits(0), bits(0)]);
        assert!(compare(&a, &b, 31).is_none());
    }

    /// 動画は 3 箇所すべてが一致することを要求する。黒画面や白画面が
    /// ありふれているので、1 箇所の一致では偽陽性が増える。
    #[test]
    fn videos_need_every_frame_to_match() {
        let a = entry("a.mp4", true, vec![bits(0), bits(0), bits(0)]);

        let all_close = entry("b.mp4", true, vec![bits(10), bits(20), bits(30)]);
        assert_eq!(compare(&a, &all_close, 31).unwrap().distance, 30);

        // 3 箇所目だけ離れていれば一致としない。
        let one_far = entry("c.mp4", true, vec![bits(10), bits(20), bits(40)]);
        assert!(compare(&a, &one_far, 31).is_none());
    }

    /// 距離はフレームごとの最大値。「全フレームがこの値以内」を意味する。
    #[test]
    fn distance_is_the_worst_frame() {
        let a = entry("a.mp4", true, vec![bits(0), bits(0), bits(0)]);
        let b = entry("b.mp4", true, vec![bits(4), bits(28), bits(12)]);
        assert_eq!(compare(&a, &b, 31).unwrap().distance, 28);
    }

    #[test]
    fn rotated_copies_match_through_a_variant() {
        let rotated = bits(200); // 元とは大きく違うハッシュ
        let mut a = entry("a.jpg", false, vec![bits(0)]);
        a.frames[0].variants.push(rotated); // rotate90 相当

        let b = entry("b.jpg", false, vec![rotated]);
        let matched = compare(&a, &b, 31).unwrap();
        assert_eq!(matched.distance, 0);
        assert_eq!(matched.transform, Some("rotate90"));

        // 元の向きで一致した場合は変換名を付けない。
        let c = entry("c.jpg", false, vec![bits(0)]);
        assert_eq!(compare(&a, &c, 31).unwrap().transform, None);
    }

    /// 回転・反転は 1 つの変換で全フレームが揃うことを要求する。フレームごとに
    /// 別々の変換で一致するのは実際には起こりえないので、弾く。
    #[test]
    fn one_transform_must_explain_every_frame() {
        let (x, y) = (bits(100), bits(200));

        // a の 1 枚目は変種 1 が x に、2 枚目は変種 2 が y に一致する。
        let mut a = entry("a.mp4", true, vec![bits(0), bits(0)]);
        a.frames[0].variants = vec![bits(0), x, bits(50)];
        a.frames[1].variants = vec![bits(0), bits(50), y];
        let b = entry("b.mp4", true, vec![x, y]);
        assert!(
            compare(&a, &b, 31).is_none(),
            "フレームごとに違う変換を使う組み合わせを通してはいけない"
        );

        // 同じ変種 1 で両フレームが揃うなら一致とする。
        let mut c = entry("c.mp4", true, vec![bits(0), bits(0)]);
        c.frames[0].variants = vec![bits(0), x];
        c.frames[1].variants = vec![bits(0), y];
        let matched = compare(&c, &b, 31).unwrap();
        assert_eq!(matched.transform, Some("rotate90"));
    }

    /// 元の向きでも変種でも一致する場合は、元の向きを優先する。
    #[test]
    fn the_original_orientation_wins_ties() {
        let mut a = entry("a.jpg", false, vec![bits(0)]);
        a.frames[0].variants.push(bits(0)); // 変種も同じ距離で一致する
        let b = entry("b.jpg", false, vec![bits(0)]);
        assert_eq!(compare(&a, &b, 31).unwrap().transform, None);
    }

    /// 連鎖するデータを 1 つのグループにまとめない。隣どうしは近いが両端が
    /// 遠い並びで、推移閉包を取ると距離の離れた 2 つが同居してしまう。
    #[test]
    fn chains_are_split_instead_of_merged() {
        // 0, 20, 40, 60 ビット。隣どうしは 20 だがしきい値 31 で両端は 60。
        let entries: Vec<Entry> = (0..4)
            .map(|i| entry(&format!("{i}.jpg"), false, vec![bits(i * 20)]))
            .collect();

        let groups = group(&entries, 31, &mut Reporter::new());
        assert_eq!(groups.len(), 2, "{groups:?} は 2 グループになるはず", groups = groups.len());
        assert_eq!(groups[0].path, "0.jpg");
        assert_eq!(groups[0].similar.len(), 1); // 20 ビット差のものだけ
        assert_eq!(groups[1].path, "2.jpg");
    }

    /// 似たファイルが 1 つもないものは出力しない。
    #[test]
    fn lone_files_are_omitted() {
        let entries = vec![
            entry("a.jpg", false, vec![bits(0)]),
            entry("b.jpg", false, vec![bits(100)]),
            entry("c.jpg", false, vec![bits(200)]),
        ];
        assert!(group(&entries, 31, &mut Reporter::new()).is_empty());
    }

    /// 一度どこかのグループに入ったファイルは、以降のグループに現れない。
    #[test]
    fn each_file_appears_in_at_most_one_group() {
        let entries: Vec<Entry> = (0..5)
            .map(|i| entry(&format!("{i}.jpg"), false, vec![bits(i * 4)]))
            .collect();

        let groups = group(&entries, 31, &mut Reporter::new());
        let mut seen = std::collections::HashSet::new();
        for group in &groups {
            assert!(seen.insert(group.path));
            for similar in &group.similar {
                assert!(seen.insert(similar.path), "{} が複数のグループに現れた", similar.path);
            }
        }
    }

    #[test]
    fn hex_parsing_roundtrips_through_bits() {
        let hex = "d8f8f0cee0f4a84f0637022a078f67f0b36e2ed596621e1d33e6339c4e9c9b22";
        let parsed = parse(hex).unwrap();
        let zero = parse(&"0".repeat(64)).unwrap();
        // 立っているビット数が pdq 側の数え方と一致する。
        let expected = pdq::Hash256::from_hex(hex).unwrap().hamming_norm();
        assert_eq!(distance(&parsed, &zero), expected);
        assert!(parse("短すぎる").is_none());
    }
}
