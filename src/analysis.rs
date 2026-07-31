use image::{DynamicImage, GenericImageView};

/// Jämför två bilder och returnerar en skillnadsgrad mellan 0.0 (identiska) och 1.0 (helt olika).
/// Jämförelsen sker kanal för kanal (RGB) på det överlappande området.
/// Pixlar utanför det gemensamma området räknas som helt olika.
pub fn compare_images(img1: &DynamicImage, img2: &DynamicImage) -> f64 {
    let (w1, h1) = img1.dimensions();
    let (w2, h2) = img2.dimensions();

    if w1 == 0 || h1 == 0 || w2 == 0 || h2 == 0 {
        return 1.0;
    }

    let common_width = std::cmp::min(w1, w2);
    let common_height = std::cmp::min(h1, h2);

    let mut diff_sum: u64 = 0;
    let mut pixels_count: u64 = 0;

    for y in 0..common_height {
        for x in 0..common_width {
            let p1 = img1.get_pixel(x, y);
            let p2 = img2.get_pixel(x, y);

            let r_diff = (p1[0] as i32 - p2[0] as i32).unsigned_abs();
            let g_diff = (p1[1] as i32 - p2[1] as i32).unsigned_abs();
            let b_diff = (p1[2] as i32 - p2[2] as i32).unsigned_abs();

            diff_sum += (r_diff + g_diff + b_diff) as u64;
            pixels_count += 1;
        }
    }

    // Ta hänsyn till storleksskillnad
    let total_pixels = std::cmp::max(w1 * h1, w2 * h2) as u64;
    let size_diff_pixels = total_pixels - pixels_count;

    // För pixlar som saknas i det överlappande området räknar vi maximal RGB-skillnad.
    diff_sum += size_diff_pixels * 765;

    let max_possible_diff = total_pixels * 765;
    if max_possible_diff == 0 {
        return 0.0;
    }

    (diff_sum as f64) / (max_possible_diff as f64)
}

pub fn change_detected(
    has_previous_image: bool,
    pixel_diff: f64,
    threshold: f64,
    ocr_changed: bool,
) -> bool {
    has_previous_image && (pixel_diff >= threshold || ocr_changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn create_mock_image(width: u32, height: u32, color: Rgba<u8>) -> DynamicImage {
        let buffer = ImageBuffer::from_pixel(width, height, color);
        DynamicImage::ImageRgba8(buffer)
    }

    #[test]
    fn test_identical_images() {
        let img1 = create_mock_image(10, 10, Rgba([255, 0, 0, 255]));
        let img2 = create_mock_image(10, 10, Rgba([255, 0, 0, 255]));
        let diff = compare_images(&img1, &img2);
        assert_eq!(diff, 0.0);
    }

    #[test]
    fn test_completely_different_images() {
        let img1 = create_mock_image(10, 10, Rgba([0, 0, 0, 0]));
        let img2 = create_mock_image(10, 10, Rgba([255, 255, 255, 255]));
        let diff = compare_images(&img1, &img2);
        assert_eq!(diff, 1.0);
    }

    #[test]
    fn test_alpha_only_difference_is_ignored() {
        let img1 = create_mock_image(10, 10, Rgba([20, 40, 60, 0]));
        let img2 = create_mock_image(10, 10, Rgba([20, 40, 60, 255]));

        assert_eq!(compare_images(&img1, &img2), 0.0);
    }

    #[test]
    fn test_partial_difference() {
        let img1 = create_mock_image(10, 10, Rgba([0, 0, 0, 255]));
        let mut buffer = ImageBuffer::from_pixel(10, 10, Rgba([0, 0, 0, 255]));
        for x in 0..5 {
            for y in 0..10 {
                buffer.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let img2 = DynamicImage::ImageRgba8(buffer);
        let diff = compare_images(&img1, &img2);
        assert_eq!(diff, 0.5);
    }

    #[test]
    fn test_one_percent_visible_change() {
        let img1 = create_mock_image(100, 100, Rgba([0, 0, 0, 255]));
        let mut buffer = ImageBuffer::from_pixel(100, 100, Rgba([0, 0, 0, 255]));
        for x in 0..10 {
            for y in 0..10 {
                buffer.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        let img2 = DynamicImage::ImageRgba8(buffer);

        assert!((compare_images(&img1, &img2) - 0.01).abs() < f64::EPSILON);
    }

    #[test]
    fn test_change_detection_uses_threshold_or_ocr() {
        assert!(!change_detected(false, 1.0, 0.01, true));
        assert!(change_detected(true, 0.01, 0.01, false));
        assert!(change_detected(true, 0.0, 0.01, true));
        assert!(!change_detected(true, 0.009, 0.01, false));
    }

    #[test]
    fn test_different_sizes() {
        let img1 = create_mock_image(5, 5, Rgba([0, 0, 0, 255]));
        let img2 = create_mock_image(10, 10, Rgba([0, 0, 0, 255]));
        let diff = compare_images(&img1, &img2);
        assert_eq!(diff, 0.75);
    }
}
