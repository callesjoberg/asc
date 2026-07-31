use xcap::{Monitor, Window};
use image::{DynamicImage, ImageBuffer, Rgba};
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub app_name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Clone, Debug)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn list_windows() -> Result<Vec<WindowInfo>, String> {
    let windows = Window::all().map_err(|e| format!("Kunde inte hämta fönster: {}", e))?;
    let mut list = Vec::new();
    for w in windows {
        // Ignorera minimerade fönster eller fönster utan titel för att hålla listan ren
        if w.is_minimized().unwrap_or(false) {
            continue;
        }
        let title = match w.title() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if title.trim().is_empty() {
            continue;
        }
        
        let app_name = w.app_name().unwrap_or_else(|_| "Okänd App".to_string());
        let id = match w.id() {
            Ok(id) => id,
            Err(_) => continue,
        };
        
        list.push(WindowInfo {
            id,
            title,
            app_name,
            x: w.x().unwrap_or(0),
            y: w.y().unwrap_or(0),
            width: w.width().unwrap_or(0),
            height: w.height().unwrap_or(0),
        });
    }
    // Sortera efter app-namn
    list.sort_by(|a, b| a.app_name.to_lowercase().cmp(&b.app_name.to_lowercase()));
    Ok(list)
}

pub fn list_monitors() -> Result<Vec<MonitorInfo>, String> {
    let monitors = Monitor::all().map_err(|e| format!("Kunde inte hämta skärmar: {}", e))?;
    let mut list = Vec::new();
    for (i, m) in monitors.into_iter().enumerate() {
        let is_primary = m.is_primary().unwrap_or(false);
        let name = format!("Skärm {} ({})", i + 1, if is_primary { "Huvudskärm" } else { "Sekundär" });
        list.push(MonitorInfo {
            id: i as u32,
            name,
            x: m.x().unwrap_or(0),
            y: m.y().unwrap_or(0),
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
        });
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
            .ok_ok_or_else(|| format!("Hittade inte fönster med ID {}", source_id))?;
        
        window.capture_image().map_err(|e| format!("Kunde inte ta fönsterklipp: {}", e))?
    } else {
        let monitors = Monitor::all().map_err(|e| format!("Skärmsökning misslyckades: {}", e))?;
        let monitor = monitors
            .into_iter()
            .enumerate()
            .find(|(i, _)| *i as u32 == source_id)
            .map(|(_, m)| m)
            .ok_ok_or_else(|| format!("Hittade inte skärm med ID {}", source_id))?;
        
        monitor.capture_image().map_err(|e| format!("Kunde inte ta skärmklipp: {}", e))?
    };

    // Konvertera RgbaImage till DynamicImage
    let width = raw_img.width();
    let height = raw_img.height();
    let flat_samples = raw_img.into_raw();
    
    let buffer = ImageBuffer::<Rgba<u8>, Vec<u8>>::from_raw(width, height, flat_samples)
        .ok_ok_or_else(|| "Kunde inte skapa bildbuffert".to_string())?;
    
    let mut img = DynamicImage::ImageRgba8(buffer);

    // Utför beskärning om det efterfrågas
    if let Some((mut cx, mut cy, mut cw, mut ch)) = crop_area {
        // Säkerställ att vi inte klipper utanför bildens gränser (undvik krascher)
        if cx >= width { cx = 0; }
        if cy >= height { cy = 0; }
        if cx + cw > width { cw = width - cx; }
        if cy + ch > height { ch = height - cy; }
        
        if cw > 0 && ch > 0 {
            img = img.crop(cx, cy, cw, ch);
        }
    }

    Ok(img)
}

// Custom Helper Extension för Option / Result
trait OptionExt<T> {
    fn ok_ok_or_else<F: FnOnce() -> String>(self, f: F) -> Result<T, String>;
}

impl<T> OptionExt<T> for Option<T> {
    fn ok_ok_or_else<F: FnOnce() -> String>(self, f: F) -> Result<T, String> {
        self.ok_or_else(f)
    }
}
