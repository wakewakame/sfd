//! ffmpeg / ffprobe を子プロセスとして呼び、フレームと尺を得る。
//!
//! フレームは PAM (P7) 形式で受け取る。ヘッダに寸法とチャンネル数が入っているので、
//! 画像については ffprobe を呼ばずに済む。ffprobe を使うのは動画の尺を得るときだけ。

use std::path::Path;
use std::process::Command;

use pdq::Image;

/// ffmpeg / ffprobe の実行ファイル。
#[derive(Debug, Clone)]
pub struct Tools {
    pub ffmpeg: String,
    pub ffprobe: String,
}

impl Tools {
    /// 動画の尺 (秒) を得る。映像ストリームが無ければ `Ok(None)`。
    ///
    /// これがないと 1/4, 2/4, 3/4 地点を計算できない。ffmpeg 自身も stderr に
    /// `Duration:` を出しているが、あれは人間向けの表示で形式の保証がないので、
    /// 機械向けのインターフェースを持つ ffprobe を使う。
    ///
    /// 映像の有無を尺とは別に確かめているのは、`format` の尺がコンテナ全体の
    /// ものだから。音声しか入っていない .webm でも尺は返ってくるので、それだけを
    /// 見ていると 1/4 地点を取りに行って
    /// 「Output file does not contain any stream」で落ちる。
    pub fn probe_duration(&self, path: &Path) -> Result<Option<f64>, String> {
        let output = Command::new(&self.ffprobe)
            .args(["-v", "error", "-select_streams", "v:0"])
            .args(["-show_entries", "format=duration:stream=duration", "-of", "json"])
            .arg(path)
            .output()
            .map_err(|e| format!("cannot run ffprobe ({}): {e}", self.ffprobe))?;

        if !output.status.success() {
            return Err(format!("ffprobe failed: {}", error_message(&output.stderr)));
        }

        let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("cannot parse the ffprobe output: {e}"))?;

        // -select_streams v:0 を付けてあるので、映像が無ければ streams は空になる。
        let Some(video) = parsed.get("streams").and_then(|streams| streams.get(0)) else {
            return Ok(None);
        };

        // 映像ストリーム自身の尺を優先し、無ければコンテナ全体の尺を使う。
        let duration = video
            .get("duration")
            .or_else(|| parsed.get("format").and_then(|format| format.get("duration")))
            .and_then(|value| value.as_str())
            .and_then(|text| text.parse::<f64>().ok());

        match duration {
            Some(seconds) if seconds.is_finite() && seconds > 0.0 => Ok(Some(seconds)),
            _ => Err("cannot determine the duration".to_string()),
        }
    }

    /// 画像を復号する。
    pub fn decode_image(&self, path: &Path) -> Result<Image, String> {
        self.decode(path, None)
    }

    /// 動画の `at` 秒地点のフレームを復号する。
    pub fn decode_frame_at(&self, path: &Path, at: f64) -> Result<Image, String> {
        self.decode(path, Some(at))
    }

    fn decode(&self, path: &Path, seek: Option<f64>) -> Result<Image, String> {
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-v", "error", "-nostdin"]);

        if let Some(at) = seek {
            // -ss は -i より前に置く。ここが速さと精度を両立する唯一の形で、
            // 後ろに置くと先頭から復号するため長い動画で極端に遅くなる。
            //
            // ffmpeg 2.1 (2013-10) より前は前置きだとキーフレームにスナップした
            // ため、正確に取るには後ろに置く必要があった。当時の情報が今も多く
            // 残っているが、現在は「キーフレームまで飛んでから目的の時刻まで
            // 復号して捨てる」ので両立する。古い記事を見て後ろに動かさないこと。
            //
            // -noaccurate_seek は付けない。付けるとキーフレーム基準に戻り、
            // 取れるフレームがキーフレーム配置に依存してしまう。同じ動画の
            // 再エンコード版から別のフレームが取れると知覚ハッシュが一致しなくなる。
            command.arg("-ss").arg(format!("{at:.6}"));
        }

        // ストリームを明示しないのは意図的。iPhone の HEIC は画像がタイルに分割されて
        // 数百のストリームとして見えるため、-map 0:v:0 を付けると先頭のタイルだけを
        // 掴んでしまう。またそのグリッド再構成は内部で complex filtergraph を使うため、
        // -vf を併用すると "Simple and complex filtering cannot be used together" で失敗する。
        //
        // 画素形式は rgb24 に固定する。自動交渉に任せると、1 ビットの白黒 PNG では
        // pam エンコーダが monob を選び、MAXVAL 1 のビット詰めされた PAM が出てくる。
        // 他にも gray16be や rgba64be を選ぶ余地があり、そのたびに読めない形式が
        // 増える。-pix_fmt は複合フィルタとは別の経路なので HEIC とも併用できる。
        //
        // 代償として、グレースケール画像も RGB 経由になり輝度係数の丸めが挟まる。
        // リファレンス実装の専用経路とは数ビットずれるが、読めない形式が出るより良い。
        command
            .arg("-i")
            .arg(path)
            .args(["-frames:v", "1", "-pix_fmt", "rgb24"])
            .args(["-f", "image2pipe", "-c:v", "pam", "-"]);

        let output = command
            .output()
            .map_err(|e| format!("cannot run ffmpeg ({}): {e}", self.ffmpeg))?;

        if !output.status.success() {
            return Err(format!("ffmpeg failed: {}", error_message(&output.stderr)));
        }
        if output.stdout.is_empty() {
            return Err("ffmpeg produced no frame".to_string());
        }

        let mut image = pdq::netpbm::parse(output.stdout).map_err(|e| e.to_string())?;
        // ハッシュが受け付けるのは 1 チャンネルか 3 チャンネルだけ。
        image.drop_alpha();
        Ok(image)
    }
}

/// ffmpeg / ffprobe の stderr から、記録に使う 1 行を取り出す。
///
/// 先頭の行を取るのは、そこに根本原因が出るため。後ろの行は「結局何も出力
/// されなかった」といった結果の報告になりがちで、原因が分からない。
///
/// `[mjpeg @ 0x7f...]` のような接頭辞は実行のたびに変わるポインタを含むので
/// 落とす。同じ原因なら同じ文字列になっていた方が、hash.json を突き合わせやすい。
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

/// 尺を 4 分割した 1/4, 2/4, 3/4 地点。
///
/// 端を避けているのは、動画の先頭と末尾が黒画面やロゴであることが多く、
/// 内容を代表しないため。
pub fn sample_times(duration: f64) -> [f64; 3] {
    [duration * 0.25, duration * 0.5, duration * 0.75]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_quarter_points() {
        assert_eq!(sample_times(100.0), [25.0, 50.0, 75.0]);
        assert_eq!(sample_times(4.0), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn picks_the_first_meaningful_stderr_line() {
        assert_eq!(error_message(b"a\nb\n"), "a");
        assert_eq!(error_message(b"\n\nb\n"), "b");
        assert_eq!(error_message(b""), "no details");
        assert_eq!(error_message(b"  only line  \n"), "only line");
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
