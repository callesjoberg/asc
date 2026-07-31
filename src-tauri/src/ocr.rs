use std::fs;
use std::env;
use std::process::Command;
use tauri::AppHandle;

// Villkorlig kompilering: Bädda in rätt hjälpprogram i själva binären baserat på målsystem
#[cfg(target_os = "windows")]
const OCR_HELPER_BYTES: &[u8] = include_bytes!("../resources/ocr-helper-win.exe");

#[cfg(target_os = "macos")]
const OCR_HELPER_BYTES: &[u8] = include_bytes!("../resources/ocr-helper-macos");

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const OCR_HELPER_BYTES: &[u8] = &[];

/// Utför OCR på en bildfil genom att anropa det inbäddade hjälpprogrammet.
pub fn run_ocr(_app_handle: &AppHandle, image_path: &str) -> Result<String, String> {
    let binary_name = if cfg!(target_os = "windows") {
        "ocr-helper-win.exe"
    } else if cfg!(target_os = "macos") {
        "ocr-helper-macos"
    } else {
        return Err("OCR stöds inte på det här operativsystemet för närvarande.".to_string());
    };

    // Extrahera hjälpprogrammet till systemets temp-mapp för körning
    let temp_dir = env::temp_dir();
    let helper_path = temp_dir.join(binary_name);

    // Om filen inte redan ligger där, eller om vi vill säkerställa att den är uppdaterad, skriv ut den
    if !helper_path.exists() {
        fs::write(&helper_path, OCR_HELPER_BYTES)
            .map_err(|e| format!("Kunde inte skriva OCR-hjälpprogram till temp-katalog: {}", e))?;

        // Om vi kör på macOS (Unix), se till att sätta exekveringsrättigheter (+x)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(&helper_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o755); // Läsa/skriva/exekvera för ägaren, läsa/exekvera för andra
                let _ = fs::set_permissions(&helper_path, perms);
            }
        }
    }

    // Kör programmet som en subprocess
    let output = Command::new(&helper_path)
        .arg(image_path)
        .output()
        .map_err(|e| format!("Misslyckades att starta hjälpprogrammet {:?}: {}", helper_path, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(format!("OCR-processen returnerade ett fel: {}", stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout.trim().to_string())
}
