mod capture;
mod analysis;
mod ocr;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use chrono::Local;
use tauri::{AppHandle, Emitter, State};
use image::DynamicImage;
use std::path::Path;
use base64::{Engine as _, engine::general_purpose};
use std::io::Cursor;

struct CaptureState {
    is_running: Arc<AtomicBool>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct CaptureSettings {
    pub source_type: String, // "window" or "screen"
    pub source_id: u32,
    pub interval_secs: u64,
    pub save_dir: String,
    pub crop_area: Option<(u32, u32, u32, u32)>, // x, y, width, height
    pub enable_ocr: bool,
    pub ocr_area: Option<(u32, u32, u32, u32)>, // x, y, width, height (relative to cropped image)
    pub diff_threshold: f64, // e.g. 0.01 for 1% change
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct CaptureResult {
    pub timestamp: String,
    pub file_name: String,
    pub pixel_diff: f64,
    pub ocr_text: String,
    pub is_changed: bool,
    pub image_base64: String,
}

#[tauri::command]
fn list_windows() -> Result<Vec<capture::WindowInfo>, String> {
    capture::list_windows()
}

#[tauri::command]
fn list_monitors() -> Result<Vec<capture::MonitorInfo>, String> {
    capture::list_monitors()
}

#[tauri::command]
fn select_folder() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Välj målmapp för skärmklipp")
        .pick_folder()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn stop_capture(state: State<'_, CaptureState>) -> Result<(), String> {
    state.is_running.store(false, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn get_capture_status(state: State<'_, CaptureState>) -> bool {
    state.is_running.load(Ordering::SeqCst)
}

#[tauri::command]
async fn start_capture(
    app_handle: AppHandle,
    state: State<'_, CaptureState>,
    settings: CaptureSettings,
) -> Result<String, String> {
    if state.is_running.load(Ordering::SeqCst) {
        return Err("En skärmklippsloop körs redan.".to_string());
    }

    state.is_running.store(true, Ordering::SeqCst);
    let is_running = state.is_running.clone();

    // Kör loopen i en bakgrundstråd så att vi inte blockerar Tauri-huvudtråden
    tokio::spawn(async move {
        let mut prev_img: Option<DynamicImage> = None;
        let mut prev_ocr: Option<String> = None;

        println!("Startar skärmklippsloop med intervall {}s", settings.interval_secs);

        while is_running.load(Ordering::SeqCst) {
            let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let file_timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
            
            // Ta skärmklippet
            match capture::capture_source(&settings.source_type, settings.source_id, settings.crop_area) {
                Ok(img) => {
                    let file_name = format!("screenshot_{}.png", file_timestamp);
                    let file_path = Path::new(&settings.save_dir).join(&file_name);

                    // Spara bilden till disk
                    if let Err(e) = img.save(&file_path) {
                        eprintln!("Kunde inte spara bild: {}", e);
                    }

                    // Jämför pixeländring
                    let mut pixel_diff = 0.0;
                    if let Some(ref p_img) = prev_img {
                        pixel_diff = analysis::compare_images(p_img, &img);
                    }

                    // Utför OCR om aktiverat
                    let mut ocr_text = String::new();
                    if settings.enable_ocr {
                        // Om vi har ett specifikt ocr_area beskära vi bilden ytterligare för OCR
                        let ocr_img = if let Some((ox, oy, ow, oh)) = settings.ocr_area {
                            let w = img.width();
                            let h = img.height();
                            let mut cx = ox;
                            let mut cy = oy;
                            let mut cw = ow;
                            let mut ch = oh;
                            if cx >= w { cx = 0; }
                            if cy >= h { cy = 0; }
                            if cx + cw > w { cw = w - cx; }
                            if cy + ch > h { ch = h - cy; }
                            if cw > 0 && ch > 0 {
                                img.crop_imm(cx, cy, cw, ch)
                            } else {
                                img.clone()
                            }
                        } else {
                            img.clone()
                        };

                        // Spara tillfällig fil för OCR
                        let temp_ocr_name = format!(".ocr_temp_{}.png", file_timestamp);
                        let temp_ocr_path = Path::new(&settings.save_dir).join(&temp_ocr_name);
                        
                        if ocr_img.save(&temp_ocr_path).is_ok() {
                            match ocr::run_ocr(&app_handle, &temp_ocr_path.to_string_lossy()) {
                                Ok(text) => {
                                    ocr_text = text;
                                }
                                Err(e) => {
                                    eprintln!("OCR-fel: {}", e);
                                    ocr_text = format!("Fel: {}", e);
                                }
                            }
                            // Ta bort den tillfälliga filen
                            let _ = std::fs::remove_file(temp_ocr_path);
                        }
                    }

                    // Avgör om förändring har skett
                    let mut is_changed = false;
                    
                    if prev_img.is_some() {
                        if settings.enable_ocr {
                            // Om OCR är på, reagerar vi på om texten ändrats
                            if let Some(ref p_ocr) = prev_ocr {
                                is_changed = ocr_text.trim() != p_ocr.trim();
                            }
                        } else {
                            // Annars går vi på pixel-tröskelvärdet
                            is_changed = pixel_diff >= settings.diff_threshold;
                        }
                    }

                    let image_base64 = img_to_base64(&img);

                    // Skicka resultat till frontend
                    let result = CaptureResult {
                        timestamp: timestamp.clone(),
                        file_name,
                        pixel_diff,
                        ocr_text: ocr_text.clone(),
                        is_changed,
                        image_base64,
                    };

                    if let Err(e) = app_handle.emit("capture-result", result) {
                        eprintln!("Kunde inte skicka event till frontend: {}", e);
                    }

                    // Spara nuvarande bild/OCR för nästa jämförelse
                    prev_img = Some(img);
                    if settings.enable_ocr {
                        prev_ocr = Some(ocr_text);
                    }
                }
                Err(e) => {
                    eprintln!("Klippfel: {}", e);
                    let _ = app_handle.emit("capture-error", format!("Klippfel: {}", e));
                }
            }

            // Vänta till nästa intervall
            tokio::time::sleep(Duration::from_secs(settings.interval_secs)).await;
        }

        println!("Skärmklippsloop avslutad.");
    });

    Ok("Loopen startades framgångsrikt.".to_string())
}

#[derive(serde::Serialize, Debug)]
pub struct OfflineResult {
    pub file_name: String,
    pub pixel_diff: f64,
    pub ocr_text: String,
    pub is_changed: bool,
    pub image_base64: String,
}

#[tauri::command]
async fn run_offline_analysis(
    app_handle: AppHandle,
    dir_path: String,
    crop_area: Option<(u32, u32, u32, u32)>,
    enable_ocr: bool,
    ocr_area: Option<(u32, u32, u32, u32)>,
    diff_threshold: f64,
) -> Result<Vec<OfflineResult>, String> {
    let path = Path::new(&dir_path);
    if !path.exists() || !path.is_dir() {
        return Err("Den angivna sökvägen finns inte eller är inte en mapp.".to_string());
    }

    // Läs alla filer i mappen
    let entries = std::fs::read_dir(path).map_err(|e| format!("Kunde inte läsa mappen: {}", e))?;
    let mut files = Vec::new();

    for entry in entries {
        if let Ok(entry) = entry {
            let p = entry.path();
            if p.is_file() {
                if let Some(ext) = p.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if ext_str == "png" || ext_str == "jpg" || ext_str == "jpeg" {
                        files.push(p);
                    }
                }
            }
        }
    }

    // Sortera filer alfabetiskt/kronologiskt
    files.sort();

    let mut results = Vec::new();
    let mut prev_img: Option<DynamicImage> = None;
    let mut prev_ocr: Option<String> = None;

    for f_path in files {
        let file_name = f_path.file_name().unwrap().to_string_lossy().to_string();
        
        // Hoppa över dolda/tillfälliga filer
        if file_name.starts_with('.') {
            continue;
        }

        let img_result = image::open(&f_path);
        if let Ok(mut img) = img_result {
            // Beskär enligt inställt yta
            if let Some((cx, cy, cw, ch)) = crop_area {
                let w = img.width();
                let h = img.height();
                let mut x = cx;
                let mut y = cy;
                let mut w_crop = cw;
                let mut h_crop = ch;
                if x >= w { x = 0; }
                if y >= h { y = 0; }
                if x + w_crop > w { w_crop = w - x; }
                if y + h_crop > h { h_crop = h - y; }
                if w_crop > 0 && h_crop > 0 {
                    img = img.crop(x, y, w_crop, h_crop);
                }
            }

            // Jämför pixel-differens
            let mut pixel_diff = 0.0;
            if let Some(ref p_img) = prev_img {
                pixel_diff = analysis::compare_images(p_img, &img);
            }

            // OCR
            let mut ocr_text = String::new();
            if enable_ocr {
                let ocr_img = if let Some((ox, oy, ow, oh)) = ocr_area {
                    let w = img.width();
                    let h = img.height();
                    let mut cx = ox;
                    let mut cy = oy;
                    let mut cw = ow;
                    let mut ch = oh;
                    if cx >= w { cx = 0; }
                    if cy >= h { cy = 0; }
                    if cx + cw > w { cw = w - cx; }
                    if cy + ch > h { ch = h - cy; }
                    if cw > 0 && ch > 0 {
                        img.crop_imm(cx, cy, cw, ch)
                    } else {
                        img.clone()
                    }
                } else {
                    img.clone()
                };

                let temp_ocr_name = format!(".ocr_offline_{}.png", file_name);
                let temp_ocr_path = path.join(&temp_ocr_name);
                
                if ocr_img.save(&temp_ocr_path).is_ok() {
                    if let Ok(text) = ocr::run_ocr(&app_handle, &temp_ocr_path.to_string_lossy()) {
                        ocr_text = text;
                    }
                    let _ = std::fs::remove_file(temp_ocr_path);
                }
            }

            // Kolla om ändrad
            let mut is_changed = false;
            if prev_img.is_some() {
                if enable_ocr {
                    if let Some(ref p_ocr) = prev_ocr {
                        is_changed = ocr_text.trim() != p_ocr.trim();
                    }
                } else {
                    is_changed = pixel_diff >= diff_threshold;
                }
            }

            let image_base64 = img_to_base64(&img);

            results.push(OfflineResult {
                file_name,
                pixel_diff,
                ocr_text: ocr_text.clone(),
                is_changed,
                image_base64,
            });

            prev_img = Some(img);
            if enable_ocr {
                prev_ocr = Some(ocr_text);
            }
        }
    }

    Ok(results)
}

fn img_to_base64(img: &DynamicImage) -> String {
    let mut image_data: Vec<u8> = Vec::new();
    let mut cursor = Cursor::new(&mut image_data);
    let rgb_img = img.to_rgb8();
    if rgb_img.write_to(&mut cursor, image::ImageFormat::Jpeg).is_ok() {
        let b64 = general_purpose::STANDARD.encode(image_data);
        format!("data:image/jpeg;base64,{}", b64)
    } else {
        String::new()
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(CaptureState {
            is_running: Arc::new(AtomicBool::new(false)),
        })
        .invoke_handler(tauri::generate_handler![
            list_windows,
            list_monitors,
            select_folder,
            start_capture,
            stop_capture,
            get_capture_status,
            run_offline_analysis
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
