use image::DynamicImage;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceStatus {
    Available,
    Busy,
    DoNotDisturb,
    Away,
    BeRightBack,
    Offline,
    Unknown,
}

impl PresenceStatus {
    pub const ALL: [Self; 7] = [
        Self::Available,
        Self::Busy,
        Self::DoNotDisturb,
        Self::Away,
        Self::BeRightBack,
        Self::Offline,
        Self::Unknown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Available => "Tillgänglig",
            Self::Busy => "Upptagen",
            Self::DoNotDisturb => "Stör ej",
            Self::Away => "Frånvarande",
            Self::BeRightBack => "Strax tillbaka",
            Self::Offline => "Offline",
            Self::Unknown => "Okänd",
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub window_id: u32,
    pub person: String,
    pub ocr_area: Option<(u32, u32, u32, u32)>,
    pub color_area: Option<(u32, u32, u32, u32)>,
    pub interval: Duration,
}

pub enum Event {
    Sample {
        status: PresenceStatus,
        ocr_text: String,
    },
    Error(String),
    Stopped,
}

pub fn run(config: Config, control: Receiver<()>, events: Sender<Event>) {
    loop {
        match control.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => {
                let _ = events.send(Event::Stopped);
                return;
            }
            Err(TryRecvError::Empty) => {}
        }

        match crate::capture::capture_source("window", config.window_id, None) {
            Ok(image) => {
                let mut ocr_text = String::new();
                if let Some(area) = config.ocr_area {
                    let ocr_image = crate::capture::crop_image(&image, area);
                    let temp_path = std::env::temp_dir().join(format!(
                        "anteckningar-asc-presence-{}.png",
                        std::process::id()
                    ));
                    if let Err(error) = ocr_image.save(&temp_path) {
                        let _ = events.send(Event::Error(format!(
                            "Kunde inte skapa Teams OCR-bild: {error}"
                        )));
                    } else {
                        match crate::ocr::run_ocr(&temp_path.to_string_lossy()) {
                            Ok(text) => ocr_text = text,
                            Err(error) => {
                                let _ = events.send(Event::Error(error));
                            }
                        }
                    }
                }

                let person_matches = config.ocr_area.is_none()
                    || config.person.trim().is_empty()
                    || normalize(&ocr_text).contains(&normalize(&config.person));
                let text_status = person_matches
                    .then(|| status_from_text(&ocr_text))
                    .flatten();
                // Den visuellt markerade statusbollen är redan knuten till rätt rad.
                // Låt därför färgbytet fungera även om OCR stavar personnamnet fel.
                let status = text_status
                    .or_else(|| {
                        config
                            .color_area
                            .map(|area| status_from_color(&image, area))
                    })
                    .unwrap_or(PresenceStatus::Unknown);
                let _ = events.send(Event::Sample { status, ocr_text });
            }
            Err(error) => {
                let _ = events.send(Event::Error(error));
            }
        }

        let mut remaining = config.interval;
        while !remaining.is_zero() {
            let slice = remaining.min(Duration::from_millis(100));
            std::thread::sleep(slice);
            remaining = remaining.saturating_sub(slice);
            match control.try_recv() {
                Ok(()) | Err(TryRecvError::Disconnected) => {
                    let _ = events.send(Event::Stopped);
                    return;
                }
                Err(TryRecvError::Empty) => {}
            }
        }
    }
}

pub fn status_from_text(text: &str) -> Option<PresenceStatus> {
    let text = normalize(text);
    let mappings = [
        (
            PresenceStatus::DoNotDisturb,
            ["stör ej", "do not disturb"].as_slice(),
        ),
        (
            PresenceStatus::BeRightBack,
            ["strax tillbaka", "be right back"].as_slice(),
        ),
        (
            PresenceStatus::Available,
            ["tillgänglig", "available", "närvarande"].as_slice(),
        ),
        (
            PresenceStatus::Busy,
            ["upptagen", "busy", "i ett samtal", "i möte"].as_slice(),
        ),
        (
            PresenceStatus::Away,
            ["frånvarande", "away", "inte vid datorn"].as_slice(),
        ),
        (
            PresenceStatus::Offline,
            ["offline", "visas som offline", "appear offline"].as_slice(),
        ),
    ];
    mappings.into_iter().find_map(|(status, terms)| {
        terms
            .iter()
            .any(|term| text.contains(&normalize(term)))
            .then_some(status)
    })
}

fn status_from_color(image: &DynamicImage, area: (u32, u32, u32, u32)) -> PresenceStatus {
    let cropped = crate::capture::crop_image(image, area).to_rgb8();
    let mut green = 0_u64;
    let mut red = 0_u64;
    let mut yellow = 0_u64;
    let mut gray = 0_u64;
    for pixel in cropped.pixels() {
        let r = i32::from(pixel[0]);
        let g = i32::from(pixel[1]);
        let b = i32::from(pixel[2]);
        let maximum = r.max(g).max(b);
        let minimum = r.min(g).min(b);
        let chroma = maximum - minimum;

        // Teams-bollen är liten och får ofta kantpixlar från antialiasing och
        // skärmdelning. Jämför därför färgton och mättnad i stället för en
        // enda exakt RGB-färg.
        if g >= r + 12 && g >= b + 8 && g >= 65 && chroma >= 30 {
            green += 1;
        } else if r >= 105 && g >= 75 && r >= b + 28 && g >= b + 20 && chroma >= 35 {
            yellow += 1;
        } else if r >= 100 && r >= g + 25 && r >= b + 20 && chroma >= 35 {
            red += 1;
        } else if (r - g).abs() <= 24
            && (g - b).abs() <= 24
            && (r - b).abs() <= 24
            && (70..=205).contains(&maximum)
        {
            gray += 1;
        }
    }
    // En statusboll på hög-DPI-skärm kan ha endast ett par helt färgade
    // mittpixlar. En ensam pixel godtas därför för mycket små mätområden.
    let minimum = if cropped.width() * cropped.height() <= 16 {
        1
    } else {
        2
    };
    let (count, status) = [
        (green, PresenceStatus::Available),
        (red, PresenceStatus::Busy),
        (yellow, PresenceStatus::Away),
        (gray, PresenceStatus::Offline),
    ]
    .into_iter()
    .max_by_key(|(count, _)| *count)
    .unwrap_or((0, PresenceStatus::Unknown));
    if count >= minimum {
        status
    } else {
        PresenceStatus::Unknown
    }
}

fn normalize(value: &str) -> String {
    let mut normalized = String::with_capacity(value.len());
    let mut previous_was_space = true;
    for character in value.chars().flat_map(char::to_lowercase) {
        let character = match character {
            // OCR kan både behålla och tappa svenska diakritiska tecken.
            'å' | 'ä' => 'a',
            'ö' => 'o',
            // Stöd även för dekomponerad Unicode, t.ex. "a" + ring.
            '\u{0308}' | '\u{030a}' => continue,
            character => character,
        };
        if character.is_alphanumeric() {
            normalized.push(character);
            previous_was_space = false;
        } else if !previous_was_space {
            normalized.push(' ');
            previous_was_space = true;
        }
    }
    normalized.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_status_prioritizes_do_not_disturb_over_red_busy() {
        assert_eq!(
            status_from_text("Anna – Stör ej"),
            Some(PresenceStatus::DoNotDisturb)
        );
        assert_eq!(
            status_from_text("Status: Tillgänglig"),
            Some(PresenceStatus::Available)
        );
    }

    #[test]
    fn ocr_status_accepts_swedish_characters_and_common_ocr_variants() {
        assert_eq!(
            status_from_text("Status: Tillganglig"),
            Some(PresenceStatus::Available)
        );
        assert_eq!(
            status_from_text("Status: Sto\u{0308}r-ej"),
            Some(PresenceStatus::DoNotDisturb)
        );
        assert_eq!(
            status_from_text("Visas som Franvarande"),
            Some(PresenceStatus::Away)
        );
    }

    #[test]
    fn color_status_detects_small_antialiased_teams_balls() {
        use image::{Rgb, RgbImage};

        let mut image = RgbImage::from_pixel(7, 7, Rgb([250, 250, 250]));
        for (x, y) in [(3, 2), (2, 3), (3, 3), (4, 3), (3, 4)] {
            image.put_pixel(x, y, Rgb([109, 173, 67]));
        }
        assert_eq!(
            status_from_color(&DynamicImage::ImageRgb8(image), (0, 0, 7, 7)),
            PresenceStatus::Available
        );

        let mut image = RgbImage::from_pixel(4, 4, Rgb([255, 255, 255]));
        image.put_pixel(1, 1, Rgb([201, 67, 88]));
        assert_eq!(
            status_from_color(&DynamicImage::ImageRgb8(image), (0, 0, 4, 4)),
            PresenceStatus::Busy
        );

        let mut image = RgbImage::from_pixel(7, 7, Rgb([250, 250, 250]));
        image.put_pixel(3, 3, Rgb([245, 199, 56]));
        image.put_pixel(3, 4, Rgb([233, 184, 45]));
        assert_eq!(
            status_from_color(&DynamicImage::ImageRgb8(image), (0, 0, 7, 7)),
            PresenceStatus::Away
        );
    }

    #[test]
    fn color_status_does_not_treat_white_background_as_offline() {
        use image::{Rgb, RgbImage};

        let image = RgbImage::from_pixel(8, 8, Rgb([255, 255, 255]));
        assert_eq!(
            status_from_color(&DynamicImage::ImageRgb8(image), (0, 0, 8, 8)),
            PresenceStatus::Unknown
        );
    }
}
