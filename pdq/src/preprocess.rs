//! リファレンス CLI (`pdq-photo-hasher`) がハッシュ前に行う縮小の再現。
//!
//! これは PDQ アルゴリズムの一部ではなく CLI の方針だが、ThreatExchange が
//! 公開している期待値はすべてこの前処理込みで生成されている。知らないまま
//! フル解像度でハッシュすると、正しい実装でも値が 2〜8 ビットずれる。

use crate::Image;

/// リファレンス CLI が入力に施す前処理の閾値。
///
/// `pdq/cpp/io/pdqio.cpp` にこうある:
///
/// ```text
/// // The two-pass Jarosz filter is prohibitively expensive for larger images
/// // so we use off-the-shelf downsampling to get to an intermediate size.
/// const int DOWNSAMPLE_DIMS = 512;
/// ```
///
/// 縦横どちらかがこの値を超える画像は、アスペクト比を無視して 512x512 の
/// 最近傍縮小をかけてからハッシュされる。
pub const REFERENCE_DOWNSAMPLE_DIMS: usize = 512;

/// リファレンス CLI と同じ前処理を適用する。縮小が不要なら何もしない。
///
/// 副作用として非常に速くなる。Jarosz フィルタの計算量は入力画素数に比例するので、
/// 2400 万画素の写真なら 100 倍以上変わる。
pub fn reference_downsample(image: &mut Image) {
    if image.height <= REFERENCE_DOWNSAMPLE_DIMS && image.width <= REFERENCE_DOWNSAMPLE_DIMS {
        return;
    }
    let dims = REFERENCE_DOWNSAMPLE_DIMS;
    image.data = resize_nearest(image, dims, dims);
    image.height = dims;
    image.width = dims;
}

/// CImg の `resize(..., interpolation_type = 1)` と同じ最近傍縮小。
///
/// 中心合わせではなく角合わせで、出力 `x` は入力 `floor(x * width / dst_width)` を
/// 参照する。ffmpeg の `scale=...:flags=neighbor` は中心合わせなので、
/// そちらでは同じ画素にならない。
pub fn resize_nearest(image: &Image, dst_height: usize, dst_width: usize) -> Vec<u8> {
    let Image { width, height, channels, data } = image;
    assert_eq!(data.len(), height * width * channels, "画素データの長さが寸法と一致しません");
    assert!(dst_height > 0 && dst_width > 0);

    let col_map: Vec<usize> = (0..dst_width)
        .map(|x| (x as f64 * *width as f64 / dst_width as f64) as usize)
        .collect();

    let mut out = vec![0u8; dst_height * dst_width * channels];
    for y in 0..dst_height {
        let src_y = (y as f64 * *height as f64 / dst_height as f64) as usize;
        let src_row = &data[src_y * width * channels..(src_y + 1) * width * channels];
        let dst_row = &mut out[y * dst_width * channels..(y + 1) * dst_width * channels];
        for (x, &src_x) in col_map.iter().enumerate() {
            dst_row[x * channels..(x + 1) * channels]
                .copy_from_slice(&src_row[src_x * channels..(src_x + 1) * channels]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_nearest_aligns_to_corners() {
        // 4 画素 -> 2 画素。角合わせなので floor(0 * 4 / 2) = 0, floor(1 * 4 / 2) = 2。
        let image = Image { width: 4, height: 1, channels: 1, data: vec![10, 20, 30, 40] };
        assert_eq!(resize_nearest(&image, 1, 2), vec![10, 30]);
    }

    #[test]
    fn resize_nearest_handles_multiple_channels() {
        let image =
            Image { width: 2, height: 2, channels: 3, data: (0..12).collect() };
        // 2x2 -> 1x1 は左上の画素だけを取る。
        assert_eq!(resize_nearest(&image, 1, 1), vec![0, 1, 2]);
    }

    #[test]
    fn small_images_are_left_alone() {
        let original = Image { width: 512, height: 512, channels: 1, data: vec![7; 512 * 512] };
        let mut image = original.clone();
        reference_downsample(&mut image);
        assert_eq!(image, original);
    }

    #[test]
    fn large_images_are_downsampled_to_512_square() {
        let mut image = Image { width: 1024, height: 600, channels: 3, data: vec![7; 1024 * 600 * 3] };
        reference_downsample(&mut image);
        assert_eq!((image.width, image.height), (512, 512));
        assert_eq!(image.data.len(), 512 * 512 * 3);
    }

    /// 片方だけが閾値を超えていても、正方形に潰される。
    #[test]
    fn one_oversized_dimension_is_enough() {
        let mut image = Image { width: 600, height: 100, channels: 1, data: vec![7; 600 * 100] };
        reference_downsample(&mut image);
        assert_eq!((image.width, image.height), (512, 512));
    }
}
