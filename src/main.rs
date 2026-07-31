#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod analysis;
mod capture;
mod mouse_sim;
mod ocr;

use chrono::Local;
use eframe::egui;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

#[derive(Clone, serde::Serialize)]
struct LogItem {
    timestamp: String,
    pixel_diff: f64,
    changed_pixels: u64,
    ocr_text: String,
    is_changed: bool,
    file_name: String,
}

#[derive(Clone, serde::Serialize)]
struct KeywordColorResult {
    timestamp: String,
    file_name: String,
    keyword: String,
    color: analysis::IndicatorColor,
    ocr_text: String,
}

enum ControlMessage {
    Stop,
}

enum WorkerMessage {
    Log(LogItem),
    Preview(Vec<u8>, usize, usize), // RGBA-pixlar, bredd, höjd
    Error(String),
    KeywordColor(KeywordColorResult),
    OfflineDone(String),
}

#[derive(Clone)]
struct AutomationStepEditor {
    kind: String,
    value: String,
    seconds: u64,
    confidence: f64,
}

impl AutomationStepEditor {
    fn click_word() -> Self {
        Self {
            kind: "click_word".to_string(),
            value: String::new(),
            seconds: 30,
            confidence: 90.0,
        }
    }

    fn wait() -> Self {
        Self {
            kind: "wait".to_string(),
            value: String::new(),
            seconds: 3,
            confidence: 90.0,
        }
    }
}

struct AscApp {
    active_tab: String,

    // Gränssnittsinställningar
    mode: String,        // "live" eller "offline"
    source_type: String, // "window" eller "screen"
    selected_source_id: u32,
    save_dir: String,
    interval_secs: u64,
    threshold_pct: f64,
    detect_small_changes: bool,
    small_change_min_pixels: u64,
    small_change_color_delta: u8,
    enable_crop: bool,
    crop_x: u32,
    crop_y: u32,
    crop_w: u32,
    crop_h: u32,
    enable_ocr: bool,
    enable_ocr_crop: bool,
    ocr_x: u32,
    ocr_y: u32,
    ocr_w: u32,
    ocr_h: u32,
    enable_keyword_color: bool,
    keyword: String,
    keyword_delay_frames: u32,
    keyword_rising_edge_only: bool,
    indicator_x: u32,
    indicator_y: u32,
    indicator_w: u32,
    indicator_h: u32,
    indicator_color_delta: u8,
    indicator_min_pixels: u64,
    ocr_regions: Vec<(u32, u32, u32, u32)>,
    measurement_regions: Vec<(u32, u32, u32, u32)>,
    region_editor_active: bool,
    region_editor_kind: String,
    region_drag_start: Option<egui::Pos2>,
    region_source_type: String,
    region_source_id: Option<u32>,
    region_source_size: Option<[usize; 2]>,

    // Status och historik
    is_running: bool,
    status_text: String,
    logs: Vec<LogItem>,
    diffs_history: Vec<f64>,
    total_captures: usize,
    total_changes: usize,
    keyword_results: Vec<KeywordColorResult>,
    export_pending: bool,
    last_export_at: Instant,

    // Källlistor (cachar)
    windows: Vec<capture::WindowInfo>,
    monitors: Vec<capture::MonitorInfo>,

    // Bildförhandsgranskning
    preview_texture: Option<egui::TextureHandle>,
    preview_size: Option<[usize; 2]>,
    latest_preview_texture: Option<egui::TextureHandle>,
    latest_preview_size: Option<[usize; 2]>,
    preview_zoom: f32,
    preview_pan: egui::Vec2,
    follow_latest_preview: bool,
    selected_preview_file: Option<String>,

    // Trådkommunikation
    log_receiver: Option<Receiver<WorkerMessage>>,
    control_sender: Option<Sender<ControlMessage>>,
    folder_receiver: Option<Receiver<Option<String>>>,

    // Musautomatisering
    mouse_running: bool,
    mouse_status: String,
    mouse_last_activity: String,
    mouse_moves: u64,
    mouse_clicks: u64,
    mouse_typed_words: u64,
    mouse_interval_min: u64,
    mouse_interval_max: u64,
    mouse_pause_chance: f64,
    mouse_pause_min: u64,
    mouse_pause_max: u64,
    mouse_click_enabled: bool,
    mouse_click_every: u32,
    mouse_typing_enabled: bool,
    mouse_typing_chance: f64,
    mouse_typing_words: String,
    mouse_typing_min_words: u32,
    mouse_typing_max_words: u32,
    mouse_typing_window_id: u32,
    mouse_selected_windows: HashSet<u32>,
    mouse_stop_after_enabled: bool,
    mouse_stop_after_minutes: u64,
    mouse_receiver: Option<Receiver<mouse_sim::Event>>,
    mouse_control_sender: Option<Sender<()>>,

    // OCR-styrd RPA-sekvens
    automation_running: bool,
    automation_status: String,
    automation_last_activity: String,
    automation_window_id: u32,
    automation_steps: Vec<AutomationStepEditor>,
    automation_repeat: bool,
    automation_receiver: Option<Receiver<mouse_sim::Event>>,
    automation_control_sender: Option<Sender<()>>,
    automation_image_picker_receiver: Option<Receiver<Option<(usize, String)>>>,
}

impl Default for AscApp {
    fn default() -> Self {
        let mut app = Self {
            active_tab: "analysis".to_string(),
            mode: "live".to_string(),
            source_type: "screen".to_string(),
            selected_source_id: 0,
            save_dir: String::new(),
            interval_secs: 5,
            threshold_pct: 1.0,
            detect_small_changes: true,
            small_change_min_pixels: 5,
            small_change_color_delta: 24,
            enable_crop: false,
            crop_x: 0,
            crop_y: 0,
            crop_w: 800,
            crop_h: 600,
            enable_ocr: false,
            enable_ocr_crop: false,
            ocr_x: 0,
            ocr_y: 0,
            ocr_w: 300,
            ocr_h: 50,
            enable_keyword_color: false,
            keyword: "öppet".to_string(),
            keyword_delay_frames: 0,
            keyword_rising_edge_only: true,
            indicator_x: 0,
            indicator_y: 0,
            indicator_w: 20,
            indicator_h: 20,
            indicator_color_delta: 24,
            indicator_min_pixels: 5,
            ocr_regions: Vec::new(),
            measurement_regions: Vec::new(),
            region_editor_active: false,
            region_editor_kind: "screenshot".to_string(),
            region_drag_start: None,
            region_source_type: String::new(),
            region_source_id: None,
            region_source_size: None,
            is_running: false,
            status_text: "Klar att starta".to_string(),
            logs: Vec::new(),
            diffs_history: Vec::new(),
            total_captures: 0,
            total_changes: 0,
            keyword_results: Vec::new(),
            export_pending: false,
            last_export_at: Instant::now(),
            windows: Vec::new(),
            monitors: Vec::new(),
            preview_texture: None,
            preview_size: None,
            latest_preview_texture: None,
            latest_preview_size: None,
            preview_zoom: 1.0,
            preview_pan: egui::Vec2::ZERO,
            follow_latest_preview: true,
            selected_preview_file: None,
            log_receiver: None,
            control_sender: None,
            folder_receiver: None,
            mouse_running: false,
            mouse_status: "Klar att starta".to_string(),
            mouse_last_activity: "Ingen aktivitet ännu.".to_string(),
            mouse_moves: 0,
            mouse_clicks: 0,
            mouse_typed_words: 0,
            mouse_interval_min: 5,
            mouse_interval_max: 15,
            mouse_pause_chance: 20.0,
            mouse_pause_min: 10,
            mouse_pause_max: 30,
            mouse_click_enabled: false,
            mouse_click_every: 3,
            mouse_typing_enabled: false,
            mouse_typing_chance: 30.0,
            mouse_typing_words: "hej,anteckning,test,grön,röd,grå".to_string(),
            mouse_typing_min_words: 1,
            mouse_typing_max_words: 3,
            mouse_typing_window_id: 0,
            mouse_selected_windows: HashSet::new(),
            mouse_stop_after_enabled: true,
            mouse_stop_after_minutes: 60,
            mouse_receiver: None,
            mouse_control_sender: None,
            automation_running: false,
            automation_status: "Klar att starta".to_string(),
            automation_last_activity: "Ingen aktivitet ännu.".to_string(),
            automation_window_id: 0,
            automation_steps: vec![
                AutomationStepEditor::click_word(),
                AutomationStepEditor::wait(),
            ],
            automation_repeat: false,
            automation_receiver: None,
            automation_control_sender: None,
            automation_image_picker_receiver: None,
        };
        app.refresh_sources();
        app.automation_window_id = app.windows.first().map(|window| window.id).unwrap_or(0);
        app.mouse_typing_window_id = app.windows.first().map(|window| window.id).unwrap_or(0);
        app
    }
}

impl AscApp {
    fn export_analysis(&self) -> Result<(), String> {
        if self.save_dir.is_empty() {
            return Ok(());
        }

        write_analysis(Path::new(&self.save_dir), &self.logs, &self.keyword_results)
    }

    fn refresh_sources(&mut self) {
        if let Ok(w) = capture::list_windows() {
            self.windows = w;
        }
        if let Ok(m) = capture::list_monitors() {
            self.monitors = m;
        }

        // Återställ vald källa om den inte längre existerar
        if self.source_type == "window" {
            if !self.windows.iter().any(|w| w.id == self.selected_source_id) {
                self.selected_source_id = self.windows.first().map(|w| w.id).unwrap_or(0);
            }
        } else {
            if !self
                .monitors
                .iter()
                .any(|m| m.id == self.selected_source_id)
            {
                self.selected_source_id = self.monitors.first().map(|m| m.id).unwrap_or(0);
            }
        }
    }

    fn clear_visual_regions(&mut self) {
        self.enable_crop = false;
        self.ocr_regions.clear();
        self.measurement_regions.clear();
        self.region_editor_active = false;
        self.region_drag_start = None;
        self.region_source_type.clear();
        self.region_source_id = None;
        self.region_source_size = None;
    }

    fn has_visual_regions(&self) -> bool {
        self.enable_crop || !self.ocr_regions.is_empty() || !self.measurement_regions.is_empty()
    }

    fn start_monitoring(&mut self, ctx: egui::Context) {
        let source_exists = if self.source_type == "window" {
            self.windows
                .iter()
                .any(|window| window.id == self.selected_source_id)
        } else {
            self.monitors
                .iter()
                .any(|monitor| monitor.id == self.selected_source_id)
        };
        if !source_exists {
            self.status_text = "Fel: Välj en tillgänglig källa först.".to_string();
            return;
        }
        if !self.save_dir.is_empty() && !Path::new(&self.save_dir).is_dir() {
            self.status_text = "Fel: Den valda målmappen finns inte.".to_string();
            return;
        }
        if self.enable_keyword_color && (!self.enable_ocr || self.keyword.trim().is_empty()) {
            self.status_text =
                "Fel: Aktivera OCR och ange ett sökord för ord-/färganalysen.".to_string();
            return;
        }

        self.logs.clear();
        self.diffs_history.clear();
        self.total_captures = 0;
        self.total_changes = 0;
        self.keyword_results.clear();
        self.export_pending = false;
        self.preview_texture = None;
        self.preview_size = None;
        self.latest_preview_texture = None;
        self.latest_preview_size = None;
        self.preview_zoom = 1.0;
        self.preview_pan = egui::Vec2::ZERO;
        self.follow_latest_preview = true;
        self.selected_preview_file = None;

        let (log_tx, log_rx) = channel();
        let (control_tx, control_rx) = channel();

        self.log_receiver = Some(log_rx);
        self.control_sender = Some(control_tx);
        self.is_running = true;
        self.status_text = "Övervakar...".to_string();

        let source_type = self.source_type.clone();
        let source_id = self.selected_source_id;
        let save_dir = self.save_dir.clone();
        let interval_secs = self.interval_secs;
        let threshold = self.threshold_pct / 100.0;
        let detect_small_changes = self.detect_small_changes;
        let small_change_min_pixels = self.small_change_min_pixels;
        let small_change_color_delta = self.small_change_color_delta;

        let crop_area = if self.enable_crop {
            Some((self.crop_x, self.crop_y, self.crop_w, self.crop_h))
        } else {
            None
        };

        let enable_ocr = self.enable_ocr;
        let ocr_area = if self.enable_ocr && self.enable_ocr_crop {
            Some((self.ocr_x, self.ocr_y, self.ocr_w, self.ocr_h))
        } else {
            None
        };
        let enable_keyword_color = self.enable_keyword_color;
        let keyword = self.keyword.trim().to_string();
        let keyword_delay_frames = self.keyword_delay_frames;
        let keyword_rising_edge_only = self.keyword_rising_edge_only;
        let indicator_area = (
            self.indicator_x,
            self.indicator_y,
            self.indicator_w,
            self.indicator_h,
        );
        let indicator_color_delta = self.indicator_color_delta;
        let indicator_min_pixels = self.indicator_min_pixels;
        let ocr_regions = self.ocr_regions.clone();
        let measurement_regions = self.measurement_regions.clone();

        std::thread::spawn(move || {
            let mut prev_img: Option<image::DynamicImage> = None;
            let mut prev_source_img: Option<image::DynamicImage> = None;
            let mut prev_ocr: Option<String> = None;
            let mut keyword_tracker = enable_keyword_color.then(|| {
                analysis::KeywordTracker::new(
                    &keyword,
                    keyword_delay_frames,
                    keyword_rising_edge_only,
                )
            });
            loop {
                // Kontrollera om vi ska stoppa tråden
                if let Ok(ControlMessage::Stop) = control_rx.try_recv() {
                    break;
                }

                // Ta skärmklipp
                match capture::capture_source(&source_type, source_id, None) {
                    Ok(source_img) => {
                        let img = if let Some(area) = crop_area {
                            capture::crop_image(&source_img, area)
                        } else {
                            source_img.clone()
                        };
                        let timestamp = Local::now().format("%H:%M:%S").to_string();
                        let file_timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();

                        // Spara fil om mapp angivits
                        let mut file_name = String::new();
                        if !save_dir.is_empty() {
                            let fname = format!("capture_{}.png", file_timestamp);
                            let fpath = Path::new(&save_dir).join(&fname);
                            if img.save(&fpath).is_ok() {
                                file_name = fname;
                            }
                        }

                        // Beräkna bildskillnad
                        let mut diff = 0.0;
                        let mut changed_pixels = 0;
                        if let Some(ref p_img) = prev_img {
                            let difference = if measurement_regions.is_empty() {
                                analysis::analyze_images(p_img, &img, small_change_color_delta)
                            } else {
                                analysis::analyze_regions(
                                    prev_source_img.as_ref().unwrap_or(p_img),
                                    &source_img,
                                    &measurement_regions,
                                    small_change_color_delta,
                                )
                            };
                            diff = difference.average;
                            changed_pixels = difference.changed_pixels;
                        }

                        // OCR (Textigenkänning)
                        let mut ocr_text = String::new();
                        if enable_ocr {
                            let ocr_images = if ocr_regions.is_empty() {
                                vec![if let Some(area) = ocr_area {
                                    capture::crop_image(&img, area)
                                } else {
                                    img.clone()
                                }]
                            } else {
                                ocr_regions
                                    .iter()
                                    .map(|&area| capture::crop_image(&source_img, area))
                                    .collect::<Vec<_>>()
                            };
                            let mut texts = Vec::new();
                            for (region_index, ocr_img) in ocr_images.into_iter().enumerate() {
                                let temp_path = std::env::temp_dir().join(format!(
                                    "asc_ocr_{}_{}_{}.png",
                                    std::process::id(),
                                    file_timestamp,
                                    region_index
                                ));
                                if ocr_img.save(&temp_path).is_ok() {
                                    match ocr::run_ocr(&temp_path.to_string_lossy()) {
                                        Ok(text) => texts.push(text),
                                        Err(error) => texts.push(format!("OCR-fel: {error}")),
                                    }
                                    let _ = fs::remove_file(temp_path);
                                }
                            }
                            ocr_text = texts.join("\n");
                        }

                        let ocr_changed = enable_ocr
                            && prev_ocr
                                .as_ref()
                                .is_some_and(|previous| ocr_text.trim() != previous.trim());
                        if let Some(tracker) = keyword_tracker.as_mut() {
                            let due_results = tracker.advance(&ocr_text);
                            if due_results > 0 {
                                let color = analysis::classify_indicator(
                                    &source_img,
                                    indicator_area,
                                    indicator_color_delta,
                                    indicator_min_pixels,
                                );
                                for _ in 0..due_results {
                                    let _ = log_tx.send(WorkerMessage::KeywordColor(
                                        KeywordColorResult {
                                            timestamp: timestamp.clone(),
                                            file_name: file_name.clone(),
                                            keyword: keyword.clone(),
                                            color,
                                            ocr_text: ocr_text.clone(),
                                        },
                                    ));
                                }
                            }
                        }
                        let is_changed = analysis::change_detected(
                            prev_img.is_some(),
                            diff,
                            threshold,
                            detect_small_changes && changed_pixels >= small_change_min_pixels,
                            ocr_changed,
                        );

                        prev_img = Some(img.clone());
                        prev_source_img = Some(source_img);
                        if enable_ocr {
                            prev_ocr = Some(ocr_text.clone());
                        }

                        // Skicka logg
                        let log_item = LogItem {
                            timestamp,
                            pixel_diff: diff,
                            changed_pixels,
                            ocr_text,
                            is_changed,
                            file_name,
                        };
                        let _ = log_tx.send(WorkerMessage::Log(log_item));

                        // Skicka förhandsvisningsbild (RGBA)
                        let rgba = img.to_rgba8();
                        let w = rgba.width() as usize;
                        let h = rgba.height() as usize;
                        let raw = rgba.into_raw();
                        let _ = log_tx.send(WorkerMessage::Preview(raw, w, h));

                        ctx.request_repaint();
                    }
                    Err(e) => {
                        let _ = log_tx.send(WorkerMessage::Error(format!("Klippfel: {}", e)));
                        ctx.request_repaint();
                    }
                }

                // Vänta inställt intervall (i mindre steg för att snabbare kunna avsluta tråden)
                for _ in 0..(interval_secs * 10) {
                    if let Ok(ControlMessage::Stop) = control_rx.try_recv() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
    }

    fn stop_monitoring(&mut self) {
        if let Some(ref sender) = self.control_sender {
            let _ = sender.send(ControlMessage::Stop);
        }
        self.is_running = false;
        self.status_text = "Avslutad".to_string();
        self.log_receiver = None;
        self.control_sender = None;
    }

    fn apply_live_settings(&mut self, ctx: egui::Context) {
        let logs = std::mem::take(&mut self.logs);
        let history = std::mem::take(&mut self.diffs_history);
        let keyword_results = std::mem::take(&mut self.keyword_results);
        let total_captures = self.total_captures;
        let total_changes = self.total_changes;

        if let Some(sender) = self.control_sender.as_ref() {
            let _ = sender.send(ControlMessage::Stop);
        }
        self.log_receiver = None;
        self.control_sender = None;
        self.is_running = false;
        self.start_monitoring(ctx);

        let restart_status = self.status_text.clone();
        self.logs = logs;
        self.diffs_history = history;
        self.keyword_results = keyword_results;
        self.total_captures = total_captures;
        self.total_changes = total_changes;
        if self.is_running {
            self.status_text = "Övervakar med uppdaterade inställningar…".to_string();
            self.export_pending = true;
        } else {
            self.status_text = restart_status;
        }
    }

    fn start_offline_analysis(&mut self, ctx: egui::Context) {
        if self.save_dir.is_empty() || !Path::new(&self.save_dir).is_dir() {
            self.status_text = "Fel: Välj en befintlig analysmapp först.".to_string();
            return;
        }
        if self.enable_keyword_color && (!self.enable_ocr || self.keyword.trim().is_empty()) {
            self.status_text =
                "Fel: Aktivera OCR och ange ett sökord för ord-/färganalysen.".to_string();
            return;
        }

        self.logs.clear();
        self.diffs_history.clear();
        self.total_captures = 0;
        self.total_changes = 0;
        self.keyword_results.clear();
        self.export_pending = false;
        self.preview_texture = None;
        self.preview_size = None;
        self.latest_preview_texture = None;
        self.latest_preview_size = None;
        self.preview_zoom = 1.0;
        self.preview_pan = egui::Vec2::ZERO;
        self.follow_latest_preview = true;
        self.selected_preview_file = None;

        let (log_tx, log_rx) = channel();
        self.log_receiver = Some(log_rx);
        self.is_running = true;
        self.status_text = "Kör offline-analys...".to_string();

        let save_dir = self.save_dir.clone();
        let threshold = self.threshold_pct / 100.0;
        let detect_small_changes = self.detect_small_changes;
        let small_change_min_pixels = self.small_change_min_pixels;
        let small_change_color_delta = self.small_change_color_delta;
        let crop_area = if self.enable_crop {
            Some((self.crop_x, self.crop_y, self.crop_w, self.crop_h))
        } else {
            None
        };
        let enable_ocr = self.enable_ocr;
        let ocr_area = if self.enable_ocr && self.enable_ocr_crop {
            Some((self.ocr_x, self.ocr_y, self.ocr_w, self.ocr_h))
        } else {
            None
        };
        let enable_keyword_color = self.enable_keyword_color;
        let keyword = self.keyword.trim().to_string();
        let keyword_delay_frames = self.keyword_delay_frames;
        let keyword_rising_edge_only = self.keyword_rising_edge_only;
        let indicator_area = (
            self.indicator_x,
            self.indicator_y,
            self.indicator_w,
            self.indicator_h,
        );
        let indicator_color_delta = self.indicator_color_delta;
        let indicator_min_pixels = self.indicator_min_pixels;
        let ocr_regions = self.ocr_regions.clone();
        let measurement_regions = self.measurement_regions.clone();

        std::thread::spawn(move || {
            let dir_path = Path::new(&save_dir);
            let mut entries = Vec::new();
            if let Ok(read_dir) = fs::read_dir(dir_path) {
                for entry in read_dir.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        let ext = path
                            .extension()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_lowercase();
                        if ext == "png" || ext == "jpg" || ext == "jpeg" {
                            entries.push(path);
                        }
                    }
                }
            }

            // Sortera efter filnamn (tidsordning)
            entries.sort();

            if entries.is_empty() {
                let _ = log_tx.send(WorkerMessage::OfflineDone(
                    "Inga bildfiler hittades i mappen.".to_string(),
                ));
                ctx.request_repaint();
                return;
            }

            let mut prev_img: Option<image::DynamicImage> = None;
            let mut prev_source_img: Option<image::DynamicImage> = None;
            let mut prev_ocr: Option<String> = None;
            let mut keyword_tracker = enable_keyword_color.then(|| {
                analysis::KeywordTracker::new(
                    &keyword,
                    keyword_delay_frames,
                    keyword_rising_edge_only,
                )
            });
            for (idx, path) in entries.iter().enumerate() {
                match image::open(path) {
                    Ok(original_img) => {
                        let source_img = original_img;
                        let img = if let Some(area) = crop_area {
                            capture::crop_image(&source_img, area)
                        } else {
                            source_img.clone()
                        };
                        let file_name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let timestamp = format!("#{}", idx + 1);

                        // Beräkna bildskillnad
                        let mut diff = 0.0;
                        let mut changed_pixels = 0;
                        if let Some(ref p_img) = prev_img {
                            let difference = if measurement_regions.is_empty() {
                                analysis::analyze_images(p_img, &img, small_change_color_delta)
                            } else {
                                analysis::analyze_regions(
                                    prev_source_img.as_ref().unwrap_or(p_img),
                                    &source_img,
                                    &measurement_regions,
                                    small_change_color_delta,
                                )
                            };
                            diff = difference.average;
                            changed_pixels = difference.changed_pixels;
                        }

                        // OCR (Textigenkänning)
                        let mut ocr_text = String::new();
                        if enable_ocr {
                            let ocr_images = if ocr_regions.is_empty() {
                                vec![if let Some(area) = ocr_area {
                                    capture::crop_image(&img, area)
                                } else {
                                    img.clone()
                                }]
                            } else {
                                ocr_regions
                                    .iter()
                                    .map(|&area| capture::crop_image(&source_img, area))
                                    .collect::<Vec<_>>()
                            };
                            let mut texts = Vec::new();
                            for (region_index, ocr_img) in ocr_images.into_iter().enumerate() {
                                let temp_path = std::env::temp_dir().join(format!(
                                    "asc_ocr_{}_{}_{}.png",
                                    std::process::id(),
                                    idx,
                                    region_index
                                ));
                                if ocr_img.save(&temp_path).is_ok() {
                                    match ocr::run_ocr(&temp_path.to_string_lossy()) {
                                        Ok(text) => texts.push(text),
                                        Err(error) => texts.push(format!("OCR-fel: {error}")),
                                    }
                                    let _ = fs::remove_file(temp_path);
                                }
                            }
                            ocr_text = texts.join("\n");
                        }

                        let ocr_changed = enable_ocr
                            && prev_ocr
                                .as_ref()
                                .is_some_and(|previous| ocr_text.trim() != previous.trim());
                        if let Some(tracker) = keyword_tracker.as_mut() {
                            let due_results = tracker.advance(&ocr_text);
                            if due_results > 0 {
                                let color = analysis::classify_indicator(
                                    &source_img,
                                    indicator_area,
                                    indicator_color_delta,
                                    indicator_min_pixels,
                                );
                                for _ in 0..due_results {
                                    let _ = log_tx.send(WorkerMessage::KeywordColor(
                                        KeywordColorResult {
                                            timestamp: timestamp.clone(),
                                            file_name: file_name.clone(),
                                            keyword: keyword.clone(),
                                            color,
                                            ocr_text: ocr_text.clone(),
                                        },
                                    ));
                                }
                            }
                        }
                        let is_changed = analysis::change_detected(
                            prev_img.is_some(),
                            diff,
                            threshold,
                            detect_small_changes && changed_pixels >= small_change_min_pixels,
                            ocr_changed,
                        );

                        prev_img = Some(img.clone());
                        prev_source_img = Some(source_img);
                        if enable_ocr {
                            prev_ocr = Some(ocr_text.clone());
                        }

                        // Skicka logg
                        let log_item = LogItem {
                            timestamp,
                            pixel_diff: diff,
                            changed_pixels,
                            ocr_text,
                            is_changed,
                            file_name,
                        };
                        let _ = log_tx.send(WorkerMessage::Log(log_item));

                        // Skicka förhandsvisningsbild (RGBA)
                        let rgba = img.to_rgba8();
                        let w = rgba.width() as usize;
                        let h = rgba.height() as usize;
                        let raw = rgba.into_raw();
                        let _ = log_tx.send(WorkerMessage::Preview(raw, w, h));

                        ctx.request_repaint();
                        std::thread::sleep(Duration::from_millis(50)); // Liten fördröjning för visuell återkoppling
                    }
                    Err(e) => {
                        let _ = log_tx.send(WorkerMessage::Error(format!(
                            "Kunde inte öppna {}: {}",
                            path.display(),
                            e
                        )));
                        ctx.request_repaint();
                    }
                }
            }

            let _ = log_tx.send(WorkerMessage::OfflineDone(format!(
                "Klart! Analyserade {} bilder.",
                entries.len()
            )));
            ctx.request_repaint();
        });
    }
}

impl AscApp {
    fn refresh_mouse_windows(&mut self) {
        self.refresh_sources();
        let available = self
            .windows
            .iter()
            .map(|window| window.id)
            .collect::<HashSet<_>>();
        self.mouse_selected_windows
            .retain(|window_id| available.contains(window_id));
        if !available.contains(&self.automation_window_id) {
            self.automation_window_id = self.windows.first().map(|window| window.id).unwrap_or(0);
        }
        if !available.contains(&self.mouse_typing_window_id) {
            self.mouse_typing_window_id = self.windows.first().map(|window| window.id).unwrap_or(0);
        }
    }

    fn start_mouse_simulation(&mut self, ctx: &egui::Context) {
        if self.automation_running {
            self.mouse_status = "Stoppa OCR-klicksekvensen först.".to_string();
            return;
        }
        if self.mouse_typing_enabled && !self.mouse_click_enabled {
            self.mouse_status = "Aktivera fönsterklick för att kunna skriva ord.".to_string();
            return;
        }
        if self.mouse_click_enabled && self.mouse_selected_windows.is_empty() {
            self.mouse_status = "Välj minst ett tillåtet fönster eller stäng av klick.".to_string();
            return;
        }
        let typing_words = self
            .mouse_typing_words
            .split(',')
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if self.mouse_typing_enabled && typing_words.is_empty() {
            self.mouse_status = "Ange minst ett ord för slumpmässig textinmatning.".to_string();
            return;
        }
        if self.mouse_typing_enabled
            && !self
                .windows
                .iter()
                .any(|window| window.id == self.mouse_typing_window_id)
        {
            self.mouse_status = "Välj ett tillgängligt skrivfönster.".to_string();
            return;
        }

        let config = mouse_sim::Config {
            interval_min: Duration::from_secs(self.mouse_interval_min.min(self.mouse_interval_max)),
            interval_max: Duration::from_secs(self.mouse_interval_min.max(self.mouse_interval_max)),
            pause_chance: self.mouse_pause_chance / 100.0,
            pause_min: Duration::from_secs(self.mouse_pause_min.min(self.mouse_pause_max)),
            pause_max: Duration::from_secs(self.mouse_pause_min.max(self.mouse_pause_max)),
            click_enabled: self.mouse_click_enabled,
            click_every: self.mouse_click_every.max(1),
            window_ids: self.mouse_selected_windows.iter().copied().collect(),
            typing_enabled: self.mouse_typing_enabled,
            typing_chance: self.mouse_typing_chance / 100.0,
            typing_words,
            typing_min_words: self.mouse_typing_min_words,
            typing_max_words: self.mouse_typing_max_words,
            typing_window_id: self
                .mouse_typing_enabled
                .then_some(self.mouse_typing_window_id),
            stop_after: self
                .mouse_stop_after_enabled
                .then(|| Duration::from_secs(self.mouse_stop_after_minutes.max(1) * 60)),
        };
        let (event_tx, event_rx) = channel();
        let (control_tx, control_rx) = channel();
        self.mouse_receiver = Some(event_rx);
        self.mouse_control_sender = Some(control_tx);
        self.mouse_running = true;
        self.mouse_moves = 0;
        self.mouse_clicks = 0;
        self.mouse_typed_words = 0;
        self.mouse_last_activity = "Väntar på första rörelsen…".to_string();
        self.mouse_status = "Musautomatisering körs.".to_string();
        std::thread::spawn(move || mouse_sim::run(config, control_rx, event_tx));
        ctx.request_repaint();
    }

    fn stop_mouse_simulation(&mut self) {
        if let Some(sender) = self.mouse_control_sender.as_ref() {
            let _ = sender.send(());
        }
        self.mouse_status = "Stoppar…".to_string();
    }

    fn start_automation(&mut self, ctx: &egui::Context) {
        if self.mouse_running {
            self.automation_status = "Stoppa muspekarsimuleringen först.".to_string();
            return;
        }
        if !self
            .windows
            .iter()
            .any(|window| window.id == self.automation_window_id)
        {
            self.automation_status = "Välj ett tillgängligt fönster först.".to_string();
            return;
        }

        let mut steps = Vec::new();
        for (index, step) in self.automation_steps.iter().enumerate() {
            match step.kind.as_str() {
                "wait" => steps.push(mouse_sim::AutomationStep::Wait(Duration::from_secs(
                    step.seconds,
                ))),
                "type_text" => {
                    if step.value.is_empty() {
                        self.automation_status =
                            format!("Steg {} saknar text att skriva.", index + 1);
                        return;
                    }
                    steps.push(mouse_sim::AutomationStep::TypeText(step.value.clone()));
                }
                "click_image" => {
                    if !Path::new(&step.value).is_file() {
                        self.automation_status =
                            format!("Steg {} saknar en giltig referensbild.", index + 1);
                        return;
                    }
                    steps.push(mouse_sim::AutomationStep::ClickImage {
                        image_path: step.value.clone(),
                        timeout: Duration::from_secs(step.seconds.max(1)),
                        confidence: step.confidence / 100.0,
                    });
                }
                _ => {
                    if step.value.trim().is_empty() {
                        self.automation_status = format!("Steg {} saknar OCR-ord.", index + 1);
                        return;
                    }
                    steps.push(mouse_sim::AutomationStep::ClickOcrWord {
                        word: step.value.trim().to_string(),
                        timeout: Duration::from_secs(step.seconds.max(1)),
                    });
                }
            }
        }
        if steps.is_empty() {
            self.automation_status = "Lägg till minst ett flödessteg.".to_string();
            return;
        }

        let config = mouse_sim::OcrSequenceConfig {
            window_id: self.automation_window_id,
            steps,
            repeat: self.automation_repeat,
        };
        let (event_tx, event_rx) = channel();
        let (control_tx, control_rx) = channel();
        self.automation_receiver = Some(event_rx);
        self.automation_control_sender = Some(control_tx);
        self.automation_running = true;
        self.automation_status = "OCR-klicksekvensen körs.".to_string();
        self.automation_last_activity = "Startar första steget…".to_string();
        std::thread::spawn(move || mouse_sim::run_ocr_sequence(config, control_rx, event_tx));
        ctx.request_repaint();
    }

    fn stop_automation(&mut self) {
        if let Some(sender) = self.automation_control_sender.as_ref() {
            let _ = sender.send(());
        }
        self.automation_status = "Stoppar…".to_string();
    }

    fn show_mouse_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Muspekar-simulering");
            ui.label(
                "Gör mjuka, slumpmässiga musrörelser. Analysen kan fortsätta samtidigt i den andra fliken.",
            );
            ui.add_space(8.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("Status:");
                ui.colored_label(
                    if self.mouse_running {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::LIGHT_GRAY
                    },
                    &self.mouse_status,
                );
            });
            ui.horizontal(|ui| {
                ui.label(format!("Rörelser: {}", self.mouse_moves));
                ui.separator();
                ui.label(format!("Klick: {}", self.mouse_clicks));
                ui.separator();
                ui.label(format!("Skrivna ord: {}", self.mouse_typed_words));
            });
            ui.small(format!("Senast: {}", self.mouse_last_activity));
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!self.mouse_running, |ui| {
                    ui.heading("Tidsinställningar");
                    egui::Grid::new("mouse_timing_grid")
                        .num_columns(3)
                        .spacing([10.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("Intervall mellan rörelser:");
                            ui.add(
                                egui::DragValue::new(&mut self.mouse_interval_min)
                                    .range(1..=3_600)
                                    .suffix(" sek min"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut self.mouse_interval_max)
                                    .range(1..=3_600)
                                    .suffix(" sek max"),
                            );
                            ui.end_row();
                            ui.label("Slumpmässig paus:");
                            ui.add(
                                egui::Slider::new(&mut self.mouse_pause_chance, 0.0..=100.0)
                                    .suffix(" % chans"),
                            );
                            ui.label("");
                            ui.end_row();
                            ui.label("Pauslängd:");
                            ui.add(
                                egui::DragValue::new(&mut self.mouse_pause_min)
                                    .range(1..=3_600)
                                    .suffix(" sek min"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut self.mouse_pause_max)
                                    .range(1..=3_600)
                                    .suffix(" sek max"),
                            );
                            ui.end_row();
                        });

                    ui.add_space(12.0);
                    ui.heading("Fönsterklick");
                    ui.checkbox(
                        &mut self.mouse_click_enabled,
                        "Klicka i särskilt valda fönster",
                    );
                    ui.small(
                        "Klick är avstängt som standard. ASC klickar aldrig i ett omarkerat fönster.",
                    );
                    if self.mouse_click_enabled {
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Klicka var");
                            ui.add(
                                egui::DragValue::new(&mut self.mouse_click_every)
                                    .range(1..=100)
                                    .suffix(":e rörelse"),
                            );
                            if ui.button("Uppdatera fönster").clicked() {
                                self.refresh_mouse_windows();
                            }
                            if ui.button("Markera alla").clicked() {
                                self.mouse_selected_windows
                                    .extend(self.windows.iter().map(|window| window.id));
                            }
                            if ui.button("Avmarkera alla").clicked() {
                                self.mouse_selected_windows.clear();
                            }
                        });
                        egui::ScrollArea::vertical()
                            .id_source("mouse_window_list")
                            .max_height(220.0)
                            .show(ui, |ui| {
                                if self.windows.is_empty() {
                                    ui.weak(
                                        "Inga öppna fönster hittades. Tryck Uppdatera fönster.",
                                    );
                                }
                                for window in &self.windows {
                                    let mut selected =
                                        self.mouse_selected_windows.contains(&window.id);
                                    if ui
                                        .checkbox(
                                            &mut selected,
                                            format!("{} — {}", window.app_name, window.title),
                                        )
                                        .changed()
                                    {
                                        if selected {
                                            self.mouse_selected_windows.insert(window.id);
                                        } else {
                                            self.mouse_selected_windows.remove(&window.id);
                                        }
                                    }
                                }
                            });

                        ui.add_space(8.0);
                        ui.separator();
                        ui.checkbox(
                            &mut self.mouse_typing_enabled,
                            "Skriv ibland slumpmässiga ord efter ett fönsterklick",
                        );
                        if self.mouse_typing_enabled {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                "Observera: detta ändrar innehållet i det valda programmet.",
                            );
                            ui.horizontal(|ui| {
                                ui.label("Skrivchans:");
                                ui.add(
                                    egui::Slider::new(
                                        &mut self.mouse_typing_chance,
                                        1.0..=100.0,
                                    )
                                    .suffix(" % av klicken"),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Antal ord:");
                                ui.add(
                                    egui::DragValue::new(&mut self.mouse_typing_min_words)
                                        .range(1..=20)
                                        .suffix(" min"),
                                );
                                ui.add(
                                    egui::DragValue::new(&mut self.mouse_typing_max_words)
                                        .range(1..=20)
                                        .suffix(" max"),
                                );
                            });
                            ui.label("Ordlista, separerad med kommatecken:");
                            ui.text_edit_singleline(&mut self.mouse_typing_words);
                            ui.horizontal(|ui| {
                                ui.label("Skriv endast i:");
                                egui::ComboBox::from_id_source("mouse_typing_window")
                                    .width(380.0)
                                    .selected_text(
                                        self.windows
                                            .iter()
                                            .find(|window| {
                                                window.id == self.mouse_typing_window_id
                                            })
                                            .map(|window| {
                                                format!("{} — {}", window.app_name, window.title)
                                            })
                                            .unwrap_or_else(|| "Välj fönster…".to_string()),
                                    )
                                    .show_ui(ui, |ui| {
                                        for window in &self.windows {
                                            ui.selectable_value(
                                                &mut self.mouse_typing_window_id,
                                                window.id,
                                                format!(
                                                    "{} — {}",
                                                    window.app_name, window.title
                                                ),
                                            );
                                        }
                                    });
                            });
                            ui.small(
                                "Vid skrivning klickar ASC endast i det valda skrivfönstrets innehållsyta.",
                            );
                        }
                    }

                    ui.add_space(12.0);
                    ui.heading("Säkerhet");
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut self.mouse_stop_after_enabled,
                            "Stoppa automatiskt efter",
                        );
                        ui.add_enabled(
                            self.mouse_stop_after_enabled,
                            egui::DragValue::new(&mut self.mouse_stop_after_minutes)
                                .range(1..=480)
                                .suffix(" minuter"),
                        );
                    });
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Nödstopp: flytta muspekaren till skärmens övre vänstra hörn.",
                    );
                });

                ui.add_space(16.0);
                if self.mouse_running {
                    if ui
                        .add_sized(
                            [230.0, 42.0],
                            egui::Button::new("Stoppa musautomatisering")
                                .fill(egui::Color32::from_rgb(180, 60, 60)),
                        )
                        .clicked()
                    {
                        self.stop_mouse_simulation();
                    }
                } else if ui
                    .add_sized(
                        [230.0, 42.0],
                        egui::Button::new("Starta musautomatisering")
                            .fill(egui::Color32::from_rgb(60, 160, 60)),
                    )
                    .clicked()
                {
                    self.start_mouse_simulation(ctx);
                }
            });
        });
    }

    fn show_automation_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("RPA-klicksekvens");
            ui.label(
                "Bygg ett flöde som väntar, hittar text eller bilder, klickar och skriver i ett valt fönster.",
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Status:");
                ui.colored_label(
                    if self.automation_running {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::LIGHT_GRAY
                    },
                    &self.automation_status,
                );
            });
            ui.small(format!("Senast: {}", self.automation_last_activity));
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!self.automation_running, |ui| {
                    ui.heading("Målfönster");
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_source("automation_window_combo")
                            .width(420.0)
                            .selected_text(
                                self.windows
                                    .iter()
                                    .find(|window| window.id == self.automation_window_id)
                                    .map(|window| {
                                        format!("{} — {}", window.app_name, window.title)
                                    })
                                    .unwrap_or_else(|| "Välj fönster…".to_string()),
                            )
                            .show_ui(ui, |ui| {
                                for window in &self.windows {
                                    ui.selectable_value(
                                        &mut self.automation_window_id,
                                        window.id,
                                        format!("{} — {}", window.app_name, window.title),
                                    );
                                }
                            });
                        if ui.button("Uppdatera fönster").clicked() {
                            self.refresh_mouse_windows();
                        }
                    });
                    ui.small(
                        "Alla OCR-klick begränsas till det valda fönstrets aktuella innehåll.",
                    );

                    ui.add_space(14.0);
                    ui.heading("Flödessteg");
                    let mut remove = None;
                    let mut move_up = None;
                    let mut move_down = None;
                    let mut pick_image = None;
                    let image_picker_running = self.automation_image_picker_receiver.is_some();
                    let step_count = self.automation_steps.len();
                    for (index, step) in self.automation_steps.iter_mut().enumerate() {
                        ui.group(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(format!("{}", index + 1));
                                egui::ComboBox::from_id_source(("automation_step", index))
                                    .selected_text(match step.kind.as_str() {
                                        "wait" => "Vänta",
                                        "type_text" => "Skriv text",
                                        "click_image" => "Klicka på bild",
                                        _ => "Klicka på OCR-ord",
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "click_word".to_string(),
                                            "Klicka på OCR-ord",
                                        );
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "click_image".to_string(),
                                            "Klicka på bild",
                                        );
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "wait".to_string(),
                                            "Vänta",
                                        );
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "type_text".to_string(),
                                            "Skriv text",
                                        );
                                    });
                                match step.kind.as_str() {
                                    "wait" => {
                                        ui.add(
                                            egui::DragValue::new(&mut step.seconds)
                                                .range(0..=3_600)
                                                .suffix(" sekunder"),
                                        );
                                    }
                                    "type_text" => {
                                        ui.label("Text:");
                                        ui.text_edit_singleline(&mut step.value);
                                    }
                                    "click_image" => {
                                        ui.label("Bild:");
                                        ui.text_edit_singleline(&mut step.value);
                                        if ui
                                            .add_enabled(
                                                !image_picker_running,
                                                egui::Button::new("Välj…"),
                                            )
                                            .clicked()
                                        {
                                            pick_image = Some(index);
                                        }
                                        ui.label("Likhet:");
                                        ui.add(
                                            egui::DragValue::new(&mut step.confidence)
                                                .range(50.0..=100.0)
                                                .suffix(" %"),
                                        );
                                        ui.label("Timeout:");
                                        ui.add(
                                            egui::DragValue::new(&mut step.seconds)
                                                .range(1..=600)
                                                .suffix(" sek"),
                                        );
                                    }
                                    _ => {
                                        ui.label("Ord:");
                                        ui.text_edit_singleline(&mut step.value);
                                        ui.label("Timeout:");
                                        ui.add(
                                            egui::DragValue::new(&mut step.seconds)
                                                .range(1..=600)
                                                .suffix(" sek"),
                                        );
                                    }
                                }
                                if ui.small_button("↑").clicked() && index > 0 {
                                    move_up = Some(index);
                                }
                                if ui.small_button("↓").clicked() && index + 1 < step_count {
                                    move_down = Some(index);
                                }
                                if ui.small_button("Ta bort").clicked() {
                                    remove = Some(index);
                                }
                            });
                        });
                    }
                    if let Some(index) = move_up {
                        self.automation_steps.swap(index, index - 1);
                    } else if let Some(index) = move_down {
                        self.automation_steps.swap(index, index + 1);
                    } else if let Some(index) = remove {
                        self.automation_steps.remove(index);
                    }
                    if let Some(index) = pick_image {
                        let (picker_tx, picker_rx) = channel();
                        self.automation_image_picker_receiver = Some(picker_rx);
                        let repaint_ctx = ctx.clone();
                        std::thread::spawn(move || {
                            let result = rfd::FileDialog::new()
                                .set_title("Välj referensbild")
                                .add_filter("Bild", &["png", "jpg", "jpeg"])
                                .pick_file()
                                .map(|path| (index, path.to_string_lossy().to_string()));
                            let _ = picker_tx.send(result);
                            repaint_ctx.request_repaint();
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui.button("+ OCR-klick").clicked() {
                            self.automation_steps.push(AutomationStepEditor::click_word());
                        }
                        if ui.button("+ Vänta").clicked() {
                            self.automation_steps.push(AutomationStepEditor::wait());
                        }
                        if ui.button("+ Bildklick").clicked() {
                            self.automation_steps.push(AutomationStepEditor {
                                kind: "click_image".to_string(),
                                value: String::new(),
                                seconds: 30,
                                confidence: 90.0,
                            });
                        }
                        if ui.button("+ Skriv text").clicked() {
                            self.automation_steps.push(AutomationStepEditor {
                            kind: "type_text".to_string(),
                            value: String::new(),
                            seconds: 0,
                            confidence: 90.0,
                            });
                        }
                    });
                    ui.small("Referensbilden bör vara ett tätt beskuret PNG-klipp av knappen eller ikonen i samma skala som i målfönstret.");
                    ui.checkbox(&mut self.automation_repeat, "Upprepa hela flödet");
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Kontrollera flödet utan Upprepa först. Klick och text kan ändra innehåll eller starta nedladdningar.",
                    );
                    ui.label("Nödstopp: flytta muspekaren till skärmens övre vänstra hörn.");
                });

                ui.add_space(16.0);
                if self.automation_running {
                    if ui
                        .add_sized(
                            [240.0, 42.0],
                            egui::Button::new("Stoppa klicksekvens")
                                .fill(egui::Color32::from_rgb(180, 60, 60)),
                        )
                        .clicked()
                    {
                        self.stop_automation();
                    }
                } else if ui
                    .add_sized(
                        [240.0, 42.0],
                        egui::Button::new("Kör klicksekvens")
                            .fill(egui::Color32::from_rgb(60, 160, 60)),
                    )
                    .clicked()
                {
                    self.start_automation(ctx);
                }
            });
        });
    }

    fn display_preview_image(
        &mut self,
        image: image::DynamicImage,
        title: String,
        ctx: &egui::Context,
    ) {
        let rgba = image.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        self.preview_texture = Some(ctx.load_texture(
            "preview_image",
            egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
            egui::TextureOptions::default(),
        ));
        self.preview_size = Some(size);
        self.preview_zoom = 1.0;
        self.preview_pan = egui::Vec2::ZERO;
        self.follow_latest_preview = false;
        self.selected_preview_file = Some(title);
    }

    fn preview_crop(&mut self, ctx: &egui::Context) {
        if self.crop_w == 0 || self.crop_h == 0 {
            self.status_text = "Bredd och höjd måste vara större än noll.".to_string();
            return;
        }

        let area = (self.crop_x, self.crop_y, self.crop_w, self.crop_h);
        let result = if self.mode == "live" {
            capture::capture_source(&self.source_type, self.selected_source_id, Some(area))
        } else {
            let directory = Path::new(&self.save_dir);
            let mut images = fs::read_dir(directory)
                .map_err(|error| format!("Kunde inte läsa analysmappen: {error}"))
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| {
                            path.extension().is_some_and(|extension| {
                                matches!(
                                    extension.to_string_lossy().to_ascii_lowercase().as_str(),
                                    "png" | "jpg" | "jpeg"
                                )
                            })
                        })
                        .collect::<Vec<_>>()
                });
            if let Ok(paths) = &mut images {
                paths.sort();
            }
            images.and_then(|paths| {
                let path = paths
                    .first()
                    .ok_or_else(|| "Inga bilder hittades i analysmappen.".to_string())?;
                image::open(path)
                    .map(|image| capture::crop_image(&image, area))
                    .map_err(|error| format!("Kunde inte öppna {}: {error}", path.display()))
            })
        };

        match result {
            Ok(image) => {
                self.display_preview_image(image, "Beskärningsprov".to_string(), ctx);
                self.status_text = "Visar beskärningsprov.".to_string();
            }
            Err(error) => self.status_text = format!("Förhandsvisningsfel: {error}"),
        }
    }

    fn start_region_editor(&mut self, ctx: &egui::Context) {
        let result = if self.mode == "live" {
            capture::capture_source(&self.source_type, self.selected_source_id, None)
        } else {
            let mut paths = match fs::read_dir(Path::new(&self.save_dir)) {
                Ok(entries) => entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.extension().is_some_and(|extension| {
                            matches!(
                                extension.to_string_lossy().to_ascii_lowercase().as_str(),
                                "png" | "jpg" | "jpeg"
                            )
                        })
                    })
                    .collect::<Vec<_>>(),
                Err(error) => {
                    self.status_text = format!("Kunde inte läsa analysmappen: {error}");
                    return;
                }
            };
            paths.sort();
            paths
                .first()
                .ok_or_else(|| "Inga bilder hittades i analysmappen.".to_string())
                .and_then(|path| {
                    image::open(path)
                        .map_err(|error| format!("Kunde inte öppna {}: {error}", path.display()))
                })
        };

        match result {
            Ok(image) => {
                let image_size = [image.width() as usize, image.height() as usize];
                let source_type = if self.mode == "live" {
                    self.source_type.clone()
                } else {
                    format!("offline:{}", self.save_dir)
                };
                let source_id = (self.mode == "live").then_some(self.selected_source_id);
                let cleared_stale_regions = self.has_visual_regions()
                    && (self.region_source_type != source_type
                        || self.region_source_id != source_id
                        || self.region_source_size != Some(image_size));
                if cleared_stale_regions {
                    self.clear_visual_regions();
                }
                self.region_source_type = source_type;
                self.region_source_id = source_id;
                self.region_source_size = Some(image_size);
                self.display_preview_image(image, "Områdesväljare".to_string(), ctx);
                self.region_editor_active = true;
                self.region_drag_start = None;
                self.status_text = if cleared_stale_regions {
                    "Källan eller storleken ändrades. Gamla områden rensades; markera nya områden."
                        .to_string()
                } else {
                    "Dra områden direkt i förhandsvisningen och tryck sedan Klar.".to_string()
                };
            }
            Err(error) => self.status_text = format!("Områdesväljarfel: {error}"),
        }
    }

    fn show_logged_capture(&mut self, log_index: usize, ctx: &egui::Context) {
        let Some(file_name) = self.logs.get(log_index).map(|log| log.file_name.clone()) else {
            return;
        };
        if file_name.is_empty() {
            self.status_text =
                "Klippet sparades inte. Välj en målmapp för att kunna öppna äldre klipp."
                    .to_string();
            return;
        }

        let path = Path::new(&self.save_dir).join(&file_name);
        match image::open(&path) {
            Ok(original_image) => {
                let image = if self.enable_crop && self.mode == "offline" {
                    capture::crop_image(
                        &original_image,
                        (self.crop_x, self.crop_y, self.crop_w, self.crop_h),
                    )
                } else {
                    original_image
                };
                self.display_preview_image(image, file_name, ctx);
            }
            Err(error) => {
                self.status_text = format!("Kunde inte öppna {}: {error}", path.display());
            }
        }
    }

    fn show_log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Händelselogg:");
            ui.weak("Klicka på tiden för att visa klippet");
            if ui.button("Rensa logg").clicked() {
                self.logs.clear();
                self.keyword_results.clear();
                self.diffs_history.clear();
                self.total_captures = 0;
                self.total_changes = 0;
                if let Err(error) = self.export_analysis() {
                    self.status_text = format!("Exportfel: {error}");
                }
            }
        });

        let spacing = ui.spacing().item_spacing.x;
        let timestamp_width = 68.0;
        let diff_width = 72.0;
        let pixels_width = 68.0;
        let status_width = 88.0;
        let ocr_width = (ui.available_width()
            - timestamp_width
            - diff_width
            - pixels_width
            - status_width
            - spacing * 4.0)
            .max(60.0);
        let widths = [
            timestamp_width,
            diff_width,
            pixels_width,
            ocr_width,
            status_width,
        ];

        ui.horizontal(|ui| {
            for (label, width) in ["Tid", "Diff %", "Ändr. px", "OCR-text", "Status"]
                .into_iter()
                .zip(widths)
            {
                ui.add_sized([width, 18.0], egui::Label::new(label).truncate());
            }
        });
        ui.separator();

        let row_height = ui.text_style_height(&egui::TextStyle::Body).max(18.0);
        let mut clicked_log = None;
        egui::ScrollArea::vertical().auto_shrink(false).show_rows(
            ui,
            row_height,
            self.logs.len(),
            |ui, visible_rows| {
                for row in visible_rows {
                    let log_index = self.logs.len() - 1 - row;
                    let log = &self.logs[log_index];
                    ui.horizontal(|ui| {
                        let selected = self.selected_preview_file.as_ref() == Some(&log.file_name)
                            && !log.file_name.is_empty();
                        if ui
                            .add_sized(
                                [widths[0], row_height],
                                egui::SelectableLabel::new(selected, &log.timestamp),
                            )
                            .on_hover_text("Visa detta skärmklipp")
                            .clicked()
                        {
                            clicked_log = Some(log_index);
                        }
                        ui.add_sized(
                            [widths[1], row_height],
                            egui::Label::new(format!("{:.5}%", log.pixel_diff * 100.0)).truncate(),
                        );
                        ui.add_sized(
                            [widths[2], row_height],
                            egui::Label::new(log.changed_pixels.to_string()).truncate(),
                        );
                        ui.add_sized(
                            [widths[3], row_height],
                            egui::Label::new(&log.ocr_text).truncate(),
                        )
                        .on_hover_text(&log.ocr_text);
                        let status = if log.is_changed {
                            egui::RichText::new("Ändrad").color(egui::Color32::GREEN)
                        } else {
                            egui::RichText::new("Ingen ändring")
                        };
                        ui.add_sized([widths[4], row_height], egui::Label::new(status).truncate());
                    });
                }
            },
        );
        if let Some(log_index) = clicked_log {
            self.show_logged_capture(log_index, ui.ctx());
        }
    }

    fn show_preview_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let title = self
                .selected_preview_file
                .as_deref()
                .map_or("Senaste skärmklipp", |file| file);
            ui.label(format!("Visar: {title}"));
            if ui
                .add_enabled(!self.follow_latest_preview, egui::Button::new("Senaste"))
                .on_hover_text("Återgå till det senaste skärmklippet")
                .clicked()
            {
                self.follow_latest_preview = true;
                self.selected_preview_file = None;
                if let Some(texture) = self.latest_preview_texture.as_ref() {
                    self.preview_texture = Some(texture.clone());
                    self.preview_size = self.latest_preview_size;
                }
                self.preview_zoom = 1.0;
                self.preview_pan = egui::Vec2::ZERO;
            }
            if ui
                .button("Anpassa")
                .on_hover_text("Visa hela bilden")
                .clicked()
            {
                self.preview_zoom = 1.0;
                self.preview_pan = egui::Vec2::ZERO;
            }
            ui.add(
                egui::Slider::new(&mut self.preview_zoom, 0.1..=8.0)
                    .logarithmic(true)
                    .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
            )
            .on_hover_text("Zoom relativt Anpassa-läget");
            if self.region_editor_active {
                ui.label("Rulla för zoom · dra för att markera · dubbelklicka för Anpassa");
            } else {
                ui.label("Rulla för zoom · dra för panorering · dubbelklicka för Anpassa");
            }
        });
        if self.region_editor_active {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Markera:");
                ui.selectable_value(
                    &mut self.region_editor_kind,
                    "screenshot".to_string(),
                    "Skärmklipp",
                );
                ui.selectable_value(
                    &mut self.region_editor_kind,
                    "ocr".to_string(),
                    "OCR-område",
                );
                ui.selectable_value(
                    &mut self.region_editor_kind,
                    "measurement".to_string(),
                    "Pixel/färg",
                );
                if ui.button("Ångra senaste").clicked() {
                    match self.region_editor_kind.as_str() {
                        "screenshot" => self.enable_crop = false,
                        "ocr" => {
                            self.ocr_regions.pop();
                        }
                        _ => {
                            self.measurement_regions.pop();
                        }
                    }
                }
                if ui.button("Rensa alla områden").clicked() {
                    self.enable_crop = false;
                    self.ocr_regions.clear();
                    self.measurement_regions.clear();
                }
                if ui.button("Klar").clicked() {
                    self.region_editor_active = false;
                    self.region_drag_start = None;
                    self.status_text = format!(
                        "Områden sparade: {} OCR, {} pixel/färg.",
                        self.ocr_regions.len(),
                        self.measurement_regions.len()
                    );
                }
            });
            ui.small(
                "Dra med vänster musknapp för att skapa området. Zooma med hjulet; använd Anpassa för helbild.",
            );
        }
        ui.separator();

        let canvas_size = egui::vec2(
            ui.available_width().max(1.0),
            ui.available_height().max(1.0),
        );
        let (rect, response) = ui.allocate_exact_size(canvas_size, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, egui::Color32::from_black_alpha(40));

        let (Some(texture_id), Some(size)) = (
            self.preview_texture.as_ref().map(|texture| texture.id()),
            self.preview_size,
        ) else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Ingen bild att visa",
                egui::FontId::proportional(12.0),
                egui::Color32::GRAY,
            );
            return;
        };

        if response.double_clicked() {
            self.preview_zoom = 1.0;
            self.preview_pan = egui::Vec2::ZERO;
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.raw_scroll_delta.y);
            if scroll != 0.0 {
                self.preview_zoom = (self.preview_zoom * (scroll * 0.0015).exp()).clamp(0.1, 8.0);
            }
        }
        if !self.region_editor_active && response.dragged_by(egui::PointerButton::Primary) {
            self.preview_pan += ui.input(|input| input.pointer.delta());
        }

        let image_size = egui::vec2(size[0] as f32, size[1] as f32);
        let fit_scale = (rect.width() / image_size.x)
            .min(rect.height() / image_size.y)
            .max(f32::EPSILON);
        let draw_size = image_size * fit_scale * self.preview_zoom;
        let max_pan = ((draw_size - rect.size()) * 0.5).max(egui::Vec2::ZERO);
        self.preview_pan.x = self.preview_pan.x.clamp(-max_pan.x, max_pan.x);
        self.preview_pan.y = self.preview_pan.y.clamp(-max_pan.y, max_pan.y);

        let image_rect = egui::Rect::from_center_size(rect.center() + self.preview_pan, draw_size);
        painter.image(
            texture_id,
            image_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        if self.region_editor_active {
            let pointer_to_image = |position: egui::Pos2| {
                egui::pos2(
                    ((position.x - image_rect.left()) / image_rect.width() * size[0] as f32)
                        .clamp(0.0, size[0] as f32),
                    ((position.y - image_rect.top()) / image_rect.height() * size[1] as f32)
                        .clamp(0.0, size[1] as f32),
                )
            };
            let region_to_screen = |region: (u32, u32, u32, u32)| {
                let left =
                    image_rect.left() + region.0 as f32 / size[0] as f32 * image_rect.width();
                let top = image_rect.top() + region.1 as f32 / size[1] as f32 * image_rect.height();
                let right = image_rect.left()
                    + (region.0 + region.2) as f32 / size[0] as f32 * image_rect.width();
                let bottom = image_rect.top()
                    + (region.1 + region.3) as f32 / size[1] as f32 * image_rect.height();
                egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
            };
            let draw_region = |region, color: egui::Color32, label: &str| {
                let screen_rect = region_to_screen(region);
                painter.rect_stroke(screen_rect, 0.0, egui::Stroke::new(2.0, color));
                painter.text(
                    screen_rect.left_top() + egui::vec2(3.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(11.0),
                    color,
                );
            };

            if self.enable_crop {
                draw_region(
                    (self.crop_x, self.crop_y, self.crop_w, self.crop_h),
                    egui::Color32::from_rgb(255, 90, 90),
                    "Skärmklipp",
                );
            }
            for (index, region) in self.ocr_regions.iter().copied().enumerate() {
                draw_region(
                    region,
                    egui::Color32::from_rgb(90, 170, 255),
                    &format!("OCR {}", index + 1),
                );
            }
            for (index, region) in self.measurement_regions.iter().copied().enumerate() {
                draw_region(
                    region,
                    egui::Color32::from_rgb(255, 210, 70),
                    &format!("Mät {}", index + 1),
                );
            }

            if response.drag_started_by(egui::PointerButton::Primary) {
                if let Some(position) = response.interact_pointer_pos() {
                    if image_rect.contains(position) {
                        self.region_drag_start = Some(pointer_to_image(position));
                    }
                }
            }
            if let (Some(start), Some(position)) =
                (self.region_drag_start, response.interact_pointer_pos())
            {
                let current = pointer_to_image(position);
                let preview_region = egui::Rect::from_two_pos(start, current);
                let color = match self.region_editor_kind.as_str() {
                    "screenshot" => egui::Color32::from_rgb(255, 90, 90),
                    "ocr" => egui::Color32::from_rgb(90, 170, 255),
                    _ => egui::Color32::from_rgb(255, 210, 70),
                };
                painter.rect_stroke(
                    region_to_screen((
                        preview_region.min.x.floor() as u32,
                        preview_region.min.y.floor() as u32,
                        preview_region.width().ceil() as u32,
                        preview_region.height().ceil() as u32,
                    )),
                    0.0,
                    egui::Stroke::new(2.0, color),
                );
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                if let (Some(start), Some(position)) = (
                    self.region_drag_start.take(),
                    response.interact_pointer_pos(),
                ) {
                    let selection = egui::Rect::from_two_pos(start, pointer_to_image(position));
                    let region = (
                        selection.min.x.floor().max(0.0) as u32,
                        selection.min.y.floor().max(0.0) as u32,
                        selection.width().ceil() as u32,
                        selection.height().ceil() as u32,
                    );
                    if region.2 > 0 && region.3 > 0 {
                        match self.region_editor_kind.as_str() {
                            "screenshot" => {
                                self.enable_crop = true;
                                self.crop_x = region.0;
                                self.crop_y = region.1;
                                self.crop_w = region.2;
                                self.crop_h = region.3;
                            }
                            "ocr" => {
                                self.enable_ocr = true;
                                self.ocr_regions.push(region);
                            }
                            _ => {
                                self.measurement_regions.push(region);
                                if self.measurement_regions.len() == 1 {
                                    self.indicator_x = region.0;
                                    self.indicator_y = region.1;
                                    self.indicator_w = region.2;
                                    self.indicator_h = region.3;
                                }
                            }
                        }
                    }
                }
            }
        }

        if response.hovered() {
            ui.ctx().set_cursor_icon(if self.region_editor_active {
                egui::CursorIcon::Crosshair
            } else if response.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
    }
}

impl eframe::App for AscApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let folder_result =
            self.folder_receiver
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(folder) => Some(folder),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                });
        if let Some(folder) = folder_result {
            self.folder_receiver = None;
            if let Some(folder) = folder {
                self.save_dir = folder;
                self.status_text = "Mapp vald.".to_string();
            } else {
                self.status_text = "Mappval avbrutet.".to_string();
            }
        }

        let image_picker_result =
            self.automation_image_picker_receiver
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                });
        if let Some(result) = image_picker_result {
            self.automation_image_picker_receiver = None;
            if let Some((index, path)) = result {
                if let Some(step) = self.automation_steps.get_mut(index) {
                    if step.kind == "click_image" {
                        step.value = path;
                        self.automation_status = "Referensbild vald.".to_string();
                    }
                }
            }
        }

        let mut mouse_stopped = None;
        if let Some(receiver) = self.mouse_receiver.as_ref() {
            while let Ok(event) = receiver.try_recv() {
                match event {
                    mouse_sim::Event::Activity {
                        description,
                        moves,
                        clicks,
                        typed_words,
                    } => {
                        self.mouse_last_activity = description;
                        self.mouse_moves = moves;
                        self.mouse_clicks = clicks;
                        self.mouse_typed_words = typed_words;
                        self.mouse_status = "Musautomatisering körs.".to_string();
                    }
                    mouse_sim::Event::Status(status) => self.mouse_status = status,
                    mouse_sim::Event::Stopped(reason) => mouse_stopped = Some(reason),
                }
            }
        }
        if let Some(reason) = mouse_stopped {
            self.mouse_running = false;
            self.mouse_status = reason;
            self.mouse_receiver = None;
            self.mouse_control_sender = None;
        } else if self.mouse_running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        let mut automation_stopped = None;
        if let Some(receiver) = self.automation_receiver.as_ref() {
            while let Ok(event) = receiver.try_recv() {
                match event {
                    mouse_sim::Event::Activity { description, .. } => {
                        self.automation_last_activity = description;
                        self.automation_status = "OCR-klicksekvensen körs.".to_string();
                    }
                    mouse_sim::Event::Status(status) => self.automation_status = status,
                    mouse_sim::Event::Stopped(reason) => automation_stopped = Some(reason),
                }
            }
        }
        if let Some(reason) = automation_stopped {
            self.automation_running = false;
            self.automation_status = reason;
            self.automation_receiver = None;
            self.automation_control_sender = None;
        } else if self.automation_running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        // Kontrollera om vi tagit emot meddelanden från bakgrundstråden
        let mut should_clear_receiver = false;
        let mut analysis_changed = false;
        if let Some(ref receiver) = self.log_receiver {
            while let Ok(msg) = receiver.try_recv() {
                match msg {
                    WorkerMessage::Log(item) => {
                        self.total_captures += 1;
                        if item.is_changed {
                            self.total_changes += 1;
                        }
                        self.diffs_history.push(item.pixel_diff);
                        self.logs.push(item);
                        analysis_changed = true;
                    }
                    WorkerMessage::Preview(rgba_bytes, w, h) => {
                        let color_image =
                            egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba_bytes);
                        let texture = ctx.load_texture(
                            "preview_image",
                            color_image,
                            egui::TextureOptions::default(),
                        );
                        self.latest_preview_texture = Some(texture.clone());
                        self.latest_preview_size = Some([w, h]);
                        if self.follow_latest_preview {
                            self.preview_texture = Some(texture);
                            self.preview_size = Some([w, h]);
                        }
                    }
                    WorkerMessage::Error(err) => {
                        self.status_text = format!("Fel: {}", err);
                    }
                    WorkerMessage::KeywordColor(result) => {
                        self.keyword_results.push(result);
                        analysis_changed = true;
                    }
                    WorkerMessage::OfflineDone(msg) => {
                        self.status_text = msg;
                        self.is_running = false;
                        should_clear_receiver = true;
                    }
                }
            }
        }
        if should_clear_receiver {
            self.log_receiver = None;
        }
        if analysis_changed {
            self.export_pending = true;
        }
        let export_interval = Duration::from_secs(1);
        let export_elapsed = self.last_export_at.elapsed();
        let export_due =
            self.export_pending && (should_clear_receiver || export_elapsed >= export_interval);
        if export_due {
            if let Err(error) = self.export_analysis() {
                self.status_text = format!("Exportfel: {error}");
            }
            self.export_pending = false;
            self.last_export_at = Instant::now();
        } else if self.export_pending {
            ctx.request_repaint_after(export_interval - export_elapsed);
        }

        egui::TopBottomPanel::top("main_tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(
                    &mut self.active_tab,
                    "analysis".to_string(),
                    if self.is_running {
                        "Skärmklipp & analys • körs"
                    } else {
                        "Skärmklipp & analys"
                    },
                );
                ui.selectable_value(
                    &mut self.active_tab,
                    "mouse".to_string(),
                    if self.mouse_running {
                        "Muspekar-simulering • körs"
                    } else {
                        "Muspekar-simulering"
                    },
                );
                ui.selectable_value(
                    &mut self.active_tab,
                    "automation".to_string(),
                    if self.automation_running {
                        "RPA-sekvens • körs"
                    } else {
                        "RPA-sekvens"
                    },
                );
            });
        });
        if self.active_tab == "mouse" {
            self.show_mouse_tab(ctx);
            return;
        }
        if self.active_tab == "automation" {
            self.show_automation_tab(ctx);
            return;
        }

        // Definiera det övergripande gränssnittet
        egui::SidePanel::left("left_panel")
            .resizable(true)
            .default_width(320.0)
            .min_width(260.0)
            .max_width(520.0)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("ASC - Skärmklipp & Analys");
                    ui.colored_label(
                        egui::Color32::from_rgb(100, 110, 240),
                        "Infödd Rust-version",
                    );
                });
                ui.separator();

                ui.add_enabled_ui(!self.is_running || self.mode == "live", |ui| {
                    // Lägeval
                    ui.label("Läge:");
                    ui.add_enabled_ui(!self.is_running, |ui| {
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.mode, "live".to_string(), "Live-övervakning");
                            ui.radio_value(
                                &mut self.mode,
                                "offline".to_string(),
                                "Efterhandsanalys",
                            );
                        });
                    });
                    ui.add_space(5.0);

                    // Källa
                    if self.mode == "live" {
                        let source_before = (self.source_type.clone(), self.selected_source_id);
                        ui.horizontal(|ui| {
                            ui.label("Källtyp:");
                            if ui
                                .radio_value(&mut self.source_type, "screen".to_string(), "Skärm")
                                .clicked()
                            {
                                self.refresh_sources();
                            }
                            if ui
                                .radio_value(&mut self.source_type, "window".to_string(), "Fönster")
                                .clicked()
                            {
                                self.refresh_sources();
                            }
                        });

                        ui.horizontal(|ui| {
                            ui.label("Källa:");
                            let combobox_width = 200.0;
                            if self.source_type == "window" {
                                egui::ComboBox::from_id_source("window_combo")
                                    .width(combobox_width)
                                    .selected_text(
                                        self.windows
                                            .iter()
                                            .find(|w| w.id == self.selected_source_id)
                                            .map(|w| format!("{} - {}", w.app_name, w.title))
                                            .unwrap_or_else(|| "Välj fönster...".to_string()),
                                    )
                                    .show_ui(ui, |ui| {
                                        for w in &self.windows {
                                            ui.selectable_value(
                                                &mut self.selected_source_id,
                                                w.id,
                                                format!("{} - {}", w.app_name, w.title),
                                            );
                                        }
                                    });
                            } else {
                                egui::ComboBox::from_id_source("screen_combo")
                                    .width(combobox_width)
                                    .selected_text(
                                        self.monitors
                                            .iter()
                                            .find(|m| m.id == self.selected_source_id)
                                            .map(|m| m.name.clone())
                                            .unwrap_or_else(|| "Välj skärm...".to_string()),
                                    )
                                    .show_ui(ui, |ui| {
                                        for m in &self.monitors {
                                            ui.selectable_value(
                                                &mut self.selected_source_id,
                                                m.id,
                                                m.name.clone(),
                                            );
                                        }
                                    });
                            }

                            if ui
                                .button("🔄")
                                .on_hover_text("Uppdatera fönsterlista")
                                .clicked()
                            {
                                self.refresh_sources();
                            }
                        });
                        if source_before != (self.source_type.clone(), self.selected_source_id)
                            && self.has_visual_regions()
                        {
                            self.clear_visual_regions();
                            self.status_text = "Källan ändrades. Gamla visuella områden rensades så att koordinaterna inte återanvänds på fel fönster.".to_string();
                        }
                        ui.add_space(5.0);
                    }

                    // Spara i mapp (RFD Dialog)
                    ui.label(if self.mode == "live" {
                        "Spara skärmklipp och analys i mapp (valfritt):"
                    } else {
                        "Analysmapp (Bilder):"
                    });
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(&mut self.save_dir);
                        let picker_running = self.folder_receiver.is_some();
                        if ui
                            .add_enabled(
                                !picker_running,
                                egui::Button::new(if picker_running {
                                    "Öppnar…"
                                } else {
                                    "Bläddra..."
                                }),
                            )
                            .clicked()
                        {
                            let (folder_tx, folder_rx) = channel();
                            self.folder_receiver = Some(folder_rx);
                            self.status_text = "Öppnar mappväljaren…".to_string();
                            let repaint_ctx = ctx.clone();
                            std::thread::spawn(move || {
                                let folder = rfd::FileDialog::new()
                                    .set_title("Välj mapp")
                                    .pick_folder()
                                    .map(|path| path.to_string_lossy().to_string());
                                let _ = folder_tx.send(folder);
                                repaint_ctx.request_repaint();
                            });
                        }
                    });
                    if !self.save_dir.is_empty() {
                        ui.small("Analysen sparas som asc-analysis.csv och asc-analysis.json");
                    }
                    ui.add_space(5.0);

                    // Parametrar
                    if self.mode == "live" {
                        ui.horizontal(|ui| {
                            ui.label("Intervall:");
                            ui.add(egui::Slider::new(&mut self.interval_secs, 1..=120).text("sek"));
                        });
                    }

                    ui.horizontal(|ui| {
                        ui.label("Jämförelse-tröskel:");
                        ui.add(
                            egui::Slider::new(&mut self.threshold_pct, 0.0001..=10.0)
                                .logarithmic(true)
                                .custom_formatter(|value, _| format!("{value:.5} %")),
                        );
                    });
                    ui.checkbox(
                        &mut self.detect_small_changes,
                        "Upptäck små lokala färgförändringar",
                    );
                    if self.detect_small_changes {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("Minst");
                                ui.add(
                                    egui::DragValue::new(&mut self.small_change_min_pixels)
                                        .range(1..=100_000)
                                        .suffix(" ändrade pixlar"),
                                );
                            });
                            ui.horizontal(|ui| {
                                ui.label("Minsta färgskillnad per pixel:");
                                ui.add(
                                    egui::DragValue::new(&mut self.small_change_color_delta)
                                        .range(1..=255),
                                );
                            });
                            ui.small(
                                "För en cirka 5 px bred färgindikator: prova 5 pixlar och färgskillnad 24.",
                            );
                        });
                    }
                    ui.add_space(8.0);

                    // Beskärning
                    if ui.button("Välj områden visuellt...").clicked() {
                        self.start_region_editor(ctx);
                    }
                    if !self.ocr_regions.is_empty() || !self.measurement_regions.is_empty() {
                        ui.small(format!(
                            "Visuellt valda områden: {} OCR, {} pixel/färg",
                            self.ocr_regions.len(),
                            self.measurement_regions.len()
                        ));
                    }
                    ui.checkbox(&mut self.enable_crop, "Beskär skärmklipp");
                    if self.enable_crop {
                        ui.group(|ui| {
                            ui.horizontal(|ui| {
                                ui.label("X:");
                                ui.add(egui::DragValue::new(&mut self.crop_x));
                                ui.label("Y:");
                                ui.add(egui::DragValue::new(&mut self.crop_y));
                            });
                            ui.horizontal(|ui| {
                                ui.label("Bredd:");
                                ui.add(egui::DragValue::new(&mut self.crop_w));
                                ui.label("Höjd:");
                                ui.add(egui::DragValue::new(&mut self.crop_h));
                            });
                            if ui.button("Förhandsgranska beskärning").clicked() {
                                self.preview_crop(ctx);
                            }
                        });
                    }
                    ui.add_space(5.0);

                    // OCR
                    ui.checkbox(&mut self.enable_ocr, "Aktivera textigenkänning (OCR)");
                    if self.enable_ocr {
                        ui.group(|ui| {
                            ui.checkbox(&mut self.enable_ocr_crop, "Beskär OCR-område");
                            if !self.ocr_regions.is_empty() {
                                ui.label(format!(
                                    "{} visuellt valda OCR-områden används.",
                                    self.ocr_regions.len()
                                ));
                            }
                            if self.enable_ocr_crop {
                                ui.horizontal(|ui| {
                                    ui.label("X:");
                                    ui.add(egui::DragValue::new(&mut self.ocr_x));
                                    ui.label("Y:");
                                    ui.add(egui::DragValue::new(&mut self.ocr_y));
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Bredd:");
                                    ui.add(egui::DragValue::new(&mut self.ocr_w));
                                    ui.label("Höjd:");
                                    ui.add(egui::DragValue::new(&mut self.ocr_h));
                                });
                            }
                        });

                        ui.add_space(5.0);
                        ui.checkbox(
                            &mut self.enable_keyword_color,
                            "Koppla OCR-sökord till grön/röd indikator",
                        );
                        if self.enable_keyword_color {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label("Sökord:");
                                    ui.text_edit_singleline(&mut self.keyword);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Läs färgen efter:");
                                    ui.add(
                                        egui::DragValue::new(&mut self.keyword_delay_frames)
                                            .range(0..=1_000)
                                            .suffix(" skärmklipp"),
                                    );
                                });
                                ui.checkbox(
                                    &mut self.keyword_rising_edge_only,
                                    "Räkna bara när ordet nyss dyker upp",
                                );
                                ui.label("Indikatorområde (eller första gula mätområdet):");
                                ui.horizontal(|ui| {
                                    ui.label("X:");
                                    ui.add(egui::DragValue::new(&mut self.indicator_x));
                                    ui.label("Y:");
                                    ui.add(egui::DragValue::new(&mut self.indicator_y));
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Bredd:");
                                    ui.add(egui::DragValue::new(&mut self.indicator_w));
                                    ui.label("Höjd:");
                                    ui.add(egui::DragValue::new(&mut self.indicator_h));
                                });
                                ui.horizontal(|ui| {
                                    ui.label("Färgdominans:");
                                    ui.add(
                                        egui::DragValue::new(&mut self.indicator_color_delta)
                                            .range(1..=255),
                                    );
                                    ui.label("Min pixlar:");
                                    ui.add(
                                        egui::DragValue::new(&mut self.indicator_min_pixels)
                                            .range(1..=100_000),
                                    );
                                });
                            });
                        }
                    }
                });

                ui.add_space(15.0);
                ui.separator();
                ui.add_space(5.0);

                // Start/Stopp-knapp
                if !self.is_running {
                    let btn_text = if self.mode == "live" {
                        "Starta övervakning"
                    } else {
                        "Starta offline-analys"
                    };
                    if ui
                        .add_sized(
                            [ui.available_width(), 40.0],
                            egui::Button::new(btn_text).fill(egui::Color32::from_rgb(60, 160, 60)),
                        )
                        .clicked()
                    {
                        if self.mode == "live" {
                            self.start_monitoring(ctx.clone());
                        } else {
                            self.start_offline_analysis(ctx.clone());
                        }
                    }
                } else {
                    let btn_text = if self.mode == "live" {
                        "Stoppa övervakning"
                    } else {
                        "Stoppar..."
                    };
                    if ui
                        .add_sized(
                            [ui.available_width(), 40.0],
                            egui::Button::new(btn_text).fill(egui::Color32::from_rgb(180, 60, 60)),
                        )
                        .clicked()
                        && self.mode == "live"
                    {
                        self.stop_monitoring();
                    }
                    if self.mode == "live"
                        && ui
                            .add_sized(
                                [ui.available_width(), 34.0],
                                egui::Button::new("Tillämpa ändringar under körning")
                                    .fill(egui::Color32::from_rgb(70, 110, 180)),
                            )
                            .clicked()
                    {
                        self.apply_live_settings(ctx.clone());
                    }
                }

                // Visa status
                ui.add_space(15.0);
                ui.horizontal(|ui| {
                    ui.label("Status:");
                    ui.colored_label(
                        if self.is_running {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::LIGHT_GRAY
                        },
                        &self.status_text,
                    );
                });
            });

        // Huvudyta
        egui::CentralPanel::default().show(ctx, |ui| {
            // Statistikpanel
            ui.horizontal(|ui| {
                ui.columns(3, |columns| {
                    columns[0].vertical(|ui| {
                        ui.label("Skärmklipp:");
                        ui.heading(self.total_captures.to_string());
                    });
                    columns[1].vertical(|ui| {
                        ui.label("Förändringar:");
                        ui.heading(self.total_changes.to_string());
                    });
                    columns[2].vertical(|ui| {
                        ui.label("Genomsnittlig diff:");
                        let avg = if self.diffs_history.is_empty() {
                            0.0
                        } else {
                            (self.diffs_history.iter().sum::<f64>()
                                / self.diffs_history.len() as f64)
                                * 100.0
                        };
                        ui.heading(format!("{:.5}%", avg));
                    });
                });
            });
            if self.enable_keyword_color || !self.keyword_results.is_empty() {
                let green = self
                    .keyword_results
                    .iter()
                    .filter(|result| result.color == analysis::IndicatorColor::Green)
                    .count();
                let red = self
                    .keyword_results
                    .iter()
                    .filter(|result| result.color == analysis::IndicatorColor::Red)
                    .count();
                let gray = self.keyword_results.len().saturating_sub(green + red);
                ui.horizontal_wrapped(|ui| {
                    ui.strong(format!("OCR-ord ‘{}’:", self.keyword));
                    ui.colored_label(egui::Color32::GREEN, format!("Grön {green}"));
                    ui.colored_label(egui::Color32::RED, format!("Röd {red}"));
                    ui.label(format!("Grå/okänd {gray}"));
                    if let Some(latest) = self.keyword_results.last() {
                        ui.label(format!("Senast: {}", latest.color.label()));
                    }
                });
            }
            ui.separator();

            // Skillnadsgraf (Egengjord linjegraf ritad via Painter)
            ui.label("Skillnad över tid (procent):");
            let graph_height = 140.0;
            let (rect, _response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), graph_height),
                egui::Sense::hover(),
            );
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, egui::Color32::from_black_alpha(200));

            // Rita rutnät och linje
            let pad_left = 35.0;
            let pad_right = 10.0;
            let pad_top = 10.0;
            let pad_bottom = 15.0;
            let draw_w = rect.width() - pad_left - pad_right;
            let draw_h = rect.height() - pad_top - pad_bottom;

            // Beräkna max y
            let mut max_y_pct = 5.0;
            for &val in &self.diffs_history {
                if val * 100.0 > max_y_pct {
                    max_y_pct = val * 100.0 * 1.15;
                }
            }

            // Rita rutnät (linjer och etiketter)
            for i in 0..=4 {
                let pct = (max_y_pct / 4.0) * i as f64;
                let y = rect.bottom() - pad_bottom - (draw_h * (pct / max_y_pct) as f32);
                painter.line_segment(
                    [
                        egui::pos2(rect.left() + pad_left, y),
                        egui::pos2(rect.right() - pad_right, y),
                    ],
                    egui::Stroke::new(1.0, egui::Color32::from_white_alpha(15)),
                );
                painter.text(
                    egui::pos2(rect.left() + pad_left - 5.0, y),
                    egui::Align2::RIGHT_CENTER,
                    format!("{:.1}%", pct),
                    egui::FontId::proportional(9.0),
                    egui::Color32::GRAY,
                );
            }

            // Rita datapunkter
            if self.diffs_history.len() >= 2 {
                let history_slice = if self.diffs_history.len() > 50 {
                    &self.diffs_history[self.diffs_history.len() - 50..]
                } else {
                    &self.diffs_history[..]
                };

                let points: Vec<egui::Pos2> = history_slice
                    .iter()
                    .enumerate()
                    .map(|(idx, &val)| {
                        let x = rect.left()
                            + pad_left
                            + (draw_w / (history_slice.len() - 1) as f32) * idx as f32;
                        let y = rect.bottom()
                            - pad_bottom
                            - (draw_h * ((val * 100.0) / max_y_pct) as f32);
                        egui::pos2(x, y)
                    })
                    .collect();

                for i in 0..points.len() - 1 {
                    painter.line_segment(
                        [points[i], points[i + 1]],
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 110, 240)),
                    );
                }
            } else {
                painter.text(
                    egui::pos2(rect.center().x, rect.center().y),
                    egui::Align2::CENTER_CENTER,
                    "Väntar på skärmklippsdata...",
                    egui::FontId::proportional(12.0),
                    egui::Color32::GRAY,
                );
            }

            ui.add_space(10.0);

            egui::SidePanel::left("log_panel")
                .resizable(true)
                .default_width(420.0)
                .min_width(260.0)
                .max_width(720.0)
                .show_inside(ui, |ui| self.show_log_panel(ui));
            egui::CentralPanel::default().show_inside(ui, |ui| self.show_preview_panel(ui));
        });
    }
}

fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn write_analysis(
    directory: &Path,
    logs: &[LogItem],
    keyword_results: &[KeywordColorResult],
) -> Result<(), String> {
    if !directory.is_dir() {
        return Err("analysmappen finns inte".to_string());
    }

    let json = serde_json::to_vec_pretty(logs)
        .map_err(|error| format!("kunde inte skapa JSON: {error}"))?;
    fs::write(directory.join("asc-analysis.json"), json)
        .map_err(|error| format!("kunde inte skriva asc-analysis.json: {error}"))?;

    let mut csv =
        String::from("timestamp,file_name,pixel_diff_percent,changed_pixels,ocr_text,is_changed\n");
    for item in logs {
        let _ = writeln!(
            csv,
            "{},{},{:.6},{},{},{}",
            csv_field(&item.timestamp),
            csv_field(&item.file_name),
            item.pixel_diff * 100.0,
            item.changed_pixels,
            csv_field(&item.ocr_text),
            item.is_changed
        );
    }
    fs::write(directory.join("asc-analysis.csv"), csv)
        .map_err(|error| format!("kunde inte skriva asc-analysis.csv: {error}"))?;

    let keyword_json = serde_json::to_vec_pretty(keyword_results)
        .map_err(|error| format!("kunde inte skapa ord-/färg-JSON: {error}"))?;
    fs::write(directory.join("asc-keyword-colors.json"), keyword_json)
        .map_err(|error| format!("kunde inte skriva asc-keyword-colors.json: {error}"))?;

    let mut keyword_csv = String::from("timestamp,file_name,keyword,color,ocr_text\n");
    for result in keyword_results {
        let _ = writeln!(
            keyword_csv,
            "{},{},{},{},{}",
            csv_field(&result.timestamp),
            csv_field(&result.file_name),
            csv_field(&result.keyword),
            result.color.label(),
            csv_field(&result.ocr_text),
        );
    }
    fs::write(directory.join("asc-keyword-colors.csv"), keyword_csv)
        .map_err(|error| format!("kunde inte skriva asc-keyword-colors.csv: {error}"))?;

    Ok(())
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ASC - Skärmklipp & Analys")
            .with_inner_size([1024.0, 768.0]),
        ..Default::default()
    };

    eframe::run_native(
        "asc-screen-analyzer",
        options,
        Box::new(|_cc| Ok(Box::new(AscApp::default()))),
    )
}

#[cfg(test)]
mod export_tests {
    use super::{csv_field, write_analysis, KeywordColorResult, LogItem};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn csv_fields_escape_quotes() {
        assert_eq!(csv_field("text, \"citat\""), "\"text, \"\"citat\"\"\"");
    }

    #[test]
    fn analysis_is_written_as_csv_and_json() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("systemklockan ska vara efter Unix-epoken")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("asc-export-test-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).expect("testmappen ska kunna skapas");
        let logs = vec![LogItem {
            timestamp: "12:34:56".to_string(),
            pixel_diff: 0.0125,
            changed_pixels: 42,
            ocr_text: "Rad 1, \"test\"".to_string(),
            is_changed: true,
            file_name: "capture.png".to_string(),
        }];

        let keyword_results = vec![KeywordColorResult {
            timestamp: "12:34:57".to_string(),
            file_name: "capture.png".to_string(),
            keyword: "öppet".to_string(),
            color: crate::analysis::IndicatorColor::Green,
            ocr_text: "Status öppet".to_string(),
        }];

        write_analysis(&directory, &logs, &keyword_results).expect("analysen ska kunna exporteras");

        let csv =
            fs::read_to_string(directory.join("asc-analysis.csv")).expect("CSV-filen ska finnas");
        let json =
            fs::read_to_string(directory.join("asc-analysis.json")).expect("JSON-filen ska finnas");
        assert!(csv.contains("1.250000"));
        assert!(csv.contains(",42,"));
        assert!(csv.contains("\"Rad 1, \"\"test\"\"\""));
        assert!(json.contains("\"is_changed\": true"));
        let keyword_csv = fs::read_to_string(directory.join("asc-keyword-colors.csv"))
            .expect("ord-/färg-CSV ska finnas");
        assert!(keyword_csv.contains("\"öppet\",grön"));

        fs::remove_dir_all(directory).expect("testmappen ska kunna tas bort");
    }
}
