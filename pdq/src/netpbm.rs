//! Netpbm (PAM / PPM / PGM) のバイナリ形式を読む。
//!
//! ffmpeg に `-c:v pam` でデコードさせると、寸法とチャンネル数がヘッダに入った
//! 状態で画素が流れてくる。ffprobe を別に呼ばずに済むので、このクレートを使う側は
//! ffmpeg のプロセス 1 つだけでフレームを取得できる。
//!
//! 画像フォーマット全般を扱うつもりはなく、ffmpeg の pam / ppm / pgm エンコーダが
//! 出す範囲 (8 ビット、MAXVAL 255) だけに対応する。

use std::fmt;

use crate::Image;

/// Netpbm として解釈できなかった。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError(message.into()))
}

/// PAM (P7)、PPM (P6)、PGM (P5) のバイナリ形式を読む。
///
/// 入力を所有で受け取り、ヘッダを取り除いて画素データとして再利用する。
/// フル解像度の写真だと画素だけで数十 MB になるので、複製を避けている。
pub fn parse(mut bytes: Vec<u8>) -> Result<Image, ParseError> {
    let header = match bytes.get(..2) {
        Some(b"P7") => parse_pam_header(&bytes)?,
        Some(b"P6") => parse_plain_header(&bytes, 3)?,
        Some(b"P5") => parse_plain_header(&bytes, 1)?,
        _ => return err("Netpbm 形式ではありません"),
    };

    if header.maxval != 255 {
        return err(format!("8 ビット画像ではありません (MAXVAL={})", header.maxval));
    }

    let Some(expected) = header
        .width
        .checked_mul(header.height)
        .and_then(|n| n.checked_mul(header.channels))
    else {
        return err("画像の寸法が大きすぎます");
    };

    let available = bytes.len() - header.len;
    if available < expected {
        return err(format!(
            "画素データが足りません ({available} バイト、{expected} バイト必要)"
        ));
    }

    bytes.drain(..header.len);
    bytes.truncate(expected);

    Ok(Image {
        width: header.width,
        height: header.height,
        channels: header.channels,
        data: bytes,
    })
}

struct Header {
    /// ヘッダのバイト数。画素データはここから始まる。
    len: usize,
    width: usize,
    height: usize,
    channels: usize,
    maxval: usize,
}

/// PAM (P7)。`KEY VALUE` の行が並び、`ENDHDR` の行で終わる。
fn parse_pam_header(bytes: &[u8]) -> Result<Header, ParseError> {
    let (mut width, mut height, mut channels, mut maxval) = (None, None, None, None);

    let mut cursor = 0;
    loop {
        let Some(newline) = bytes[cursor..].iter().position(|&b| b == b'\n') else {
            return err("PAM ヘッダが ENDHDR で終わっていません");
        };
        let line_end = cursor + newline;
        let Ok(line) = std::str::from_utf8(&bytes[cursor..line_end]) else {
            return err("PAM ヘッダが UTF-8 ではありません");
        };
        cursor = line_end + 1;

        let line = line.trim();
        if line == "ENDHDR" {
            break;
        }

        // P7 の行、コメント行、TUPLTYPE などはここで読み飛ばされる。
        let mut fields = line.split_whitespace();
        if let (Some(key), Some(value)) = (fields.next(), fields.next()) {
            let parsed = value.parse().ok();
            match key {
                "WIDTH" => width = parsed,
                "HEIGHT" => height = parsed,
                "DEPTH" => channels = parsed,
                "MAXVAL" => maxval = parsed,
                _ => {}
            }
        }
    }

    let (Some(width), Some(height), Some(channels), Some(maxval)) =
        (width, height, channels, maxval)
    else {
        return err("PAM ヘッダに WIDTH / HEIGHT / DEPTH / MAXVAL が揃っていません");
    };

    Ok(Header { len: cursor, width, height, channels, maxval })
}

/// PPM (P6) と PGM (P5)。マジックのあと空白区切りで width, height, maxval が並び、
/// maxval の直後の空白 1 文字から画素データが始まる。`#` 始まりはコメント。
fn parse_plain_header(bytes: &[u8], channels: usize) -> Result<Header, ParseError> {
    let mut numbers = [0usize; 3];
    let mut cursor = 2;

    for slot in numbers.iter_mut() {
        loop {
            let Some(&b) = bytes.get(cursor) else {
                return err("Netpbm ヘッダが途中で終わっています");
            };
            if b == b'#' {
                while bytes.get(cursor).is_some_and(|&b| b != b'\n') {
                    cursor += 1;
                }
            } else if b.is_ascii_whitespace() {
                cursor += 1;
            } else {
                break;
            }
        }

        let start = cursor;
        while bytes.get(cursor).is_some_and(|b| !b.is_ascii_whitespace()) {
            cursor += 1;
        }
        let parsed = std::str::from_utf8(&bytes[start..cursor]).ok().and_then(|s| s.parse().ok());
        match parsed {
            Some(value) => *slot = value,
            None => return err("Netpbm ヘッダの数値を読めません"),
        }
    }

    if cursor >= bytes.len() {
        return err("Netpbm ヘッダが途中で終わっています");
    }
    cursor += 1; // maxval の直後の空白 1 文字

    let [width, height, maxval] = numbers;
    Ok(Header { len: cursor, width, height, channels, maxval })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ppm() {
        let mut bytes = b"P6\n2 1\n255\n".to_vec();
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        let image = parse(bytes).unwrap();
        assert_eq!((image.width, image.height, image.channels), (2, 1, 3));
        assert_eq!(image.data, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn parses_pgm_with_comment() {
        let mut bytes = b"P5\n# ffmpeg\n3 1\n255\n".to_vec();
        bytes.extend_from_slice(&[7, 8, 9]);
        let image = parse(bytes).unwrap();
        assert_eq!((image.width, image.height, image.channels), (3, 1, 1));
        assert_eq!(image.data, vec![7, 8, 9]);
    }

    #[test]
    fn parses_pam() {
        let mut bytes =
            b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n".to_vec();
        bytes.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let image = parse(bytes).unwrap();
        assert_eq!((image.width, image.height, image.channels), (2, 1, 4));
        assert_eq!(image.data, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    /// 画素データの後ろに何か続いていても、必要なぶんだけ切り出す。
    #[test]
    fn ignores_trailing_bytes() {
        let mut bytes = b"P6\n1 1\n255\n".to_vec();
        bytes.extend_from_slice(&[1, 2, 3, b'j', b'u', b'n', b'k']);
        assert_eq!(parse(bytes).unwrap().data, vec![1, 2, 3]);
    }

    #[test]
    fn rejects_short_data() {
        let mut bytes = b"P6\n2 2\n255\n".to_vec();
        bytes.extend_from_slice(&[1, 2, 3]);
        assert!(parse(bytes).is_err());
    }

    #[test]
    fn rejects_16bit() {
        let bytes = b"P6\n1 1\n65535\n\0\0\0\0\0\0".to_vec();
        assert!(parse(bytes).is_err());
    }

    #[test]
    fn rejects_unknown_magic() {
        assert!(parse(b"P4\n1 1\n".to_vec()).is_err());
        assert!(parse(Vec::new()).is_err());
    }

    #[test]
    fn rejects_truncated_header() {
        assert!(parse(b"P6\n2".to_vec()).is_err());
        assert!(parse(b"P7\nWIDTH 2\n".to_vec()).is_err());
    }
}
