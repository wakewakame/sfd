/// デコード済みの 1 フレーム。
///
/// `channels` は 1 (グレースケール)、2 (グレースケール + アルファ)、
/// 3 (RGB)、4 (RGBA) を取りうる。ハッシュ計算が受け付けるのは 1 と 3 だけなので、
/// アルファ付きは [`Image::drop_alpha`] で落としてから渡す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    /// `height * width * channels` バイトの画素データ (row-major)。
    pub data: Vec<u8>,
}

impl Image {
    /// アルファチャンネルがあれば落とす (RGBA -> RGB、グレースケール+アルファ -> グレースケール)。
    ///
    /// 合成先の色を勝手に決めるより、格納されている色をそのまま使う方が予測しやすい。
    /// 透過部分の色は元ファイル次第なので、透過画像同士の比較は当てにしないこと。
    pub fn drop_alpha(&mut self) {
        let opaque = match self.channels {
            2 => 1,
            4 => 3,
            _ => return,
        };

        // 前から詰め直すので、同じバッファ上で安全に処理できる。
        for i in 0..self.width * self.height {
            self.data.copy_within(i * self.channels..i * self.channels + opaque, i * opaque);
        }
        self.data.truncate(self.width * self.height * opaque);
        self.channels = opaque;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_alpha_from_rgba() {
        let mut image = Image {
            width: 2,
            height: 2,
            channels: 4,
            data: vec![
                1, 2, 3, 99, //
                4, 5, 6, 99, //
                7, 8, 9, 99, //
                10, 11, 12, 99,
            ],
        };
        image.drop_alpha();
        assert_eq!(image.channels, 3);
        assert_eq!(image.data, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    }

    #[test]
    fn drop_alpha_from_gray_alpha() {
        let mut image =
            Image { width: 3, height: 1, channels: 2, data: vec![1, 99, 2, 99, 3, 99] };
        image.drop_alpha();
        assert_eq!(image.channels, 1);
        assert_eq!(image.data, vec![1, 2, 3]);
    }

    #[test]
    fn drop_alpha_is_a_no_op_without_alpha() {
        let original = Image { width: 2, height: 1, channels: 3, data: vec![1, 2, 3, 4, 5, 6] };
        let mut image = original.clone();
        image.drop_alpha();
        assert_eq!(image, original);
    }
}
