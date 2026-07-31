use std::process::Command;
use tauri::{AppHandle, Manager};
use tauri::path::BaseDirectory;

/// Utför OCR på en bildfil genom att anropa det plattformsspecifika hjälpprogrammet.
pub fn run_ocr(app_handle: &AppHandle, image_path: &str) -> Result<String, String> {
    // Bestäm resursnamnet baserat på operativsystemet
    let binary_name = if cfg!(target_os = "windows") {
        "ocr-helper-win.exe"
    } else if cfg!(target_os = "macos") {
        "ocr-helper-macos"
    } else {
        return Err("OCR stöds inte på det här operativsystemet för närvarande.".to_string());
    };

    // Hitta sökvägen till resursen enligt Tauri v2 standarder
    let resource_path = app_handle
        .path()
        .resolve(format!("resources/{}", binary_name), BaseDirectory::Resource)
        .map_err(|e| format!("Kunde inte lösa resursvägen för {}: {}", binary_name, e))?;

    // Kontrollera om filen faktiskt finns innan vi anropar
    if !resource_path.exists() {
        return Err(format!(
            "Hjälpprogrammet för OCR saknas på sökvägen: {:?}. Kontrollera att det har kompilerats.",
            resource_path
        ));
    }

    // Kör programmet som en subprocess
    let output = Command::new(&resource_path)
        .arg(image_path)
        .output()
        .map_err(|e| format!("Misslyckades att starta hjälpprogrammet {:?}: {}", resource_path, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(format!("OCR-processen returnerade ett fel: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout.trim().to_string())
}
