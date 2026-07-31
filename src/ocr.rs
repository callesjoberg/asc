use std::env;
use std::fs;
use std::process::Command;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct OcrWord {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
// Villkorlig kompilering: Bädda in rätt hjälpprogram i själva binären baserat på målsystem
#[cfg(target_os = "windows")]
const OCR_HELPER_BYTES: &[u8] = include_bytes!("../resources/ocr-helper-win.exe");

#[cfg(target_os = "macos")]
const OCR_HELPER_BYTES: &[u8] = include_bytes!("../resources/ocr-helper-macos");

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const OCR_HELPER_BYTES: &[u8] = &[];

/// Utför OCR på en bildfil genom att anropa det inbäddade hjälpprogrammet.
pub fn run_ocr(image_path: &str) -> Result<String, String> {
    run_helper(image_path, false)
}

pub fn run_ocr_words(image_path: &str) -> Result<Vec<OcrWord>, String> {
    let output = run_helper(image_path, true)?;
    serde_json::from_str(&output)
        .map_err(|error| format!("OCR-hjälparen returnerade ogiltiga ordpositioner: {error}"))
}

fn run_helper(image_path: &str, words_json: bool) -> Result<String, String> {
    let binary_name = if cfg!(target_os = "windows") {
        "ocr-helper-win.exe"
    } else if cfg!(target_os = "macos") {
        "ocr-helper-macos"
    } else {
        return Err("OCR stöds inte på det här operativsystemet för närvarande.".to_string());
    };

    // Extrahera hjälpprogrammet till systemets temp-mapp för körning
    let temp_dir = env::temp_dir();
    let helper_path = temp_dir.join(format!(
        "asc-{}-{}-{}",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        binary_name
    ));

    // Skriv om hjälparen om de inbäddade bytesen har ändrats, även inom samma utvecklingsversion.
    let helper_is_current = fs::read(&helper_path)
        .map(|existing| existing == OCR_HELPER_BYTES)
        .unwrap_or(false);
    if !helper_is_current {
        fs::write(&helper_path, OCR_HELPER_BYTES).map_err(|e| {
            format!(
                "Kunde inte skriva OCR-hjälpprogram till temp-katalog: {}",
                e
            )
        })?;

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
    let mut command = Command::new(&helper_path);
    command.arg(image_path);
    if words_json {
        command.arg("--words-json");
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command.output().map_err(|e| {
        format!(
            "Misslyckades att starta hjälpprogrammet {:?}: {}",
            helper_path, e
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let details = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!("OCR-processen returnerade ett fel: {details}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    Ok(stdout.trim().to_string())
}
