use enigo::{Button, Coordinate, Direction, Enigo, Mouse, Settings};
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
    pub stop_after: Option<Duration>,
}

#[derive(Debug)]
pub enum Event {
    Activity {
        description: String,
        moves: u64,
        clicks: u64,
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

        let window_target = if should_click {
            find_window_target(&config.window_ids, previous_window, &mut rng)
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

        if let Err(error) = smooth_move(&mut enigo, target_x, target_y, &control, &mut rng) {
            let _ = events.send(Event::Stopped(error));
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
            format!("Flyttade och klickade på titelraden: {title}")
        } else if should_click {
            "Flyttade pekaren; inget valt fönster var tillgängligt för klick.".to_string()
        } else {
            "Flyttade pekaren.".to_string()
        };
        let _ = events.send(Event::Activity {
            description,
            moves,
            clicks,
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
            let target_y = y.saturating_add(12_i32.min((height / 2) as i32));
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
    use super::{is_emergency_corner, random_duration};
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
}
