use image::{DynamicImage, GenericImageView};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Difference {
    pub average: f64,
    pub changed_pixels: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IndicatorColor {
    Green,
    Red,
    Gray,
}

impl IndicatorColor {
    pub fn label(self) -> &'static str {
        match self {
            Self::Green => "grön",
            Self::Red => "röd",
            Self::Gray => "grå/okänd",
        }
    }
}

pub struct KeywordTracker {
    keyword_lowercase: String,
    delay_frames: u32,
    rising_edge_only: bool,
    was_present: bool,
    pending: VecDeque<u32>,
}

impl KeywordTracker {
    pub fn new(keyword: &str, delay_frames: u32, rising_edge_only: bool) -> Self {
        Self {
            keyword_lowercase: keyword.to_lowercase(),
            delay_frames,
            rising_edge_only,
            was_present: false,
            pending: VecDeque::new(),
        }
    }

    /// Matar in en OCR-text och returnerar antalet färgavläsningar som ska
    /// kopplas till den aktuella bildrutan.
    pub fn advance(&mut self, ocr_text: &str) -> usize {
        let mut due = 0;
        for remaining in &mut self.pending {
            *remaining = remaining.saturating_sub(1);
        }
        while self
            .pending
            .front()
            .is_some_and(|remaining| *remaining == 0)
        {
            self.pending.pop_front();
            due += 1;
        }

        let present = ocr_text.to_lowercase().contains(&self.keyword_lowercase);
        let triggered = present && (!self.rising_edge_only || !self.was_present);
        self.was_present = present;
        if triggered {
            if self.delay_frames == 0 {
                due += 1;
            } else {
                self.pending.push_back(self.delay_frames);
            }
        }
        due
    }
}

pub fn classify_indicator(
    image: &DynamicImage,
    area: (u32, u32, u32, u32),
    dominance_delta: u8,
    minimum_pixels: u64,
) -> IndicatorColor {
    let cropped = crate::capture::crop_image(image, area).to_rgb8();
    let delta = i16::from(dominance_delta.max(1));
    let mut green_pixels = 0_u64;
    let mut red_pixels = 0_u64;
    for pixel in cropped.pixels() {
        let red = i16::from(pixel[0]);
        let green = i16::from(pixel[1]);
        let blue = i16::from(pixel[2]);
        if green - red >= delta && green - blue >= delta {
            green_pixels += 1;
        } else if red - green >= delta && red - blue >= delta {
            red_pixels += 1;
        }
    }

    let minimum_pixels = minimum_pixels.max(1);
    if green_pixels >= minimum_pixels && green_pixels > red_pixels {
        IndicatorColor::Green
    } else if red_pixels >= minimum_pixels && red_pixels > green_pixels {
        IndicatorColor::Red
    } else {
        IndicatorColor::Gray
    }
}

/// Jämför två bilder och returnerar en skillnadsgrad mellan 0.0 (identiska) och 1.0 (helt olika).
/// Jämförelsen sker kanal för kanal (RGB) på det överlappande området.
/// Pixlar utanför det gemensamma området räknas som helt olika.
#[cfg(test)]
pub fn compare_images(img1: &DynamicImage, img2: &DynamicImage) -> f64 {
    analyze_images(img1, img2, 1).average
}

/// Beräknar både genomsnittlig färgskillnad och antalet pixlar vars största
/// RGB-kanalskillnad når `color_delta`. Pixelantalet gör små lokala indikatorer
/// mätbara även när de utgör en mycket liten del av en stor skärmbild.
pub fn analyze_images(img1: &DynamicImage, img2: &DynamicImage, color_delta: u8) -> Difference {
    let (w1, h1) = img1.dimensions();
    let (w2, h2) = img2.dimensions();

    if w1 == 0 || h1 == 0 || w2 == 0 || h2 == 0 {
        return Difference {
            average: 1.0,
            changed_pixels: u64::from(w1.max(w2)) * u64::from(h1.max(h2)),
        };
    }

    let common_width = std::cmp::min(w1, w2);
    let common_height = std::cmp::min(h1, h2);

    let mut diff_sum: u64 = 0;
    let mut pixels_count: u64 = 0;
    let mut changed_pixels: u64 = 0;
    let color_delta = u32::from(color_delta.max(1));

    for y in 0..common_height {
        for x in 0..common_width {
            let p1 = img1.get_pixel(x, y);
            let p2 = img2.get_pixel(x, y);

            let r_diff = (p1[0] as i32 - p2[0] as i32).unsigned_abs();
            let g_diff = (p1[1] as i32 - p2[1] as i32).unsigned_abs();
            let b_diff = (p1[2] as i32 - p2[2] as i32).unsigned_abs();

            diff_sum += (r_diff + g_diff + b_diff) as u64;
            if r_diff.max(g_diff).max(b_diff) >= color_delta {
                changed_pixels += 1;
            }
            pixels_count += 1;
        }
    }

    // Ta hänsyn till storleksskillnad
    let total_pixels = std::cmp::max(w1 * h1, w2 * h2) as u64;
    let size_diff_pixels = total_pixels - pixels_count;
    changed_pixels += size_diff_pixels;

    // För pixlar som saknas i det överlappande området räknar vi maximal RGB-skillnad.
    diff_sum += size_diff_pixels * 765;

    let max_possible_diff = total_pixels * 765;
    if max_possible_diff == 0 {
        return Difference {
            average: 0.0,
            changed_pixels,
        };
    }

    Difference {
        average: (diff_sum as f64) / (max_possible_diff as f64),
        changed_pixels,
    }
}

pub fn analyze_regions(
    img1: &DynamicImage,
    img2: &DynamicImage,
    regions: &[(u32, u32, u32, u32)],
    color_delta: u8,
) -> Difference {
    if regions.is_empty() {
        return analyze_images(img1, img2, color_delta);
    }

    let mut weighted_average = 0.0;
    let mut total_pixels = 0_u64;
    let mut changed_pixels = 0_u64;
    for &region in regions {
        let first = crate::capture::crop_image(img1, region);
        let second = crate::capture::crop_image(img2, region);
        let difference = analyze_images(&first, &second, color_delta);
        let pixels = u64::from(first.width().max(second.width()))
            * u64::from(first.height().max(second.height()));
        weighted_average += difference.average * pixels as f64;
        total_pixels += pixels;
        changed_pixels += difference.changed_pixels;
    }

    Difference {
        average: if total_pixels == 0 {
            0.0
        } else {
            weighted_average / total_pixels as f64
        },
        changed_pixels,
    }
}

pub fn change_detected(
    has_previous_image: bool,
    pixel_diff: f64,
    threshold: f64,
    local_change_detected: bool,
    ocr_changed: bool,
) -> bool {
    has_previous_image && (pixel_diff >= threshold || local_change_detected || ocr_changed)
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
        assert!(!change_detected(false, 1.0, 0.01, true, true));
        assert!(change_detected(true, 0.01, 0.01, false, false));
        assert!(change_detected(true, 0.0, 0.01, true, false));
        assert!(change_detected(true, 0.0, 0.01, false, true));
        assert!(!change_detected(true, 0.009, 0.01, false, false));
    }

    #[test]
    fn test_different_sizes() {
        let img1 = create_mock_image(5, 5, Rgba([0, 0, 0, 255]));
        let img2 = create_mock_image(10, 10, Rgba([0, 0, 0, 255]));
        let diff = compare_images(&img1, &img2);
        assert_eq!(diff, 0.75);
    }

    #[test]
    fn selected_measurement_region_ignores_changes_outside_it() {
        let img1 = create_mock_image(20, 20, Rgba([0, 0, 0, 255]));
        let mut buffer = ImageBuffer::from_pixel(20, 20, Rgba([0, 0, 0, 255]));
        buffer.put_pixel(2, 2, Rgba([255, 0, 0, 255]));
        buffer.put_pixel(12, 12, Rgba([0, 255, 0, 255]));
        let img2 = DynamicImage::ImageRgba8(buffer);

        let outside_only = analyze_regions(&img1, &img2, &[(10, 10, 5, 5)], 24);
        assert_eq!(outside_only.changed_pixels, 1);

        let unchanged_region = analyze_regions(&img1, &img2, &[(5, 5, 5, 5)], 24);
        assert_eq!(unchanged_region.changed_pixels, 0);
    }

    #[test]
    fn five_pixel_wide_color_indicator_is_counted() {
        fn indicator(color: Rgba<u8>) -> DynamicImage {
            let mut image = create_mock_image(1920, 1080, Rgba([20, 20, 20, 255])).to_rgba8();
            let center_x = 960_i32;
            let center_y = 540_i32;
            for y in (center_y - 2)..=(center_y + 2) {
                for x in (center_x - 2)..=(center_x + 2) {
                    if (x - center_x).pow(2) + (y - center_y).pow(2) <= 4 {
                        image.put_pixel(x as u32, y as u32, color);
                    }
                }
            }
            DynamicImage::ImageRgba8(image)
        }

        let red = indicator(Rgba([220, 0, 0, 255]));
        let green = indicator(Rgba([0, 220, 0, 255]));
        let gray = indicator(Rgba([128, 128, 128, 255]));
        for difference in [
            analyze_images(&red, &green, 24),
            analyze_images(&green, &gray, 24),
            analyze_images(&gray, &red, 24),
        ] {
            assert_eq!(difference.changed_pixels, 13);
            assert!(difference.average * 100.0 < 0.001);
        }
    }

    #[test]
    fn indicator_area_classifies_green_red_and_gray() {
        for (color, expected) in [
            (Rgba([0, 220, 0, 255]), IndicatorColor::Green),
            (Rgba([220, 0, 0, 255]), IndicatorColor::Red),
            (Rgba([128, 128, 128, 255]), IndicatorColor::Gray),
        ] {
            let image = create_mock_image(8, 8, color);
            assert_eq!(classify_indicator(&image, (1, 1, 5, 5), 24, 5), expected);
        }
    }

    #[test]
    fn keyword_tracker_supports_rising_edge_and_frame_delay() {
        let mut tracker = KeywordTracker::new("öppet", 2, true);
        assert_eq!(tracker.advance("Status: öppet"), 0);
        assert_eq!(tracker.advance("Status: öppet"), 0);
        assert_eq!(tracker.advance("Status: öppet"), 1);
        assert_eq!(tracker.advance("Status: stängt"), 0);
        assert_eq!(tracker.advance("ÖPPET igen"), 0);
        assert_eq!(tracker.advance("väntar"), 0);
        assert_eq!(tracker.advance("väntar"), 1);
    }
}
