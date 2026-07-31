use image::{DynamicImage, ImageBuffer, Rgba};
use xcap::{Monitor, Window};

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub app_name: String,
}

#[derive(Clone, Debug)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
}

pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    let windows = Window::all().map_err(|e| format!("Kunde inte hämta fönster: {}", e))?;
    let mut list = Vec::new();
    for w in windows {
        let id = match w.id() {
            Ok(id) => id,
            Err(_) => continue,
        };

        let title = w.title().unwrap_or_default();
        let app_name = w.app_name().unwrap_or_else(|_| "Okänd App".to_string());

        // Hoppa bara över om både titel och app-namn saknas helt
        if title.trim().is_empty() && app_name.trim().is_empty() {
            continue;
        }

        list.push(WindowInfo {
            id,
            title: if title.trim().is_empty() {
                "Namnlöst fönster".to_string()
            } else {
                title
            },
            app_name,
        });
    }
    // Sortera efter app-namn
    list.sort_by_key(|window| window.app_name.to_lowercase());
    Ok(list)
}

pub fn list_monitors() -> Result<Vec<MonitorInfo>, String> {
    let monitors = Monitor::all().map_err(|e| format!("Kunde inte hämta skärmar: {}", e))?;
    let mut list = Vec::new();
    for (i, m) in monitors.into_iter().enumerate() {
        let is_primary = m.is_primary().unwrap_or(false);
        let name = format!(
            "Skärm {} ({})",
            i + 1,
            if is_primary {
                "Huvudskärm"
            } else {
                "Sekundär"
            }
        );
        list.push(MonitorInfo { id: i as u32, name });
    }
    Ok(list)
}

pub fn capture_source(
    source_type: &str,
    source_id: u32,
    crop_area: Option<(u32, u32, u32, u32)>,
) -> Result<DynamicImage, String> {
    let raw_img = if source_type == "window" {
        let windows = Window::all().map_err(|e| format!("Fönstersökning misslyckades: {}", e))?;
        let window = windows
            .into_iter()
            .find(|w| w.id().unwrap_or(0) == source_id)
            .ok_or_else(|| format!("Hittade inte fönster med ID {}", source_id))?;

        window
            .capture_image()
            .map_err(|e| format!("Kunde inte ta fönsterklipp: {}", e))?
    } else {
        let monitors = Monitor::all().map_err(|e| format!("Skärmsökning misslyckades: {}", e))?;
        let monitor = monitors
            .into_iter()
            .enumerate()
            .find(|(i, _)| *i as u32 == source_id)
            .map(|(_, m)| m)
            .ok_or_else(|| format!("Hittade inte skärm med ID {}", source_id))?;

        monitor
            .capture_image()
            .map_err(|e| format!("Kunde inte ta skärmklipp: {}", e))?
    };

    // Konvertera RgbaImage till DynamicImage
    let width = raw_img.width();
    let height = raw_img.height();
    let flat_samples = raw_img.into_raw();

    let buffer = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, flat_samples)
        .ok_or_else(|| "Kunde inte skapa bildbuffert".to_string())?;

    let img = DynamicImage::ImageRgba8(buffer);

    // Utför beskärning om det efterfrågas
    if let Some(area) = crop_area {
        Ok(crop_image(&img, area))
    } else {
        Ok(img)
    }
}

/// Beskär en bild utan att koordinater utanför bilden kan orsaka panik.
pub fn crop_image(img: &DynamicImage, (x, y, width, height): (u32, u32, u32, u32)) -> DynamicImage {
    let image_width = img.width();
    let image_height = img.height();

    if image_width == 0 || image_height == 0 || width == 0 || height == 0 {
        return img.clone();
    }

    let x = x.min(image_width - 1);
    let y = y.min(image_height - 1);
    let width = width.min(image_width - x);
    let height = height.min(image_height - y);

    img.crop_imm(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::crop_image;
    use image::{DynamicImage, GenericImageView, RgbaImage};

    #[test]
    fn crop_is_limited_to_image_bounds() {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(10, 8));

        assert_eq!(
            crop_image(&image, (8, 6, u32::MAX, u32::MAX)).dimensions(),
            (2, 2)
        );
    }

    #[test]
    fn empty_crop_keeps_original_image() {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(10, 8));

        assert_eq!(crop_image(&image, (0, 0, 0, 4)).dimensions(), (10, 8));
    }
}
