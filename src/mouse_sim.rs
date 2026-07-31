use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use rand::Rng;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};
use xcap::Window;

#[derive(Clone, Debug)]
pub struct Config {
    pub interval_min: Duration,
    pub interval_max: Duration,
    pub pause_chance: f64,
    pub pause_min: Duration,
    pub pause_max: Duration,
    pub click_enabled: bool,
    pub click_every: u32,
    pub window_ids: Vec<u32>,
    pub typing_enabled: bool,
    pub typing_chance: f64,
    pub typing_words: Vec<String>,
    pub typing_min_words: u32,
    pub typing_max_words: u32,
    pub typing_window_id: Option<u32>,
    pub stop_after: Option<Duration>,
}

#[derive(Clone, Debug)]
pub struct OcrSequenceConfig {
    pub window_id: u32,
    pub steps: Vec<AutomationStep>,
    pub repeat: bool,
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Shortcut {
    Copy,
    Paste,
    SelectAll,
    Enter,
    Escape,
}

#[derive(Clone, Copy, Debug)]
pub enum PointerButton {
    Left,
    Right,
}

#[derive(Clone, Debug)]
pub enum AutomationStep {
    SetTargetWindow(u32),
    Wait(Duration),
    WaitForStable {
        minimum: Duration,
        stable_for: Duration,
        timeout: Duration,
        max_changed_fraction: f64,
    },
    ClickOcrWord {
        word: String,
        timeout: Duration,
    },
    ClickImage {
        template: image::DynamicImage,
        expected_area: (u32, u32, u32, u32),
        reference_size: (u32, u32),
        timeout: Duration,
        confidence: f64,
        position_tolerance: f64,
    },
    ClickRelative {
        x_fraction: f64,
        y_fraction: f64,
        button: PointerButton,
    },
    Shortcut(Shortcut),
    TypeText(String),
    PauseForConfirmation(String),
}

#[derive(Debug)]
pub enum Event {
    Activity {
        description: String,
        moves: u64,
        clicks: u64,
        typed_words: u64,
    },
    Status(String),
    Stopped(String),
}

pub fn run(config: Config, control: Receiver<()>, events: Sender<Event>) {
    let settings = Settings {
        open_prompt_to_get_permissions: true,
        ..Settings::default()
    };
    let mut enigo = match Enigo::new(&settings) {
        Ok(enigo) => enigo,
        Err(error) => {
            let _ = events.send(Event::Stopped(format!(
                "Kunde inte starta musstyrningen: {error}"
            )));
            return;
        }
    };

    let started_at = Instant::now();
    let mut rng = rand::rng();
    let mut moves = 0_u64;
    let mut clicks = 0_u64;
    let mut typed_words = 0_u64;
    let mut previous_window = None;

    loop {
        if let Some(limit) = config.stop_after {
            if started_at.elapsed() >= limit {
                let _ = events.send(Event::Stopped("Tidsgränsen nåddes.".to_string()));
                return;
            }
        }
        if let Some(reason) = interrupted(&control, &enigo, Duration::ZERO) {
            let _ = events.send(Event::Stopped(reason));
            return;
        }

        let next_move = moves + 1;
        let should_click = config.click_enabled
            && !config.window_ids.is_empty()
            && next_move.is_multiple_of(config.click_every.max(1) as u64);
        let should_type = should_click
            && config.typing_enabled
            && !config.typing_words.is_empty()
            && rng.random_bool(config.typing_chance.clamp(0.0, 1.0));

        let typing_target = config.typing_window_id.filter(|_| should_type);
        let window_target = if let Some(window_id) = typing_target {
            find_window_target(&[window_id], None, true, &mut rng)
        } else if should_click {
            find_window_target(&config.window_ids, previous_window, should_type, &mut rng)
        } else {
            None
        };
        let (target_x, target_y, target_window) = match window_target {
            Some((id, title, x, y)) => (x, y, Some((id, title))),
            None => match random_display_point(&enigo, &mut rng) {
                Ok((x, y)) => (x, y, None),
                Err(error) => {
                    let _ = events.send(Event::Stopped(error));
                    return;
                }
            },
        };

        if let Some((window_id, _)) = target_window.as_ref() {
            if let Some(reason) = activate_target(*window_id, &control, &enigo) {
                let _ = events.send(Event::Stopped(reason));
                return;
            }
        }

        if let Err(error) = smooth_move(&mut enigo, target_x, target_y, &control, &mut rng) {
            let _ = events.send(Event::Stopped(error));
            return;
        }
        // A stop request can arrive after the final movement step. Check once
        // more immediately before the irreversible action.
        if let Some(reason) = interrupted(&control, &enigo, Duration::ZERO) {
            let _ = events.send(Event::Stopped(reason));
            return;
        }
        moves += 1;

        let description = if let Some((window_id, title)) = target_window {
            if let Err(error) = enigo.button(Button::Left, Direction::Click) {
                let _ = events.send(Event::Stopped(format!("Klick misslyckades: {error}")));
                return;
            }
            clicks += 1;
            previous_window = Some(window_id);
            if should_type {
                if let Some(reason) = interrupted(
                    &control,
                    &enigo,
                    Duration::from_millis(rng.random_range(250..=700)),
                ) {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
                let word_count = rng.random_range(
                    config.typing_min_words.min(config.typing_max_words).max(1)
                        ..=config.typing_min_words.max(config.typing_max_words).max(1),
                );
                let mut text = String::new();
                for index in 0..word_count {
                    if index > 0 {
                        text.push(' ');
                    }
                    let word_index = rng.random_range(0..config.typing_words.len());
                    text.push_str(&config.typing_words[word_index]);
                }
                text.push(' ');
                if let Err(error) = enigo.text(&text) {
                    let _ = events.send(Event::Stopped(format!(
                        "Textinmatning misslyckades: {error}"
                    )));
                    return;
                }
                typed_words += u64::from(word_count);
                format!("Klickade i och skrev {word_count} ord i: {title}")
            } else {
                format!("Flyttade och klickade på titelraden: {title}")
            }
        } else if should_click {
            "Flyttade pekaren; inget valt fönster var tillgängligt för klick.".to_string()
        } else {
            "Flyttade pekaren.".to_string()
        };
        let _ = events.send(Event::Activity {
            description,
            moves,
            clicks,
            typed_words,
        });

        if rng.random_bool(config.pause_chance.clamp(0.0, 1.0)) {
            let pause = random_duration(config.pause_min, config.pause_max, &mut rng);
            let _ = events.send(Event::Status(format!(
                "Slumpmässig paus i {} sekunder…",
                pause.as_secs()
            )));
            if let Some(reason) = interrupted(&control, &enigo, pause) {
                let _ = events.send(Event::Stopped(reason));
                return;
            }
        }

        let interval = random_duration(config.interval_min, config.interval_max, &mut rng);
        let _ = events.send(Event::Status(format!(
            "Nästa rörelse om cirka {} sekunder.",
            interval.as_secs()
        )));
        if let Some(reason) = interrupted(&control, &enigo, interval) {
            let _ = events.send(Event::Stopped(reason));
            return;
        }
    }
}

pub fn run_ocr_sequence(config: OcrSequenceConfig, control: Receiver<()>, events: Sender<Event>) {
    let settings = Settings {
        open_prompt_to_get_permissions: true,
        ..Settings::default()
    };
    let mut enigo = match Enigo::new(&settings) {
        Ok(enigo) => enigo,
        Err(error) => {
            let _ = events.send(Event::Stopped(format!(
                "Kunde inte starta musstyrningen: {error}"
            )));
            return;
        }
    };
    let mut rng = rand::rng();
    let mut moves = 0_u64;
    let mut clicks = 0_u64;
    let mut active_window_id = config.window_id;

    loop {
        for (step_index, step) in config.steps.iter().enumerate() {
            if let AutomationStep::SetTargetWindow(window_id) = step {
                active_window_id = *window_id;
                let _ = events.send(Event::Activity {
                    description: format!(
                        "Steg {}: bytte målfönster för följande steg.",
                        step_index + 1
                    ),
                    moves,
                    clicks,
                    typed_words: 0,
                });
                continue;
            }
            if let AutomationStep::PauseForConfirmation(message) = step {
                let _ = events.send(Event::Stopped(format!(
                    "Pausad för manuell bekräftelse: {}",
                    message.trim()
                )));
                return;
            }
            if let AutomationStep::Wait(duration) = step {
                let effective_duration = if config.dry_run {
                    (*duration).min(Duration::from_millis(300))
                } else {
                    *duration
                };
                let _ = events.send(Event::Status(if config.dry_run {
                    format!(
                        "Steg {}: simulerar inspelad väntan på {:.2} sekunder…",
                        step_index + 1,
                        duration.as_secs_f64()
                    )
                } else {
                    format!(
                        "Steg {}: väntar {:.2} sekunder…",
                        step_index + 1,
                        duration.as_secs_f64()
                    )
                }));
                if let Some(reason) = interrupted(&control, &enigo, effective_duration) {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
                continue;
            }
            if let AutomationStep::WaitForStable {
                minimum,
                stable_for,
                timeout,
                max_changed_fraction,
            } = step
            {
                if let Some(reason) =
                    prepare_sequence_target(active_window_id, config.dry_run, &control, &enigo)
                {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
                let _ = events.send(Event::Status(format!(
                    "Steg {}: väntar minst {} sekunder på att sidan ska bli klar…",
                    step_index + 1,
                    minimum.as_secs()
                )));
                if let Some(reason) = interrupted(&control, &enigo, *minimum) {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }

                let stability_started = Instant::now();
                let mut previous =
                    match crate::capture::capture_source("window", active_window_id, None) {
                        Ok(image) => image,
                        Err(error) => {
                            let _ = events.send(Event::Stopped(error));
                            return;
                        }
                    };
                let mut stable_since = None;
                loop {
                    if stability_started.elapsed() >= *timeout {
                        let _ = events.send(Event::Stopped(format!(
                            "Steg {}: sidan blev inte stabil inom {} sekunder efter minimitiden.",
                            step_index + 1,
                            timeout.as_secs()
                        )));
                        return;
                    }
                    if let Some(reason) = interrupted(&control, &enigo, Duration::from_millis(500))
                    {
                        let _ = events.send(Event::Stopped(reason));
                        return;
                    }
                    let current =
                        match crate::capture::capture_source("window", active_window_id, None) {
                            Ok(image) => image,
                            Err(error) => {
                                let _ = events.send(Event::Stopped(error));
                                return;
                            }
                        };
                    let difference = crate::analysis::analyze_images(&previous, &current, 24);
                    let pixel_count = u64::from(current.width()) * u64::from(current.height());
                    let changed_fraction = if pixel_count == 0 {
                        1.0
                    } else {
                        difference.changed_pixels as f64 / pixel_count as f64
                    };
                    if changed_fraction <= max_changed_fraction.clamp(0.0, 1.0) {
                        let since = stable_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= *stable_for {
                            let _ = events.send(Event::Activity {
                                description: format!(
                                    "Steg {}: sidan är stabil och flödet fortsätter.",
                                    step_index + 1
                                ),
                                moves,
                                clicks,
                                typed_words: 0,
                            });
                            break;
                        }
                    } else {
                        stable_since = None;
                    }
                    previous = current;
                }
                continue;
            }
            if let AutomationStep::Shortcut(shortcut) = step {
                if let Some(reason) =
                    prepare_sequence_target(active_window_id, config.dry_run, &control, &enigo)
                {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
                if let Some(reason) = interrupted(&control, &enigo, Duration::ZERO) {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
                if !config.dry_run {
                    if let Err(error) = send_shortcut(&mut enigo, *shortcut) {
                        let _ = events.send(Event::Stopped(error));
                        return;
                    }
                }
                let _ = events.send(Event::Activity {
                    description: format!(
                        "Steg {}: {}kortkommandot {}.",
                        step_index + 1,
                        if config.dry_run {
                            "verifierade "
                        } else {
                            "skickade "
                        },
                        shortcut_label(*shortcut)
                    ),
                    moves,
                    clicks,
                    typed_words: 0,
                });
                continue;
            }
            if let AutomationStep::TypeText(text) = step {
                if let Some(reason) =
                    prepare_sequence_target(active_window_id, config.dry_run, &control, &enigo)
                {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
                if !config.dry_run {
                    if let Err(error) = enigo.text(text) {
                        let _ = events.send(Event::Stopped(format!(
                            "Textinmatning misslyckades: {error}"
                        )));
                        return;
                    }
                }
                let _ = events.send(Event::Activity {
                    description: format!(
                        "Steg {}: {} textinmatning.",
                        step_index + 1,
                        if config.dry_run {
                            "verifierade"
                        } else {
                            "utförde"
                        }
                    ),
                    moves,
                    clicks,
                    typed_words: text.split_whitespace().count() as u64,
                });
                continue;
            }
            let (target_label, timeout, template) = match step {
                AutomationStep::ClickOcrWord { word, timeout } => (word.clone(), *timeout, None),
                AutomationStep::ClickImage {
                    template,
                    expected_area,
                    reference_size,
                    timeout,
                    confidence,
                    position_tolerance,
                } => (
                    format!("bild ({:.0} %)", confidence * 100.0),
                    *timeout,
                    Some((
                        template.clone(),
                        *confidence,
                        *expected_area,
                        *reference_size,
                        *position_tolerance,
                    )),
                ),
                AutomationStep::ClickRelative {
                    x_fraction,
                    y_fraction,
                    button,
                } => {
                    if let Some(reason) =
                        prepare_sequence_target(active_window_id, config.dry_run, &control, &enigo)
                    {
                        let _ = events.send(Event::Stopped(reason));
                        return;
                    }
                    let geometry = match crate::capture::window_geometry(active_window_id) {
                        Ok(geometry) => geometry,
                        Err(error) => {
                            let _ = events.send(Event::Stopped(error));
                            return;
                        }
                    };
                    let (target_x, target_y) =
                        relative_window_point(geometry, *x_fraction, *y_fraction);
                    if !config.dry_run {
                        if let Err(error) =
                            smooth_move(&mut enigo, target_x, target_y, &control, &mut rng)
                        {
                            let _ = events.send(Event::Stopped(error));
                            return;
                        }
                        if let Some(reason) = interrupted(&control, &enigo, Duration::ZERO) {
                            let _ = events.send(Event::Stopped(reason));
                            return;
                        }
                        let enigo_button = match button {
                            PointerButton::Left => Button::Left,
                            PointerButton::Right => Button::Right,
                        };
                        if let Err(error) = enigo.button(enigo_button, Direction::Click) {
                            let _ =
                                events.send(Event::Stopped(format!("Klick misslyckades: {error}")));
                            return;
                        }
                        moves += 1;
                        clicks += 1;
                    }
                    let _ = events.send(Event::Activity {
                        description: if config.dry_run {
                            format!(
                                "Steg {}: målfönstret finns; beräknad målpunkt är {:.1} %, {:.1} %. Innehållet på punkten är inte bildverifierat.",
                                step_index + 1,
                                x_fraction * 100.0,
                                y_fraction * 100.0
                            )
                        } else {
                            format!(
                                "Steg {}: utförde {}klick vid {:.1} %, {:.1} % i målfönstret.",
                                step_index + 1,
                                match button {
                                    PointerButton::Left => "vänster",
                                    PointerButton::Right => "höger",
                                },
                                x_fraction * 100.0,
                                y_fraction * 100.0
                            )
                        },
                        moves,
                        clicks,
                        typed_words: 0,
                    });
                    continue;
                }
                _ => continue,
            };
            if let Some(reason) =
                prepare_sequence_target(active_window_id, config.dry_run, &control, &enigo)
            {
                let _ = events.send(Event::Stopped(reason));
                return;
            }
            let search_started = Instant::now();
            let word = loop {
                if let Some(reason) = interrupted(&control, &enigo, Duration::ZERO) {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
                if search_started.elapsed() >= timeout {
                    let _ = events.send(Event::Stopped(format!(
                        "Tidsgränsen nåddes: hittade inte {target_label}."
                    )));
                    return;
                }

                let image = match crate::capture::capture_source("window", active_window_id, None) {
                    Ok(image) => image,
                    Err(error) => {
                        let _ = events.send(Event::Stopped(format!(
                            "Kunde inte läsa det valda fönstret: {error}"
                        )));
                        return;
                    }
                };
                let found = if let Some((template, confidence, area, size, tolerance)) = &template {
                    find_image_target(&image, template, *confidence, *area, *size, *tolerance)
                } else {
                    let temp_path = std::env::temp_dir().join(format!(
                        "asc-ocr-sequence-{}-{}.png",
                        std::process::id(),
                        step_index
                    ));
                    if let Err(error) = image.save(&temp_path) {
                        let _ = events.send(Event::Stopped(format!(
                            "Kunde inte skapa OCR-bilden: {error}"
                        )));
                        return;
                    }
                    let recognized = match crate::ocr::run_ocr_words(&temp_path.to_string_lossy()) {
                        Ok(words) => words,
                        Err(error) => {
                            let _ = events.send(Event::Stopped(error));
                            return;
                        }
                    };
                    find_ocr_target(&recognized, &target_label)
                };
                if let Some(word) = found {
                    break (word, image.width(), image.height());
                }

                let _ = events.send(Event::Status(format!(
                    "Steg {}: söker efter ‘{}’…",
                    step_index + 1,
                    target_label
                )));
                if let Some(reason) = interrupted(&control, &enigo, Duration::from_secs(1)) {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
            };

            if let Some(reason) =
                prepare_sequence_target(active_window_id, config.dry_run, &control, &enigo)
            {
                let _ = events.send(Event::Stopped(reason));
                return;
            }
            let geometry = match crate::capture::window_geometry(active_window_id) {
                Ok(geometry) => geometry,
                Err(error) => {
                    let _ = events.send(Event::Stopped(error));
                    return;
                }
            };
            let center_x = word.0.x + word.0.width / 2.0;
            let center_y = word.0.y + word.0.height / 2.0;
            let target_x = geometry.x
                + (center_x / f64::from(word.1) * f64::from(geometry.width)).round() as i32;
            let target_y = geometry.y
                + (center_y / f64::from(word.2) * f64::from(geometry.height)).round() as i32;
            if !config.dry_run {
                if let Err(error) = smooth_move(&mut enigo, target_x, target_y, &control, &mut rng)
                {
                    let _ = events.send(Event::Stopped(error));
                    return;
                }
                // Do not click if a stop request arrived while the pointer was
                // settling on the target.
                if let Some(reason) = interrupted(&control, &enigo, Duration::ZERO) {
                    let _ = events.send(Event::Stopped(reason));
                    return;
                }
                moves += 1;
                if let Err(error) = enigo.button(Button::Left, Direction::Click) {
                    let _ = events.send(Event::Stopped(format!("Klick misslyckades: {error}")));
                    return;
                }
                clicks += 1;
            }
            let _ = events.send(Event::Activity {
                description: format!(
                    "Steg {}: {} målet ‘{}’.",
                    step_index + 1,
                    if config.dry_run {
                        "hittade"
                    } else {
                        "klickade på"
                    },
                    word.0.text
                ),
                moves,
                clicks,
                typed_words: 0,
            });
        }

        if !config.repeat {
            let _ = events.send(Event::Stopped(if config.dry_run {
                "Torrkörningen är klar. Inga klick eller textinmatningar utfördes.".to_string()
            } else {
                "RPA-sekvensen är klar.".to_string()
            }));
            return;
        }
    }
}

fn shortcut_label(shortcut: Shortcut) -> &'static str {
    match shortcut {
        Shortcut::Copy => "Kopiera",
        Shortcut::Paste => "Klistra in",
        Shortcut::SelectAll => "Markera allt",
        Shortcut::Enter => "Enter",
        Shortcut::Escape => "Escape",
    }
}

fn relative_window_point(
    geometry: crate::capture::WindowGeometry,
    x_fraction: f64,
    y_fraction: f64,
) -> (i32, i32) {
    (
        geometry.x + (x_fraction.clamp(0.0, 1.0) * f64::from(geometry.width)).round() as i32,
        geometry.y + (y_fraction.clamp(0.0, 1.0) * f64::from(geometry.height)).round() as i32,
    )
}

fn send_shortcut(enigo: &mut Enigo, shortcut: Shortcut) -> Result<(), String> {
    let single_key = match shortcut {
        Shortcut::Enter => Some(Key::Return),
        Shortcut::Escape => Some(Key::Escape),
        _ => None,
    };
    if let Some(key) = single_key {
        return enigo
            .key(key, Direction::Click)
            .map_err(|error| format!("Kortkommandot misslyckades: {error}"));
    }

    #[cfg(target_os = "macos")]
    let modifier = Key::Meta;
    #[cfg(not(target_os = "macos"))]
    let modifier = Key::Control;
    let character = match shortcut {
        Shortcut::Copy => 'c',
        Shortcut::Paste => 'v',
        Shortcut::SelectAll => 'a',
        Shortcut::Enter | Shortcut::Escape => unreachable!(),
    };

    enigo
        .key(modifier, Direction::Press)
        .map_err(|error| format!("Kortkommandot misslyckades: {error}"))?;
    let result = enigo.key(Key::Unicode(character), Direction::Click);
    let release_result = enigo.key(modifier, Direction::Release);
    result
        .and(release_result)
        .map_err(|error| format!("Kortkommandot misslyckades: {error}"))
}

fn prepare_sequence_target(
    window_id: u32,
    dry_run: bool,
    control: &Receiver<()>,
    enigo: &Enigo,
) -> Option<String> {
    if dry_run {
        if let Some(reason) = interrupted(control, enigo, Duration::ZERO) {
            return Some(reason);
        }
        return crate::capture::window_geometry(window_id)
            .err()
            .map(|error| format!("Kunde inte verifiera målfönstret: {error}"));
    }
    activate_target(window_id, control, enigo)
}

fn activate_target(window_id: u32, control: &Receiver<()>, enigo: &Enigo) -> Option<String> {
    if let Err(error) = crate::capture::focus_window(window_id) {
        return Some(format!("Kunde inte aktivera målfönstret: {error}"));
    }
    if let Some(reason) = interrupted(control, enigo, Duration::from_millis(350)) {
        return Some(reason);
    }
    crate::capture::verify_foreground_window(window_id).err()
}

fn find_image_target(
    source: &image::DynamicImage,
    template: &image::DynamicImage,
    confidence: f64,
    expected_area: (u32, u32, u32, u32),
    reference_size: (u32, u32),
    position_tolerance: f64,
) -> Option<crate::ocr::OcrWord> {
    if template.width() == 0
        || template.height() == 0
        || reference_size.0 == 0
        || reference_size.1 == 0
    {
        return None;
    }

    let width_scale = source.width() as f64 / f64::from(reference_size.0);
    let height_scale = source.height() as f64 / f64::from(reference_size.1);
    let expected_x = (f64::from(expected_area.0) * width_scale).round() as u32;
    let expected_y = (f64::from(expected_area.1) * height_scale).round() as u32;
    let template_width = (f64::from(expected_area.2) * width_scale).round().max(1.0) as u32;
    let template_height = (f64::from(expected_area.3) * height_scale).round().max(1.0) as u32;
    if template_width > source.width() || template_height > source.height() {
        return None;
    }
    let resized_template = image::DynamicImage::ImageRgba8(image::imageops::resize(
        &template.to_rgba8(),
        template_width,
        template_height,
        image::imageops::FilterType::Triangle,
    ));

    // Matcha på en mindre gråskalebild för att hålla sökningen snabb även i stora fönster.
    let minimum_scale = if template_width.min(template_height) >= 8 {
        2
    } else {
        1
    };
    let scale = template_width
        .max(template_height)
        .div_ceil(48)
        .max(minimum_scale);
    let source_small = image::imageops::resize(
        &source.to_luma8(),
        (source.width() / scale).max(1),
        (source.height() / scale).max(1),
        image::imageops::FilterType::Triangle,
    );
    let template_small = image::imageops::resize(
        &resized_template.to_luma8(),
        (template_width / scale).max(1),
        (template_height / scale).max(1),
        image::imageops::FilterType::Triangle,
    );
    if template_small.width() > source_small.width()
        || template_small.height() > source_small.height()
    {
        return None;
    }

    let sample_count = u64::from(template_small.width()) * u64::from(template_small.height());
    let maximum_error =
        ((1.0 - confidence.clamp(0.0, 1.0)) * 255.0 * sample_count as f64).round() as u64;
    let mut best = None;
    let mut best_error = maximum_error.saturating_add(1);
    let tolerance = position_tolerance.clamp(0.0, 1.0);
    let tolerance_x = (f64::from(source.width()) * tolerance).round() as u32;
    let tolerance_y = (f64::from(source.height()) * tolerance).round() as u32;
    let valid_x_max = source.width() - template_width;
    let valid_y_max = source.height() - template_height;
    let search_x_min = expected_x.saturating_sub(tolerance_x).min(valid_x_max) / scale;
    let search_y_min = expected_y.saturating_sub(tolerance_y).min(valid_y_max) / scale;
    let search_x_max = expected_x.saturating_add(tolerance_x).min(valid_x_max) / scale;
    let search_y_max = expected_y.saturating_add(tolerance_y).min(valid_y_max) / scale;
    for y in search_y_min..=search_y_max {
        for x in search_x_min..=search_x_max {
            let mut error = 0_u64;
            'pixels: for ty in 0..template_small.height() {
                for tx in 0..template_small.width() {
                    error += u64::from(
                        source_small.get_pixel(x + tx, y + ty)[0]
                            .abs_diff(template_small.get_pixel(tx, ty)[0]),
                    );
                    if error >= best_error {
                        break 'pixels;
                    }
                }
            }
            if error < best_error {
                best_error = error;
                best = Some((x, y));
            }
        }
    }

    let (x, y) = best.filter(|_| best_error <= maximum_error)?;
    Some(crate::ocr::OcrWord {
        text: "bild".to_string(),
        x: f64::from(x * scale),
        y: f64::from(y * scale),
        width: f64::from(template_width),
        height: f64::from(template_height),
    })
}

fn find_ocr_target(words: &[crate::ocr::OcrWord], requested: &str) -> Option<crate::ocr::OcrWord> {
    let requested_tokens = requested
        .split_whitespace()
        .map(normalize_ocr_token)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    if requested_tokens.is_empty() {
        return None;
    }

    if requested_tokens.len() == 1 {
        let requested = &requested_tokens[0];
        return words
            .iter()
            .find(|word| normalize_ocr_token(&word.text) == *requested)
            .or_else(|| {
                words
                    .iter()
                    .find(|word| normalize_ocr_token(&word.text).contains(requested))
            })
            .cloned();
    }

    let normalized_words = words
        .iter()
        .map(|word| normalize_ocr_token(&word.text))
        .collect::<Vec<_>>();
    let start = normalized_words
        .windows(requested_tokens.len())
        .position(|candidate| candidate == requested_tokens.as_slice())?;
    let matches = &words[start..start + requested_tokens.len()];
    let min_x = matches
        .iter()
        .map(|word| word.x)
        .fold(f64::INFINITY, f64::min);
    let min_y = matches
        .iter()
        .map(|word| word.y)
        .fold(f64::INFINITY, f64::min);
    let max_x = matches
        .iter()
        .map(|word| word.x + word.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = matches
        .iter()
        .map(|word| word.y + word.height)
        .fold(f64::NEG_INFINITY, f64::max);
    Some(crate::ocr::OcrWord {
        text: matches
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

fn normalize_ocr_token(token: &str) -> String {
    token
        .trim_matches(|character: char| !character.is_alphanumeric())
        .to_lowercase()
}

fn random_display_point(enigo: &Enigo, rng: &mut impl Rng) -> Result<(i32, i32), String> {
    let (width, height) = enigo
        .main_display()
        .map_err(|error| format!("Kunde inte läsa skärmstorleken: {error}"))?;
    if width < 80 || height < 80 {
        return Err("Skärmen är för liten för säker musautomatisering.".to_string());
    }
    Ok((
        rng.random_range(40..=(width - 40)),
        rng.random_range(40..=(height - 40)),
    ))
}

fn find_window_target(
    selected_ids: &[u32],
    previous_window: Option<u32>,
    inside_content: bool,
    rng: &mut impl Rng,
) -> Option<(u32, String, i32, i32)> {
    let windows = Window::all().ok()?;
    let mut candidates = windows
        .into_iter()
        .filter_map(|window| {
            let id = window.id().ok()?;
            if !selected_ids.contains(&id) || window.is_minimized().unwrap_or(false) {
                return None;
            }
            let x = window.x().ok()?;
            let y = window.y().ok()?;
            let width = window.width().ok()?;
            let height = window.height().ok()?;
            if width < 120 || height < 40 {
                return None;
            }
            let title = window
                .title()
                .ok()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| window.app_name().unwrap_or_else(|_| "Fönster".to_string()));
            let target_x = x.saturating_add((width / 2) as i32);
            let target_y = if inside_content {
                y.saturating_add(((height as f64 * 0.6) as i32).clamp(30, height as i32 - 10))
            } else {
                y.saturating_add(12_i32.min((height / 2) as i32))
            };
            Some((id, title, target_x, target_y))
        })
        .collect::<Vec<_>>();

    if candidates.len() > 1 {
        candidates.retain(|candidate| Some(candidate.0) != previous_window);
    }
    if candidates.is_empty() {
        None
    } else {
        Some(candidates.swap_remove(rng.random_range(0..candidates.len())))
    }
}

fn smooth_move(
    enigo: &mut Enigo,
    target_x: i32,
    target_y: i32,
    control: &Receiver<()>,
    rng: &mut impl Rng,
) -> Result<(), String> {
    let (start_x, start_y) = enigo
        .location()
        .map_err(|error| format!("Kunde inte läsa muspositionen: {error}"))?;
    if is_emergency_corner(start_x, start_y) {
        return Err("Stoppad: muspekaren placerades i övre vänstra hörnet.".to_string());
    }

    let duration_ms = rng.random_range(350_u64..=1_100_u64);
    let steps = (duration_ms / 16).max(12);
    for step in 1..=steps {
        match control.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => {
                return Err("Stoppad av användaren.".to_string());
            }
            Err(TryRecvError::Empty) => {}
        }
        let t = step as f64 / steps as f64;
        let eased = t * t * (3.0 - 2.0 * t);
        let jitter = if step == steps {
            (0, 0)
        } else {
            (rng.random_range(-2..=2), rng.random_range(-2..=2))
        };
        let x = start_x as f64 + (target_x - start_x) as f64 * eased;
        let y = start_y as f64 + (target_y - start_y) as f64 * eased;
        enigo
            .move_mouse(
                x.round() as i32 + jitter.0,
                y.round() as i32 + jitter.1,
                Coordinate::Abs,
            )
            .map_err(|error| format!("Musrörelsen misslyckades: {error}"))?;
        std::thread::sleep(Duration::from_millis(16));
    }
    Ok(())
}

fn interrupted(control: &Receiver<()>, enigo: &Enigo, duration: Duration) -> Option<String> {
    let deadline = Instant::now() + duration;
    loop {
        match control.try_recv() {
            Ok(()) | Err(TryRecvError::Disconnected) => {
                return Some("Stoppad av användaren.".to_string());
            }
            Err(TryRecvError::Empty) => {}
        }
        if let Ok((x, y)) = enigo.location() {
            if is_emergency_corner(x, y) {
                return Some("Stoppad med nödstoppet i övre vänstra hörnet.".to_string());
            }
        }
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        std::thread::sleep((deadline - now).min(Duration::from_millis(100)));
    }
}

fn random_duration(minimum: Duration, maximum: Duration, rng: &mut impl Rng) -> Duration {
    let min_ms = minimum.as_millis().min(u64::MAX as u128) as u64;
    let max_ms = maximum.as_millis().min(u64::MAX as u128) as u64;
    Duration::from_millis(rng.random_range(min_ms.min(max_ms)..=min_ms.max(max_ms)))
}

fn is_emergency_corner(x: i32, y: i32) -> bool {
    (0..=3).contains(&x) && (0..=3).contains(&y)
}

#[cfg(test)]
mod tests {
    use super::{
        find_image_target, find_ocr_target, is_emergency_corner, normalize_ocr_token,
        random_duration, relative_window_point,
    };
    use image::{DynamicImage, ImageBuffer, Rgba};
    use std::time::Duration;

    #[test]
    fn duration_range_accepts_reversed_limits() {
        let mut rng = rand::rng();
        for _ in 0..100 {
            let duration =
                random_duration(Duration::from_secs(10), Duration::from_secs(5), &mut rng);
            assert!((Duration::from_secs(5)..=Duration::from_secs(10)).contains(&duration));
        }
    }

    #[test]
    fn emergency_corner_is_small_and_explicit() {
        assert!(is_emergency_corner(0, 0));
        assert!(is_emergency_corner(3, 3));
        assert!(!is_emergency_corner(4, 3));
        assert!(!is_emergency_corner(-1, -1));
        assert!(!is_emergency_corner(100, 100));
    }

    #[test]
    fn recorded_clicks_follow_window_move_and_resize() {
        let geometry = crate::capture::WindowGeometry {
            x: 100,
            y: 50,
            width: 800,
            height: 600,
        };
        assert_eq!(relative_window_point(geometry, 0.25, 0.75), (300, 500));
        assert_eq!(relative_window_point(geometry, -1.0, 2.0), (100, 650));
    }

    #[test]
    fn multi_word_ocr_target_combines_the_click_area() {
        let words = vec![
            crate::ocr::OcrWord {
                text: "Ladda".to_string(),
                x: 10.0,
                y: 20.0,
                width: 40.0,
                height: 12.0,
            },
            crate::ocr::OcrWord {
                text: "ner".to_string(),
                x: 55.0,
                y: 20.0,
                width: 25.0,
                height: 12.0,
            },
        ];

        let target = find_ocr_target(&words, "ladda ner").expect("phrase should be found");
        assert_eq!(target.text, "Ladda ner");
        assert_eq!(
            (target.x, target.y, target.width, target.height),
            (10.0, 20.0, 70.0, 12.0)
        );
    }

    #[test]
    fn ocr_normalization_preserves_swedish_letters() {
        assert_eq!(
            normalize_ocr_token("  \u{201e}\u{00c5}tg\u{00e4}rder,\u{201d} "),
            "\u{00e5}tg\u{00e4}rder"
        );
        assert_eq!(normalize_ocr_token("F\u{00d6}NSTER"), "f\u{00f6}nster");
    }

    #[test]
    fn image_target_finds_reference_inside_window_capture() {
        let mut source = ImageBuffer::from_pixel(20, 20, Rgba([0, 0, 0, 255]));
        let template =
            ImageBuffer::from_fn(4, 3, |x, y| Rgba([(x * 40 + y * 20 + 40) as u8, 0, 0, 255]));
        for (x, y, pixel) in template.enumerate_pixels() {
            source.put_pixel(x + 1, y + 1, *pixel);
            source.put_pixel(x + 7, y + 9, *pixel);
        }

        let target = find_image_target(
            &DynamicImage::ImageRgba8(source),
            &DynamicImage::ImageRgba8(template),
            0.99,
            (7, 9, 4, 3),
            (20, 20),
            0.10,
        )
        .expect("reference should be found");
        assert_eq!((target.x, target.y), (7.0, 9.0));
    }
}
