#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod analysis;
mod capture;
mod macro_recorder;
mod mouse_sim;
mod ocr;
mod presence;

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
    change_bounds: Option<analysis::ChangeBounds>,
    change_summary: String,
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

#[derive(Clone, serde::Serialize)]
struct PresencePeriod {
    person: String,
    status: presence::PresenceStatus,
    started_at: String,
    ended_at: Option<String>,
    duration_seconds: u64,
}

enum ControlMessage {
    Stop,
}

fn stop_requested(control_rx: &Receiver<ControlMessage>) -> bool {
    matches!(control_rx.try_recv(), Ok(ControlMessage::Stop))
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
    target_window_id: u32,
    seconds: u64,
    timeout_seconds: u64,
    confidence: f64,
    template_image: Option<image::DynamicImage>,
    expected_area: Option<(u32, u32, u32, u32)>,
    reference_size: Option<(u32, u32)>,
    relative_x: f64,
    relative_y: f64,
    click_button: String,
    delay_ms: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RegionKind {
    Screenshot,
    Ocr,
    Measurement,
}

#[derive(Clone, Copy)]
enum ResizeCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Copy)]
struct RegionInteraction {
    kind: RegionKind,
    index: usize,
    original: (u32, u32, u32, u32),
    start: egui::Pos2,
    resize: Option<ResizeCorner>,
}

#[derive(Clone)]
struct VisualRegionBackup {
    enable_crop: bool,
    crop: (u32, u32, u32, u32),
    enable_ocr: bool,
    ocr_regions: Vec<(u32, u32, u32, u32)>,
    measurement_regions: Vec<(u32, u32, u32, u32)>,
    indicator: (u32, u32, u32, u32),
    source_type: String,
    source_id: Option<u32>,
    source_size: Option<[usize; 2]>,
}

impl AutomationStepEditor {
    fn basic(kind: &str, value: &str) -> Self {
        Self {
            kind: kind.to_string(),
            value: value.to_string(),
            target_window_id: 0,
            seconds: 0,
            timeout_seconds: 60,
            confidence: 90.0,
            template_image: None,
            expected_area: None,
            reference_size: None,
            relative_x: 0.5,
            relative_y: 0.5,
            click_button: "left".to_string(),
            delay_ms: 0,
        }
    }

    fn click_word() -> Self {
        Self {
            kind: "click_word".to_string(),
            value: String::new(),
            target_window_id: 0,
            seconds: 30,
            timeout_seconds: 60,
            confidence: 90.0,
            template_image: None,
            expected_area: None,
            reference_size: None,
            relative_x: 0.5,
            relative_y: 0.5,
            click_button: "left".to_string(),
            delay_ms: 0,
        }
    }

    fn wait() -> Self {
        Self {
            kind: "wait".to_string(),
            value: String::new(),
            target_window_id: 0,
            seconds: 3,
            timeout_seconds: 60,
            confidence: 90.0,
            template_image: None,
            expected_area: None,
            reference_size: None,
            relative_x: 0.5,
            relative_y: 0.5,
            click_button: "left".to_string(),
            delay_ms: 0,
        }
    }

    fn image_target(label: &str) -> Self {
        let mut step = Self::basic("click_image", label);
        step.seconds = 30;
        step.confidence = 85.0;
        step
    }

    fn wait_ready(minimum_seconds: u64) -> Self {
        let mut step = Self::basic("wait_ready", "");
        step.seconds = minimum_seconds;
        step
    }

    fn shortcut(value: &str) -> Self {
        Self::basic("shortcut", value)
    }

    fn switch_window(window_id: u32) -> Self {
        let mut step = Self::basic("switch_window", "");
        step.target_window_id = window_id;
        step
    }

    fn recorded_wait(delay_ms: u64) -> Self {
        let mut step = Self::basic("wait_recorded", "");
        step.delay_ms = delay_ms;
        step
    }

    fn recorded_click(x: f32, y: f32, button: macro_recorder::RecordedMouseButton) -> Self {
        let mut step = Self::basic("click_relative", "");
        step.relative_x = f64::from(x);
        step.relative_y = f64::from(y);
        step.click_button = match button {
            macro_recorder::RecordedMouseButton::Left => "left",
            macro_recorder::RecordedMouseButton::Right => "right",
        }
        .to_string();
        step
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
    region_selected: Option<(RegionKind, usize)>,
    region_interaction: Option<RegionInteraction>,
    region_result_preview: bool,
    region_source_image: Option<image::DynamicImage>,
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
    preview_change_bounds: Option<analysis::ChangeBounds>,

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
    automation_dry_run: bool,
    automation_receiver: Option<Receiver<mouse_sim::Event>>,
    automation_control_sender: Option<Sender<()>>,
    automation_recorder: Option<macro_recorder::MacroRecorder>,
    automation_picker_step: Option<usize>,
    automation_picker_image: Option<image::DynamicImage>,
    automation_picker_texture: Option<egui::TextureHandle>,
    automation_picker_size: Option<[usize; 2]>,
    automation_picker_drag_start: Option<egui::Pos2>,
    automation_picker_selection: Option<(u32, u32, u32, u32)>,

    // Teams-status
    presence_running: bool,
    presence_status_text: String,
    presence_window_id: u32,
    presence_person: String,
    presence_interval_secs: u64,
    presence_ocr_area: Option<(u32, u32, u32, u32)>,
    presence_color_area: Option<(u32, u32, u32, u32)>,
    presence_current_status: Option<presence::PresenceStatus>,
    presence_current_started: Option<Instant>,
    presence_periods: Vec<PresencePeriod>,
    presence_last_ocr: String,
    presence_receiver: Option<Receiver<presence::Event>>,
    presence_control_sender: Option<Sender<()>>,
    presence_region_setup_pending: bool,
    presence_region_backup: Option<VisualRegionBackup>,
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
            region_selected: None,
            region_interaction: None,
            region_result_preview: false,
            region_source_image: None,
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
            preview_change_bounds: None,
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
            automation_dry_run: true,
            automation_receiver: None,
            automation_control_sender: None,
            automation_recorder: None,
            automation_picker_step: None,
            automation_picker_image: None,
            automation_picker_texture: None,
            automation_picker_size: None,
            automation_picker_drag_start: None,
            automation_picker_selection: None,
            presence_running: false,
            presence_status_text: "Klar att starta".to_string(),
            presence_window_id: 0,
            presence_person: String::new(),
            presence_interval_secs: 5,
            presence_ocr_area: None,
            presence_color_area: None,
            presence_current_status: None,
            presence_current_started: None,
            presence_periods: Vec::new(),
            presence_last_ocr: String::new(),
            presence_receiver: None,
            presence_control_sender: None,
            presence_region_setup_pending: false,
            presence_region_backup: None,
        };
        app.refresh_sources();
        app.automation_window_id = app.windows.first().map(|window| window.id).unwrap_or(0);
        app.mouse_typing_window_id = app.windows.first().map(|window| window.id).unwrap_or(0);
        app.presence_window_id = app
            .windows
            .iter()
            .find(|window| is_teams_window(window))
            .map(|window| window.id)
            .unwrap_or(0);
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

    fn export_presence(&self) -> Result<(), String> {
        if self.save_dir.is_empty() {
            return Ok(());
        }
        let directory = Path::new(&self.save_dir);
        let json = serde_json::to_string_pretty(&self.presence_periods)
            .map_err(|error| format!("Kunde inte skapa Teams JSON: {error}"))?;
        fs::write(directory.join("teams-status.json"), json)
            .map_err(|error| format!("Kunde inte skriva teams-status.json: {error}"))?;
        let mut csv = String::from("person,status,start,slut,varaktighet_sekunder\n");
        for period in &self.presence_periods {
            let _ = writeln!(
                csv,
                "{},{},{},{},{}",
                csv_field(&period.person),
                csv_field(period.status.label()),
                csv_field(&period.started_at),
                csv_field(period.ended_at.as_deref().unwrap_or("")),
                period.duration_seconds
            );
        }
        fs::write(directory.join("teams-status.csv"), csv)
            .map_err(|error| format!("Kunde inte skriva teams-status.csv: {error}"))
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
        self.region_selected = None;
        self.region_interaction = None;
        self.region_result_preview = false;
        self.region_source_image = None;
        self.region_source_type.clear();
        self.region_source_id = None;
        self.region_source_size = None;
    }

    fn visual_region_backup(&self) -> VisualRegionBackup {
        VisualRegionBackup {
            enable_crop: self.enable_crop,
            crop: (self.crop_x, self.crop_y, self.crop_w, self.crop_h),
            enable_ocr: self.enable_ocr,
            ocr_regions: self.ocr_regions.clone(),
            measurement_regions: self.measurement_regions.clone(),
            indicator: (
                self.indicator_x,
                self.indicator_y,
                self.indicator_w,
                self.indicator_h,
            ),
            source_type: self.region_source_type.clone(),
            source_id: self.region_source_id,
            source_size: self.region_source_size,
        }
    }

    fn restore_visual_region_backup(&mut self) {
        let Some(backup) = self.presence_region_backup.take() else {
            return;
        };
        self.enable_crop = backup.enable_crop;
        (self.crop_x, self.crop_y, self.crop_w, self.crop_h) = backup.crop;
        self.enable_ocr = backup.enable_ocr;
        self.ocr_regions = backup.ocr_regions;
        self.measurement_regions = backup.measurement_regions;
        (
            self.indicator_x,
            self.indicator_y,
            self.indicator_w,
            self.indicator_h,
        ) = backup.indicator;
        self.region_source_type = backup.source_type;
        self.region_source_id = backup.source_id;
        self.region_source_size = backup.source_size;
        self.region_source_image = None;
        self.region_editor_active = false;
        self.region_result_preview = false;
        self.region_drag_start = None;
        self.region_selected = None;
        self.region_interaction = None;
    }

    fn begin_presence_region_setup(&mut self, ctx: &egui::Context) {
        if !self
            .windows
            .iter()
            .any(|window| window.id == self.presence_window_id)
        {
            self.presence_status_text = "Välj ett tillgängligt Teams-fönster först.".to_string();
            return;
        }
        self.presence_region_backup = Some(self.visual_region_backup());
        self.clear_visual_regions();
        self.mode = "live".to_string();
        self.source_type = "window".to_string();
        self.selected_source_id = self.presence_window_id;
        self.presence_region_setup_pending = true;
        self.active_tab = "analysis".to_string();
        self.start_region_editor(ctx);
        if self.region_editor_active {
            self.status_text = "Markera personraden som OCR och statusbollen som Pixel/färg. Välj sedan OK och Använd för Teams-status.".to_string();
        } else {
            self.presence_region_setup_pending = false;
            self.restore_visual_region_backup();
        }
    }

    fn finish_presence_region_setup(&mut self) -> Result<(), String> {
        if self.region_source_type != "window"
            || self.region_source_id != Some(self.presence_window_id)
        {
            return Err("Markeringarna kommer inte från det valda Teams-fönstret.".to_string());
        }
        let ocr_area = self.ocr_regions.first().copied();
        let color_area = self.measurement_regions.first().copied();
        if ocr_area.is_none() && color_area.is_none() {
            return Err("Markera minst personraden eller statusbollen först.".to_string());
        }
        self.presence_ocr_area = ocr_area;
        self.presence_color_area = color_area;
        self.presence_region_setup_pending = false;
        self.restore_visual_region_backup();
        self.active_tab = "presence".to_string();
        self.presence_status_text = format!(
            "Teams-områden klara: OCR {}, statusboll {}.",
            if self.presence_ocr_area.is_some() {
                "ja"
            } else {
                "nej"
            },
            if self.presence_color_area.is_some() {
                "ja"
            } else {
                "nej"
            }
        );
        Ok(())
    }

    fn cancel_region_setup(&mut self) {
        if self.presence_region_setup_pending {
            self.presence_region_setup_pending = false;
            self.restore_visual_region_backup();
            self.active_tab = "presence".to_string();
            self.presence_status_text = "Teams-markeringen avbröts.".to_string();
        } else {
            self.region_editor_active = false;
            self.region_result_preview = false;
            self.region_source_image = None;
            self.status_text = "Områdesväljaren stängdes.".to_string();
        }
    }

    fn has_visual_regions(&self) -> bool {
        self.enable_crop || !self.ocr_regions.is_empty() || !self.measurement_regions.is_empty()
    }

    fn set_region_value(&mut self, kind: RegionKind, index: usize, area: (u32, u32, u32, u32)) {
        match kind {
            RegionKind::Screenshot => {
                self.enable_crop = true;
                (self.crop_x, self.crop_y, self.crop_w, self.crop_h) = area;
            }
            RegionKind::Ocr => {
                if let Some(region) = self.ocr_regions.get_mut(index) {
                    *region = area;
                }
            }
            RegionKind::Measurement => {
                if let Some(region) = self.measurement_regions.get_mut(index) {
                    *region = area;
                }
                if index == 0 {
                    (
                        self.indicator_x,
                        self.indicator_y,
                        self.indicator_w,
                        self.indicator_h,
                    ) = area;
                }
            }
        }
    }

    fn show_region_result(&mut self, ctx: &egui::Context) {
        let Some(source) = self.region_source_image.clone() else {
            return;
        };
        let result = if self.enable_crop {
            capture::crop_image(
                &source,
                (self.crop_x, self.crop_y, self.crop_w, self.crop_h),
            )
        } else {
            source
        };
        self.display_preview_image(result, "Färdigt skärmklippsresultat".to_string(), ctx);
        self.region_editor_active = false;
        self.region_result_preview = true;
        self.region_drag_start = None;
        self.region_interaction = None;
        self.status_text = "Visar exakt det område som kommer att skärmklippas.".to_string();
    }

    fn return_to_region_editor(&mut self, ctx: &egui::Context) {
        if let Some(source) = self.region_source_image.clone() {
            self.display_preview_image(source, "Områdesväljare".to_string(), ctx);
            self.region_editor_active = true;
            self.region_result_preview = false;
            self.status_text = "Justera markeringarna och välj OK igen.".to_string();
        }
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
        self.preview_change_bounds = None;

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
                        let mut change_bounds = None;
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
                            change_bounds = difference.bounds;
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
                            change_summary: change_summary(
                                changed_pixels,
                                change_bounds,
                                ocr_changed,
                            ),
                            change_bounds,
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

    fn stop_offline_analysis(&mut self) {
        if let Some(sender) = self.control_sender.as_ref() {
            let _ = sender.send(ControlMessage::Stop);
        }
        // Låt worker-tråden avsluta själv så att redan beräknade meddelanden hinner tas emot
        // och exporteras innan gränssnittet återgår till viloläge.
        self.status_text = "Avbryter efterhandsanalysen…".to_string();
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
        self.preview_change_bounds = None;

        let (log_tx, log_rx) = channel();
        let (control_tx, control_rx) = channel();
        self.log_receiver = Some(log_rx);
        self.control_sender = Some(control_tx);
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

            if stop_requested(&control_rx) {
                let _ = log_tx.send(WorkerMessage::OfflineDone(
                    "Efterhandsanalysen avbröts.".to_string(),
                ));
                ctx.request_repaint();
                return;
            }

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
                // Avbryt mellan filer. Resultaten som redan har skickats till UI:t behålls.
                if stop_requested(&control_rx) {
                    let _ = log_tx.send(WorkerMessage::OfflineDone(
                        "Efterhandsanalysen avbröts.".to_string(),
                    ));
                    ctx.request_repaint();
                    return;
                }
                match image::open(path) {
                    Ok(original_img) => {
                        if stop_requested(&control_rx) {
                            let _ = log_tx.send(WorkerMessage::OfflineDone(
                                "Efterhandsanalysen avbröts.".to_string(),
                            ));
                            ctx.request_repaint();
                            return;
                        }
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
                        let mut change_bounds = None;
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
                            change_bounds = difference.bounds;
                        }

                        if stop_requested(&control_rx) {
                            let _ = log_tx.send(WorkerMessage::OfflineDone(
                                "Efterhandsanalysen avbröts.".to_string(),
                            ));
                            ctx.request_repaint();
                            return;
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
                                if stop_requested(&control_rx) {
                                    let _ = log_tx.send(WorkerMessage::OfflineDone(
                                        "Efterhandsanalysen avbröts.".to_string(),
                                    ));
                                    ctx.request_repaint();
                                    return;
                                }
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
                                if stop_requested(&control_rx) {
                                    let _ = log_tx.send(WorkerMessage::OfflineDone(
                                        "Efterhandsanalysen avbröts.".to_string(),
                                    ));
                                    ctx.request_repaint();
                                    return;
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
                            change_summary: change_summary(
                                changed_pixels,
                                change_bounds,
                                ocr_changed,
                            ),
                            change_bounds,
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
        if !available.contains(&self.presence_window_id) {
            self.presence_window_id = self
                .windows
                .iter()
                .find(|window| is_teams_window(window))
                .map(|window| window.id)
                .unwrap_or(0);
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

    fn start_macro_recording(&mut self) {
        if self.automation_running || self.mouse_running {
            self.automation_status =
                "Stoppa pågående mus- eller RPA-automatisering före inspelning.".to_string();
            return;
        }
        match macro_recorder::MacroRecorder::start() {
            Ok(recorder) => {
                self.automation_recorder = Some(recorder);
                self.automation_status = "INSPELNING PÅGÅR – använd andra program normalt. Stoppa med Ctrl+Alt+Esc eller knappen här.".to_string();
                self.automation_last_activity =
                    "Vanlig tangenttext och musrörelser spelas inte in.".to_string();
            }
            Err(error) => self.automation_status = error,
        }
    }

    fn stop_macro_recording(&mut self) {
        let Some(recorder) = self.automation_recorder.take() else {
            return;
        };
        match recorder.stop() {
            Ok(recording) => self.import_macro_recording(recording),
            Err(error) => self.automation_status = error,
        }
    }

    fn import_macro_recording(&mut self, recording: macro_recorder::MacroRecording) {
        let mut steps = Vec::new();
        let mut previous_at = None;
        let mut active_window = None;
        let mut first_window = None;
        let mut skipped = 0_usize;

        for event in recording.events {
            let Ok(window_id) = u32::try_from(event.window.hwnd) else {
                skipped += 1;
                continue;
            };
            first_window.get_or_insert(window_id);
            let delay_ms = previous_at
                .map(|previous| event.after_ms.saturating_sub(previous).min(600_000))
                .unwrap_or(0);
            previous_at = Some(event.after_ms);
            if delay_ms >= 50 {
                steps.push(AutomationStepEditor::recorded_wait(delay_ms));
            }
            if active_window != Some(window_id) {
                let mut switch = AutomationStepEditor::switch_window(window_id);
                switch.value = event.window.title.clone();
                steps.push(switch);
                active_window = Some(window_id);
            }
            match event.action {
                macro_recorder::RecordedAction::Click {
                    button,
                    normalized_x,
                    normalized_y,
                } => steps.push(AutomationStepEditor::recorded_click(
                    normalized_x,
                    normalized_y,
                    button,
                )),
                macro_recorder::RecordedAction::Shortcut(shortcut) => {
                    let value = match shortcut {
                        macro_recorder::RecordedShortcut::Copy => "copy",
                        macro_recorder::RecordedShortcut::Paste => "paste",
                        macro_recorder::RecordedShortcut::SelectAll => "select_all",
                        macro_recorder::RecordedShortcut::Enter => "enter",
                        macro_recorder::RecordedShortcut::Escape => "escape",
                    };
                    steps.push(AutomationStepEditor::shortcut(value));
                }
            }
        }

        if steps.is_empty() {
            self.automation_status =
                "Inspelningen innehöll inga importerbara klick eller godkända kortkommandon."
                    .to_string();
            return;
        }
        let action_count = steps
            .iter()
            .filter(|step| matches!(step.kind.as_str(), "click_relative" | "shortcut"))
            .count();
        self.automation_steps = steps;
        if let Some(window_id) = first_window {
            self.automation_window_id = window_id;
        }
        self.automation_repeat = false;
        self.automation_dry_run = true;
        self.refresh_mouse_windows();
        self.automation_status = format!(
            "Importerade {action_count} åtgärder från inspelningen{}; kontrollera fönster och kör torrkörning.",
            if skipped == 0 {
                String::new()
            } else {
                format!(" ({skipped} hoppades över)")
            }
        );
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
                "switch_window" => {
                    let Some(window) = self
                        .windows
                        .iter()
                        .find(|window| window.id == step.target_window_id)
                    else {
                        self.automation_status =
                            format!("Steg {} saknar ett tillgängligt målfönster.", index + 1);
                        return;
                    };
                    if !self.automation_dry_run
                        && !step.value.trim().is_empty()
                        && window.title != step.value
                    {
                        self.automation_status = format!(
                            "Steg {} stoppades: fönstertiteln har ändrats från ‘{}’ till ‘{}’. Godkänn aktuell titel i steget före riktig körning.",
                            index + 1,
                            step.value,
                            window.title
                        );
                        return;
                    }
                    steps.push(mouse_sim::AutomationStep::SetTargetWindow(
                        step.target_window_id,
                    ));
                }
                "wait" => steps.push(mouse_sim::AutomationStep::Wait(Duration::from_secs(
                    step.seconds,
                ))),
                "wait_recorded" => steps.push(mouse_sim::AutomationStep::Wait(
                    Duration::from_millis(step.delay_ms),
                )),
                "wait_ready" => steps.push(mouse_sim::AutomationStep::WaitForStable {
                    minimum: Duration::from_secs(step.seconds),
                    stable_for: Duration::from_secs(2),
                    timeout: Duration::from_secs(step.timeout_seconds.max(2)),
                    max_changed_fraction: 0.005,
                }),
                "type_text" => {
                    if step.value.is_empty() {
                        self.automation_status =
                            format!("Steg {} saknar text att skriva.", index + 1);
                        return;
                    }
                    steps.push(mouse_sim::AutomationStep::TypeText(step.value.clone()));
                }
                "shortcut" => {
                    let shortcut = match step.value.as_str() {
                        "paste" => mouse_sim::Shortcut::Paste,
                        "select_all" => mouse_sim::Shortcut::SelectAll,
                        "enter" => mouse_sim::Shortcut::Enter,
                        "escape" => mouse_sim::Shortcut::Escape,
                        _ => mouse_sim::Shortcut::Copy,
                    };
                    steps.push(mouse_sim::AutomationStep::Shortcut(shortcut));
                }
                "confirm" => steps.push(mouse_sim::AutomationStep::PauseForConfirmation(
                    if step.value.trim().is_empty() {
                        "Kontrollera resultatet innan du fortsätter manuellt.".to_string()
                    } else {
                        step.value.trim().to_string()
                    },
                )),
                "click_image" => {
                    let (Some(template), Some(expected_area), Some(reference_size)) = (
                        step.template_image.clone(),
                        step.expected_area,
                        step.reference_size,
                    ) else {
                        self.automation_status =
                            format!("Steg {} saknar en markering från målfönstret.", index + 1);
                        return;
                    };
                    steps.push(mouse_sim::AutomationStep::ClickImage {
                        template,
                        expected_area,
                        reference_size,
                        timeout: Duration::from_secs(step.seconds.max(1)),
                        confidence: step.confidence / 100.0,
                        position_tolerance: 0.10,
                    });
                }
                "click_relative" => {
                    steps.push(mouse_sim::AutomationStep::ClickRelative {
                        x_fraction: step.relative_x.clamp(0.0, 1.0),
                        y_fraction: step.relative_y.clamp(0.0, 1.0),
                        button: if step.click_button == "right" {
                            mouse_sim::PointerButton::Right
                        } else {
                            mouse_sim::PointerButton::Left
                        },
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
            dry_run: self.automation_dry_run,
        };
        let (event_tx, event_rx) = channel();
        let (control_tx, control_rx) = channel();
        self.automation_receiver = Some(event_rx);
        self.automation_control_sender = Some(control_tx);
        self.automation_running = true;
        self.automation_status = if self.automation_dry_run {
            "Torrkörning av RPA-sekvensen pågår.".to_string()
        } else {
            "RPA-sekvensen körs.".to_string()
        };
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

    fn start_presence_tracking(&mut self, ctx: &egui::Context) {
        if self.presence_person.trim().is_empty() {
            self.presence_status_text = "Ange namnet som ska följas.".to_string();
            return;
        }
        if !self
            .windows
            .iter()
            .any(|window| window.id == self.presence_window_id)
        {
            self.presence_status_text = "Välj ett tillgängligt Teams-fönster.".to_string();
            return;
        }
        if self.presence_ocr_area.is_none() && self.presence_color_area.is_none() {
            self.presence_status_text =
                "Markera ett OCR-område eller området runt statusbollen först.".to_string();
            return;
        }
        let config = presence::Config {
            window_id: self.presence_window_id,
            person: self.presence_person.trim().to_string(),
            ocr_area: self.presence_ocr_area,
            color_area: self.presence_color_area,
            interval: Duration::from_secs(self.presence_interval_secs.max(1)),
            confirmation_samples: 2,
        };
        let (event_tx, event_rx) = channel();
        let (control_tx, control_rx) = channel();
        self.presence_receiver = Some(event_rx);
        self.presence_control_sender = Some(control_tx);
        self.presence_running = true;
        self.presence_status_text = "Övervakar Teams-status…".to_string();
        std::thread::spawn(move || presence::run(config, control_rx, event_tx));
        ctx.request_repaint();
    }

    fn stop_presence_tracking(&mut self) {
        if let Some(sender) = self.presence_control_sender.as_ref() {
            let _ = sender.send(());
        }
        self.presence_status_text = "Stoppar…".to_string();
    }

    fn show_presence_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Teams-status");
            ui.label(
                "Följ din egen eller en annan synlig persons status lokalt och summera tiden i varje läge.",
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Status:");
                ui.colored_label(
                    if self.presence_running {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::LIGHT_GRAY
                    },
                    &self.presence_status_text,
                );
            });
            if let Some(status) = self.presence_current_status {
                ui.strong(format!("Aktuellt läge: {}", status.label()));
            }
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!self.presence_running, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Teams-fönster:");
                        egui::ComboBox::from_id_source("presence_window")
                            .width(420.0)
                            .selected_text(
                                self.windows
                                    .iter()
                                    .find(|window| window.id == self.presence_window_id)
                                    .map(|window| {
                                        format!("{} — {}", window.app_name, window.title)
                                    })
                                    .unwrap_or_else(|| "Välj fönster…".to_string()),
                            )
                            .show_ui(ui, |ui| {
                                for window in self
                                    .windows
                                    .iter()
                                    .filter(|window| is_teams_window(window))
                                {
                                    ui.selectable_value(
                                        &mut self.presence_window_id,
                                        window.id,
                                        format!("{} — {}", window.app_name, window.title),
                                    );
                                }
                            });
                        if !self.windows.iter().any(is_teams_window) {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                "Inget synligt Teams-fönster hittades. Öppna Teams och välj Uppdatera.",
                            );
                        }
                        if ui.button("Uppdatera").clicked() {
                            self.refresh_mouse_windows();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Namn:");
                        ui.text_edit_singleline(&mut self.presence_person);
                        ui.label("Kontrollera var");
                        ui.add(
                            egui::DragValue::new(&mut self.presence_interval_secs)
                                .range(1..=300)
                                .suffix(":e sekund"),
                        );
                    });

                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Markera områden visuellt…").clicked() {
                            self.begin_presence_region_setup(ctx);
                        }
                        if ui.button("Hämta markeringarna").clicked() {
                            if self.region_source_type == "window"
                                && self.region_source_id == Some(self.presence_window_id)
                            {
                                self.presence_ocr_area = self.ocr_regions.first().copied();
                                self.presence_color_area = self.measurement_regions.first().copied();
                                self.presence_status_text = format!(
                                    "Områden hämtade: OCR {}, statusboll {}.",
                                    if self.presence_ocr_area.is_some() { "ja" } else { "nej" },
                                    if self.presence_color_area.is_some() { "ja" } else { "nej" }
                                );
                            } else {
                                self.presence_status_text = "De befintliga markeringarna kommer inte från valt Teams-fönster. Välj Markera områden visuellt.".to_string();
                            }
                        }
                    });
                    ui.small(
                        "OCR-området bör innehålla namn och eventuell statustext. Pixel/färg-området ska ligga tätt runt statusbollen i Teams-listan.",
                    );
                    ui.label(format!(
                        "OCR: {} · statusboll: {}",
                        self.presence_ocr_area
                            .map(|area| format!("{},{} {}×{}", area.0, area.1, area.2, area.3))
                            .unwrap_or_else(|| "inte vald".to_string()),
                        self.presence_color_area
                            .map(|area| format!("{},{} {}×{}", area.0, area.1, area.2, area.3))
                            .unwrap_or_else(|| "inte vald".to_string())
                    ));
                    ui.small(
                        "Enbart statusbollen räcker för grön/tillgänglig, röd/upptagen, gul/frånvarande och grå/offline. OCR behövs för att säkert skilja Stör ej från andra röda lägen.",
                    );
                });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if self.presence_running {
                        if ui.button("Stoppa statusövervakning").clicked() {
                            self.stop_presence_tracking();
                        }
                    } else if ui.button("Starta statusövervakning").clicked() {
                        self.start_presence_tracking(ctx);
                    }
                    if ui
                        .add_enabled(!self.presence_running, egui::Button::new("Rensa historik"))
                        .clicked()
                    {
                        self.presence_periods.clear();
                        self.presence_current_status = None;
                        self.presence_current_started = None;
                    }
                });

                ui.add_space(14.0);
                ui.heading("Tid per status");
                let current_elapsed = self
                    .presence_current_started
                    .map(|started| started.elapsed().as_secs())
                    .unwrap_or(0);
                let totals = presence::PresenceStatus::ALL
                    .into_iter()
                    .map(|status| {
                        let mut seconds = self
                            .presence_periods
                            .iter()
                            .filter(|period| period.status == status)
                            .map(|period| period.duration_seconds)
                            .sum::<u64>();
                        if self.presence_current_status == Some(status) {
                            seconds += current_elapsed;
                        }
                        (status, seconds)
                    })
                    .collect::<Vec<_>>();
                let maximum = totals.iter().map(|(_, seconds)| *seconds).max().unwrap_or(1).max(1);
                for (status, seconds) in totals {
                    ui.horizontal(|ui| {
                        ui.add_sized([110.0, 20.0], egui::Label::new(status.label()));
                        let width = (ui.available_width() - 100.0).max(20.0);
                        let (bar, _) = ui.allocate_exact_size(
                            egui::vec2(width * seconds as f32 / maximum as f32, 16.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(bar, 2.0, presence_color(status));
                        ui.label(format_duration(seconds));
                    });
                }

                ui.add_space(12.0);
                ui.heading("Statushistorik");
                for period in self.presence_periods.iter().rev().take(500) {
                    let duration = if period.ended_at.is_none()
                        && self.presence_current_status == Some(period.status)
                    {
                        period.duration_seconds + current_elapsed
                    } else {
                        period.duration_seconds
                    };
                    ui.horizontal(|ui| {
                        ui.label(&period.started_at);
                        ui.colored_label(presence_color(period.status), period.status.label());
                        ui.label(format_duration(duration));
                    });
                }
                if !self.presence_last_ocr.is_empty() {
                    ui.collapsing("Senaste OCR-text", |ui| {
                        ui.label(&self.presence_last_ocr);
                    });
                }
                ui.small(
                    "Historiken exporteras till teams-status.csv och teams-status.json när en analysmapp har valts.",
                );
            });
        });
    }

    fn automation_window_for_step(&self, step_index: usize) -> u32 {
        self.automation_steps
            .iter()
            .take(step_index + 1)
            .rev()
            .find(|step| step.kind == "switch_window" && step.target_window_id != 0)
            .map(|step| step.target_window_id)
            .unwrap_or(self.automation_window_id)
    }

    fn load_outlook_chatgpt_template(&mut self) {
        let outlook_window = self.automation_window_id;
        let mut reply = AutomationStepEditor::click_word();
        reply.value = "Svara".to_string();

        self.automation_steps = vec![
            AutomationStepEditor::switch_window(outlook_window),
            AutomationStepEditor::image_target("Markera det senaste mejlet"),
            AutomationStepEditor::wait_ready(2),
            AutomationStepEditor::image_target("Markera mejlets brödtext"),
            AutomationStepEditor::shortcut("select_all"),
            AutomationStepEditor::shortcut("copy"),
            AutomationStepEditor::switch_window(0),
            AutomationStepEditor::image_target("Markera ChatGPTs skrivfält"),
            AutomationStepEditor::basic(
                "type_text",
                "Skriv ett professionellt och kort svar på följande mejl. Skapa bara svarstexten:\n\n",
            ),
            AutomationStepEditor::shortcut("paste"),
            AutomationStepEditor::shortcut("enter"),
            AutomationStepEditor::wait_ready(5),
            AutomationStepEditor::image_target("Markera ChatGPTs knapp Kopiera svar"),
            AutomationStepEditor::switch_window(outlook_window),
            reply,
            AutomationStepEditor::wait_ready(2),
            AutomationStepEditor::image_target("Markera Outlooks svarsfält"),
            AutomationStepEditor::shortcut("paste"),
            AutomationStepEditor::basic(
                "confirm",
                "Granska mottagare, ämne och svar. Skicka sedan mejlet manuellt i Outlook.",
            ),
        ];
        self.automation_repeat = false;
        self.automation_dry_run = true;
        self.automation_status = "Mallen är inlagd. Välj ChatGPT-fönster i steg 7 och markera alla bildmål innan torrkörning.".to_string();
    }

    fn load_ten_click_template(&mut self) {
        let mut steps = Vec::with_capacity(19);
        for click_number in 1..=10 {
            steps.push(AutomationStepEditor::image_target(&format!(
                "Markera klickmål {click_number}"
            )));
            if click_number < 10 {
                let mut wait = AutomationStepEditor::wait();
                wait.seconds = 2;
                steps.push(wait);
            }
        }
        self.automation_steps = steps;
        self.automation_repeat = false;
        self.automation_dry_run = true;
        self.automation_status =
            "10-klicksmall inlagd. Markera varje mål och justera väntetiderna.".to_string();
    }

    fn start_automation_picker(&mut self, step_index: usize, ctx: &egui::Context) {
        let window_id = self.automation_window_for_step(step_index);
        match capture::capture_source("window", window_id, None) {
            Ok(image) => {
                let rgba = image.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                self.automation_picker_texture = Some(ctx.load_texture(
                    "automation_target_picker",
                    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
                    egui::TextureOptions::default(),
                ));
                self.automation_picker_image = Some(image);
                self.automation_picker_size = Some(size);
                self.automation_picker_step = Some(step_index);
                self.automation_picker_drag_start = None;
                self.automation_picker_selection = None;
                self.automation_status =
                    "Dra en ram runt knappen eller bilden som ska klickas.".to_string();
            }
            Err(error) => {
                self.automation_status = format!("Kunde inte ta klipp av målfönstret: {error}");
            }
        }
    }

    fn close_automation_picker(&mut self) {
        self.automation_picker_step = None;
        self.automation_picker_image = None;
        self.automation_picker_texture = None;
        self.automation_picker_size = None;
        self.automation_picker_drag_start = None;
        self.automation_picker_selection = None;
    }

    fn show_automation_image_picker(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Markera bildmål i målfönstret");
            ui.label(
                "Dra tätt runt den textlösa knappen eller grafiken. Den ursprungliga platsen används som ankare med ±10 % sökmarginal.",
            );
            let mut cancel = false;
            let mut apply = false;
            ui.horizontal(|ui| {
                if ui.button("Avbryt").clicked() {
                    cancel = true;
                }
                if ui
                    .add_enabled(
                        self.automation_picker_selection.is_some(),
                        egui::Button::new("Använd markering"),
                    )
                    .clicked()
                {
                    apply = true;
                }
            });
            ui.separator();

            if cancel {
                self.close_automation_picker();
                self.automation_status = "Bildmarkeringen avbröts.".to_string();
                return;
            }
            if apply {
                if let (Some(step_index), Some(image), Some(area), Some(size)) = (
                    self.automation_picker_step,
                    self.automation_picker_image.as_ref(),
                    self.automation_picker_selection,
                    self.automation_picker_size,
                ) {
                    let template = capture::crop_image(image, area);
                    if let Some(step) = self.automation_steps.get_mut(step_index) {
                        step.template_image = Some(template);
                        step.expected_area = Some(area);
                        step.reference_size = Some((size[0] as u32, size[1] as u32));
                        step.value = format!(
                            "Markerat: x {}, y {}, {}×{} px",
                            area.0, area.1, area.2, area.3
                        );
                    }
                    self.close_automation_picker();
                    self.automation_status =
                        "Bildmålet sparades med ±10 % positionsmarginal.".to_string();
                }
                return;
            }

            let (Some(texture), Some(size)) = (
                self.automation_picker_texture.as_ref(),
                self.automation_picker_size,
            ) else {
                ui.label("Ingen fönsterbild tillgänglig.");
                return;
            };
            let canvas_size = egui::vec2(
                ui.available_width().max(1.0),
                ui.available_height().max(1.0),
            );
            let (canvas, response) =
                ui.allocate_exact_size(canvas_size, egui::Sense::click_and_drag());
            let painter = ui.painter_at(canvas);
            painter.rect_filled(canvas, 4.0, egui::Color32::from_black_alpha(40));
            let image_size = egui::vec2(size[0] as f32, size[1] as f32);
            let fit_scale = (canvas.width() / image_size.x)
                .min(canvas.height() / image_size.y)
                .max(f32::EPSILON);
            let image_rect = egui::Rect::from_center_size(canvas.center(), image_size * fit_scale);
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            let pointer_to_image = |position: egui::Pos2| {
                egui::pos2(
                    ((position.x - image_rect.left()) / image_rect.width() * size[0] as f32)
                        .clamp(0.0, size[0] as f32),
                    ((position.y - image_rect.top()) / image_rect.height() * size[1] as f32)
                        .clamp(0.0, size[1] as f32),
                )
            };
            let region_to_screen = |area: (u32, u32, u32, u32)| {
                egui::Rect::from_min_max(
                    egui::pos2(
                        image_rect.left() + area.0 as f32 / size[0] as f32 * image_rect.width(),
                        image_rect.top() + area.1 as f32 / size[1] as f32 * image_rect.height(),
                    ),
                    egui::pos2(
                        image_rect.left()
                            + (area.0 + area.2) as f32 / size[0] as f32 * image_rect.width(),
                        image_rect.top()
                            + (area.1 + area.3) as f32 / size[1] as f32 * image_rect.height(),
                    ),
                )
            };

            if let Some(area) = self.automation_picker_selection {
                painter.rect_stroke(
                    region_to_screen(area),
                    0.0,
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                );
            }
            if response.drag_started_by(egui::PointerButton::Primary) {
                if let Some(position) = response.interact_pointer_pos() {
                    if image_rect.contains(position) {
                        self.automation_picker_drag_start = Some(pointer_to_image(position));
                    }
                }
            }
            if let (Some(start), Some(position)) = (
                self.automation_picker_drag_start,
                response.interact_pointer_pos(),
            ) {
                let selection = egui::Rect::from_two_pos(start, pointer_to_image(position));
                let preview = (
                    selection.min.x.floor() as u32,
                    selection.min.y.floor() as u32,
                    selection.width().ceil() as u32,
                    selection.height().ceil() as u32,
                );
                painter.rect_stroke(
                    region_to_screen(preview),
                    0.0,
                    egui::Stroke::new(2.0, egui::Color32::LIGHT_BLUE),
                );
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                if let (Some(start), Some(position)) = (
                    self.automation_picker_drag_start.take(),
                    response.interact_pointer_pos(),
                ) {
                    let selection = egui::Rect::from_two_pos(start, pointer_to_image(position));
                    let area = (
                        selection.min.x.floor() as u32,
                        selection.min.y.floor() as u32,
                        selection.width().ceil() as u32,
                        selection.height().ceil() as u32,
                    );
                    if area.2 >= 8 && area.3 >= 8 {
                        self.automation_picker_selection = Some(area);
                    }
                }
            }
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        });
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
                        "Klick är avstängt som standard. Anteckningar - ASC klickar aldrig i ett omarkerat fönster.",
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
                                "Vid skrivning klickar Anteckningar - ASC endast i det valda skrivfönstrets innehållsyta.",
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
        if self.automation_picker_step.is_some() {
            self.show_automation_image_picker(ctx);
            return;
        }
        let macro_recording = self.automation_recorder.is_some();
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("RPA-klicksekvens");
            ui.label(
                "Bygg ett flöde som väntar, hittar text eller bilder, klickar och skriver i ett valt fönster.",
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("Status:");
                ui.colored_label(
                    if macro_recording {
                        egui::Color32::RED
                    } else if self.automation_running {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::LIGHT_GRAY
                    },
                    &self.automation_status,
                );
            });
            ui.small(format!("Senast: {}", self.automation_last_activity));
            ui.horizontal_wrapped(|ui| {
                if macro_recording {
                    if ui
                        .add(
                            egui::Button::new("Stoppa och importera inspelning")
                                .fill(egui::Color32::from_rgb(190, 45, 45)),
                        )
                        .clicked()
                    {
                        self.stop_macro_recording();
                    }
                    ui.colored_label(
                        egui::Color32::RED,
                        "Global inspelning pågår – Ctrl+Alt+Esc stoppar från valfritt fönster.",
                    );
                } else if ui.button("Spela in klick och kortkommandon").clicked() {
                    self.start_macro_recording();
                }
            });
            ui.small("Windows-inspelaren sparar vänster-/högerklick, Ctrl+C/V/A, Enter och Escape. Vanlig text, lösenord, urklippsinnehåll och musrörelser sparas aldrig.");
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_enabled_ui(!self.automation_running && !macro_recording, |ui| {
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
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Mall: Outlook → ChatGPT → utkast").clicked() {
                            self.load_outlook_chatgpt_template();
                        }
                        if ui.button("Mall: exakt 10 klick").clicked() {
                            self.load_ten_click_template();
                        }
                    });
                    ui.small(
                        "Mallar ersätter den aktuella steglistan och startar alltid i torrkörningsläge.",
                    );
                    let mut remove = None;
                    let mut move_up = None;
                    let mut move_down = None;
                    let mut pick_image = None;
                    let step_count = self.automation_steps.len();
                    for (index, step) in self.automation_steps.iter_mut().enumerate() {
                        ui.group(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(format!("{}", index + 1));
                                egui::ComboBox::from_id_source(("automation_step", index))
                                    .selected_text(match step.kind.as_str() {
                                        "switch_window" => "Byt målfönster",
                                        "wait" => "Vänta",
                                        "wait_recorded" => "Inspelad väntan",
                                        "wait_ready" => "Vänta tills sidan är klar",
                                        "type_text" => "Skriv text",
                                        "shortcut" => "Kortkommando",
                                        "confirm" => "Manuell bekräftelse",
                                        "click_relative" => "Inspelat positionsklick",
                                        "click_image" => "Klicka på bild",
                                        _ => "Klicka på OCR-ord",
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "switch_window".to_string(),
                                            "Byt målfönster",
                                        );
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
                                            "click_relative".to_string(),
                                            "Inspelat positionsklick",
                                        );
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "wait".to_string(),
                                            "Vänta",
                                        );
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "wait_ready".to_string(),
                                            "Vänta tills sidan är klar",
                                        );
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "type_text".to_string(),
                                            "Skriv text",
                                        );
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "shortcut".to_string(),
                                            "Kortkommando",
                                        );
                                        ui.selectable_value(
                                            &mut step.kind,
                                            "confirm".to_string(),
                                            "Manuell bekräftelse",
                                        );
                                    });
                                match step.kind.as_str() {
                                    "switch_window" => {
                                        egui::ComboBox::from_id_source((
                                            "automation_step_window",
                                            index,
                                        ))
                                        .width(360.0)
                                        .selected_text(
                                            self.windows
                                                .iter()
                                                .find(|window| {
                                                    window.id == step.target_window_id
                                                })
                                                .map(|window| {
                                                    format!(
                                                        "{} — {}",
                                                        window.app_name, window.title
                                                    )
                                                })
                                                .unwrap_or_else(|| {
                                                    "Välj nästa målfönster…".to_string()
                                                }),
                                        )
                                        .show_ui(ui, |ui| {
                                            for window in &self.windows {
                                                ui.selectable_value(
                                                    &mut step.target_window_id,
                                                    window.id,
                                                    format!(
                                                        "{} — {}",
                                                        window.app_name, window.title
                                                    ),
                                                );
                                            }
                                        });
                                        if !step.value.is_empty() {
                                            let current_title = self
                                                .windows
                                                .iter()
                                                .find(|window| {
                                                    window.id == step.target_window_id
                                                })
                                                .map(|window| window.title.clone());
                                            ui.small(format!(
                                                "Inspelad titel: {}",
                                                step.value
                                            ));
                                            if current_title.as_deref() != Some(step.value.as_str())
                                                && ui.button("Godkänn aktuell titel").clicked()
                                            {
                                                if let Some(title) = current_title {
                                                    step.value = title;
                                                }
                                            }
                                        }
                                    }
                                    "wait" => {
                                        ui.add(
                                            egui::DragValue::new(&mut step.seconds)
                                                .range(0..=3_600)
                                                .suffix(" sekunder"),
                                        );
                                    }
                                    "wait_recorded" => {
                                        ui.add(
                                            egui::DragValue::new(&mut step.delay_ms)
                                                .range(0..=600_000)
                                                .suffix(" ms"),
                                        );
                                    }
                                    "wait_ready" => {
                                        ui.label("Minst:");
                                        ui.add(
                                            egui::DragValue::new(&mut step.seconds)
                                                .range(1..=3_600)
                                                .suffix(" sek"),
                                        );
                                        ui.label("Timeout efter minimitid:");
                                        ui.add(
                                            egui::DragValue::new(&mut step.timeout_seconds)
                                                .range(2..=3_600)
                                                .suffix(" sek"),
                                        );
                                    }
                                    "type_text" => {
                                        ui.label("Text:");
                                        ui.text_edit_singleline(&mut step.value);
                                    }
                                    "shortcut" => {
                                        if step.value.is_empty() {
                                            step.value = "copy".to_string();
                                        }
                                        egui::ComboBox::from_id_source((
                                            "automation_shortcut",
                                            index,
                                        ))
                                        .selected_text(match step.value.as_str() {
                                            "paste" => "Klistra in",
                                            "select_all" => "Markera allt",
                                            "enter" => "Enter",
                                            "escape" => "Escape",
                                            _ => "Kopiera",
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut step.value,
                                                "copy".to_string(),
                                                "Kopiera",
                                            );
                                            ui.selectable_value(
                                                &mut step.value,
                                                "paste".to_string(),
                                                "Klistra in",
                                            );
                                            ui.selectable_value(
                                                &mut step.value,
                                                "select_all".to_string(),
                                                "Markera allt",
                                            );
                                            ui.selectable_value(
                                                &mut step.value,
                                                "enter".to_string(),
                                                "Enter",
                                            );
                                            ui.selectable_value(
                                                &mut step.value,
                                                "escape".to_string(),
                                                "Escape",
                                            );
                                        });
                                    }
                                    "confirm" => {
                                        ui.label("Instruktion:");
                                        ui.text_edit_singleline(&mut step.value);
                                    }
                                    "click_image" => {
                                        ui.label(if step.template_image.is_some() {
                                            &step.value
                                        } else {
                                            "Ingen markering"
                                        });
                                        if ui.button("Markera i målfönster…").clicked() {
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
                                    "click_relative" => {
                                        ui.label("X:");
                                        ui.add(
                                            egui::DragValue::new(&mut step.relative_x)
                                                .range(0.0..=1.0)
                                                .speed(0.005)
                                                .custom_formatter(|value, _| {
                                                    format!("{:.1} %", value * 100.0)
                                                }),
                                        );
                                        ui.label("Y:");
                                        ui.add(
                                            egui::DragValue::new(&mut step.relative_y)
                                                .range(0.0..=1.0)
                                                .speed(0.005)
                                                .custom_formatter(|value, _| {
                                                    format!("{:.1} %", value * 100.0)
                                                }),
                                        );
                                        egui::ComboBox::from_id_source((
                                            "automation_click_button",
                                            index,
                                        ))
                                        .selected_text(if step.click_button == "right" {
                                            "Högerklick"
                                        } else {
                                            "Vänsterklick"
                                        })
                                        .show_ui(ui, |ui| {
                                            ui.selectable_value(
                                                &mut step.click_button,
                                                "left".to_string(),
                                                "Vänsterklick",
                                            );
                                            ui.selectable_value(
                                                &mut step.click_button,
                                                "right".to_string(),
                                                "Högerklick",
                                            );
                                        });
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
                        self.start_automation_picker(index, ctx);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("+ OCR-klick").clicked() {
                            self.automation_steps.push(AutomationStepEditor::click_word());
                        }
                        if ui.button("+ Vänta").clicked() {
                            self.automation_steps.push(AutomationStepEditor::wait());
                        }
                        if ui.button("+ Vänta tills klar").clicked() {
                            self.automation_steps.push(AutomationStepEditor {
                                kind: "wait_ready".to_string(),
                                value: String::new(),
                                target_window_id: 0,
                                seconds: 5,
                                timeout_seconds: 60,
                                confidence: 90.0,
                                template_image: None,
                                expected_area: None,
                                reference_size: None,
                                relative_x: 0.5,
                                relative_y: 0.5,
                                click_button: "left".to_string(),
                                delay_ms: 0,
                            });
                        }
                        if ui.button("+ Bildklick").clicked() {
                            self.automation_steps.push(AutomationStepEditor {
                                kind: "click_image".to_string(),
                                value: String::new(),
                                target_window_id: 0,
                                seconds: 30,
                                timeout_seconds: 60,
                                confidence: 85.0,
                                template_image: None,
                                expected_area: None,
                                reference_size: None,
                                relative_x: 0.5,
                                relative_y: 0.5,
                                click_button: "left".to_string(),
                                delay_ms: 0,
                            });
                        }
                        if ui.button("+ Skriv text").clicked() {
                            self.automation_steps.push(AutomationStepEditor {
                                kind: "type_text".to_string(),
                                value: String::new(),
                                target_window_id: 0,
                                seconds: 0,
                                timeout_seconds: 60,
                                confidence: 90.0,
                                template_image: None,
                                expected_area: None,
                                reference_size: None,
                                relative_x: 0.5,
                                relative_y: 0.5,
                                click_button: "left".to_string(),
                                delay_ms: 0,
                            });
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("+ Byt fönster").clicked() {
                            self.automation_steps.push(AutomationStepEditor {
                                kind: "switch_window".to_string(),
                                value: String::new(),
                                target_window_id: 0,
                                seconds: 0,
                                timeout_seconds: 60,
                                confidence: 90.0,
                                template_image: None,
                                expected_area: None,
                                reference_size: None,
                                relative_x: 0.5,
                                relative_y: 0.5,
                                click_button: "left".to_string(),
                                delay_ms: 0,
                            });
                        }
                        if ui.button("+ Kortkommando").clicked() {
                            self.automation_steps.push(AutomationStepEditor {
                                kind: "shortcut".to_string(),
                                value: "copy".to_string(),
                                target_window_id: 0,
                                seconds: 0,
                                timeout_seconds: 60,
                                confidence: 90.0,
                                template_image: None,
                                expected_area: None,
                                reference_size: None,
                                relative_x: 0.5,
                                relative_y: 0.5,
                                click_button: "left".to_string(),
                                delay_ms: 0,
                            });
                        }
                        if ui.button("+ Manuell bekräftelse").clicked() {
                            self.automation_steps.push(AutomationStepEditor {
                                kind: "confirm".to_string(),
                                value: "Kontrollera svaret och skicka det manuellt i Outlook."
                                    .to_string(),
                                target_window_id: 0,
                                seconds: 0,
                                timeout_seconds: 60,
                                confidence: 90.0,
                                template_image: None,
                                expected_area: None,
                                reference_size: None,
                                relative_x: 0.5,
                                relative_y: 0.5,
                                click_button: "left".to_string(),
                                delay_ms: 0,
                            });
                        }
                    });
                    ui.small("Bildklick använder din markering som mall och söker bara nära ursprungsplatsen (±10 % av fönstret). Vänta tills klar kräver att högst 0,5 % av pixlarna ändras under två sekunder.");
                    ui.checkbox(&mut self.automation_repeat, "Upprepa hela flödet");
                    ui.checkbox(
                        &mut self.automation_dry_run,
                        "Torrkörning – sök och verifiera, men klicka eller skriv inte",
                    );
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
        self.preview_change_bounds = None;
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
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
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
                self.region_source_image = Some(image.clone());
                self.display_preview_image(image, "Områdesväljare".to_string(), ctx);
                self.region_editor_active = true;
                self.region_result_preview = false;
                self.region_drag_start = None;
                self.region_interaction = None;
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
        let Some(log) = self.logs.get(log_index) else {
            return;
        };
        let file_name = log.file_name.clone();
        let change_bounds = log.change_bounds;
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
                self.preview_change_bounds = change_bounds;
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
        let summary_width = 178.0;
        let ocr_width = (ui.available_width()
            - timestamp_width
            - diff_width
            - pixels_width
            - status_width
            - summary_width
            - spacing * 5.0)
            .max(60.0);
        let widths = [
            timestamp_width,
            diff_width,
            pixels_width,
            ocr_width,
            summary_width,
            status_width,
        ];

        ui.horizontal(|ui| {
            for (label, width) in [
                "Tid",
                "Diff %",
                "Ändr. px",
                "Vad ändrades?",
                "OCR-text",
                "Status",
            ]
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
                            egui::Label::new(&log.change_summary).truncate(),
                        )
                        .on_hover_text(&log.change_summary);
                        ui.add_sized(
                            [widths[4], row_height],
                            egui::Label::new(&log.ocr_text).truncate(),
                        )
                        .on_hover_text(&log.ocr_text);
                        let status = if log.is_changed {
                            egui::RichText::new("Ändrad").color(egui::Color32::GREEN)
                        } else {
                            egui::RichText::new("Ingen ändring")
                        };
                        ui.add_sized([widths[5], row_height], egui::Label::new(status).truncate());
                    });
                }
            },
        );
        if let Some(log_index) = clicked_log {
            self.show_logged_capture(log_index, ui.ctx());
        }
    }

    fn show_preview_panel(&mut self, ui: &mut egui::Ui) {
        let mut show_result = false;
        let mut return_to_editor = false;
        let mut accept_result = false;
        let mut cancel_editor = false;
        if self.region_editor_active || self.region_result_preview {
            ui.horizontal_wrapped(|ui| {
                ui.heading(if self.presence_region_setup_pending {
                    "Markera Teams-områden"
                } else {
                    "Välj analysområden"
                });
                if ui.button("Maximera fönstret").clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
                if ui.button("Avbryt områdesväljaren").clicked() {
                    cancel_editor = true;
                }
            });
        }
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
                self.preview_change_bounds = None;
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
                ui.label("Rulla för zoom · vänsterdra för markering · högerdra för panorering · dubbelklicka för Anpassa");
            } else {
                ui.label("Rulla för zoom · dra för panorering · dubbelklicka för Anpassa");
            }
        });
        if self.region_result_preview {
            ui.horizontal_wrapped(|ui| {
                ui.strong("Färdigt resultat");
                if ui.button("← Tillbaka och ändra").clicked() {
                    return_to_editor = true;
                }
                let apply_label = if self.presence_region_setup_pending {
                    "Använd för Teams-status"
                } else {
                    "Använd resultatet"
                };
                if ui.button(apply_label).clicked() {
                    accept_result = true;
                }
            });
            ui.small("Bilden nedan är exakt den skärmklippsyta som används under övervakningen.");
        }
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
                    self.region_selected = None;
                    self.region_interaction = None;
                }
                if ui.button("Rensa alla områden").clicked() {
                    self.enable_crop = false;
                    self.ocr_regions.clear();
                    self.measurement_regions.clear();
                    self.region_selected = None;
                    self.region_interaction = None;
                }
                if ui.button("OK – visa färdigt resultat").clicked() {
                    show_result = true;
                }
            });
            ui.small(
                "Dra på tom yta för att skapa. Dra inuti en vald ram för att flytta; dra ett hörnhandtag för att ändra storlek.",
            );
        }
        if cancel_editor {
            self.cancel_region_setup();
            return;
        } else if show_result {
            let ctx = ui.ctx().clone();
            self.show_region_result(&ctx);
        } else if return_to_editor {
            let ctx = ui.ctx().clone();
            self.return_to_region_editor(&ctx);
        } else if accept_result {
            if self.presence_region_setup_pending {
                if let Err(error) = self.finish_presence_region_setup() {
                    self.status_text = error;
                    return;
                }
                return;
            } else {
                self.region_result_preview = false;
                self.status_text = format!(
                    "Områden används: {} OCR, {} pixel/färg.",
                    self.ocr_regions.len(),
                    self.measurement_regions.len()
                );
            }
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
        if (!self.region_editor_active && response.dragged_by(egui::PointerButton::Primary))
            || (self.region_editor_active
                && (response.dragged_by(egui::PointerButton::Secondary)
                    || response.dragged_by(egui::PointerButton::Middle)))
        {
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

        if !self.region_editor_active {
            if let Some(bounds) = self.preview_change_bounds {
                if bounds.width > 0 && bounds.height > 0 {
                    let left =
                        image_rect.left() + bounds.x as f32 / size[0] as f32 * image_rect.width();
                    let top =
                        image_rect.top() + bounds.y as f32 / size[1] as f32 * image_rect.height();
                    let right = image_rect.left()
                        + (bounds.x + bounds.width) as f32 / size[0] as f32 * image_rect.width();
                    let bottom = image_rect.top()
                        + (bounds.y + bounds.height) as f32 / size[1] as f32 * image_rect.height();
                    let changed_rect =
                        egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
                            .intersect(image_rect);
                    painter.rect_filled(
                        changed_rect,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(230, 45, 45, 42),
                    );
                    painter.rect_stroke(
                        changed_rect,
                        0.0,
                        egui::Stroke::new(2.0, egui::Color32::RED),
                    );
                    painter.text(
                        changed_rect.left_top() + egui::vec2(3.0, 3.0),
                        egui::Align2::LEFT_TOP,
                        "Ändrat område",
                        egui::FontId::proportional(11.0),
                        egui::Color32::RED,
                    );
                }
            }
        }

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
            if response.drag_started_by(egui::PointerButton::Primary) {
                if let Some(position) = response.interact_pointer_pos() {
                    if image_rect.contains(position) {
                        let image_position = pointer_to_image(position);
                        let mut regions = Vec::new();
                        if self.enable_crop {
                            regions.push((
                                RegionKind::Screenshot,
                                0,
                                (self.crop_x, self.crop_y, self.crop_w, self.crop_h),
                            ));
                        }
                        regions.extend(
                            self.ocr_regions
                                .iter()
                                .copied()
                                .enumerate()
                                .map(|(index, area)| (RegionKind::Ocr, index, area)),
                        );
                        regions.extend(
                            self.measurement_regions
                                .iter()
                                .copied()
                                .enumerate()
                                .map(|(index, area)| (RegionKind::Measurement, index, area)),
                        );
                        let mut hit = None;
                        for (kind, index, area) in regions.into_iter().rev() {
                            let screen = region_to_screen(area);
                            let corners = [
                                (ResizeCorner::TopLeft, screen.left_top()),
                                (ResizeCorner::TopRight, screen.right_top()),
                                (ResizeCorner::BottomLeft, screen.left_bottom()),
                                (ResizeCorner::BottomRight, screen.right_bottom()),
                            ];
                            let resize = corners
                                .into_iter()
                                .find(|(_, corner)| corner.distance(position) <= 9.0)
                                .map(|(corner, _)| corner);
                            if resize.is_some() || screen.contains(position) {
                                hit = Some(RegionInteraction {
                                    kind,
                                    index,
                                    original: area,
                                    start: image_position,
                                    resize,
                                });
                                break;
                            }
                        }
                        if let Some(interaction) = hit {
                            self.region_selected = Some((interaction.kind, interaction.index));
                            self.region_interaction = Some(interaction);
                            self.region_drag_start = None;
                        } else {
                            self.region_selected = None;
                            self.region_interaction = None;
                            self.region_drag_start = Some(image_position);
                        }
                    }
                }
            }
            if let (Some(interaction), Some(position)) =
                (self.region_interaction, response.interact_pointer_pos())
            {
                let current = pointer_to_image(position);
                let delta = current - interaction.start;
                let (x, y, width, height) = interaction.original;
                let area = if let Some(corner) = interaction.resize {
                    let mut left = x as f32;
                    let mut top = y as f32;
                    let mut right = (x + width) as f32;
                    let mut bottom = (y + height) as f32;
                    match corner {
                        ResizeCorner::TopLeft => {
                            left += delta.x;
                            top += delta.y;
                        }
                        ResizeCorner::TopRight => {
                            right += delta.x;
                            top += delta.y;
                        }
                        ResizeCorner::BottomLeft => {
                            left += delta.x;
                            bottom += delta.y;
                        }
                        ResizeCorner::BottomRight => {
                            right += delta.x;
                            bottom += delta.y;
                        }
                    }
                    left = left.clamp(0.0, size[0] as f32);
                    right = right.clamp(0.0, size[0] as f32);
                    top = top.clamp(0.0, size[1] as f32);
                    bottom = bottom.clamp(0.0, size[1] as f32);
                    let selection =
                        egui::Rect::from_two_pos(egui::pos2(left, top), egui::pos2(right, bottom));
                    (
                        selection.min.x.floor() as u32,
                        selection.min.y.floor() as u32,
                        selection.width().round().max(2.0) as u32,
                        selection.height().round().max(2.0) as u32,
                    )
                } else {
                    let max_x = size[0] as f32 - width as f32;
                    let max_y = size[1] as f32 - height as f32;
                    (
                        (x as f32 + delta.x).round().clamp(0.0, max_x.max(0.0)) as u32,
                        (y as f32 + delta.y).round().clamp(0.0, max_y.max(0.0)) as u32,
                        width,
                        height,
                    )
                };
                let bounded_x = area.0.min(size[0].saturating_sub(1) as u32);
                let bounded_y = area.1.min(size[1].saturating_sub(1) as u32);
                let bounded = (
                    bounded_x,
                    bounded_y,
                    area.2
                        .min((size[0] as u32).saturating_sub(bounded_x))
                        .max(1),
                    area.3
                        .min((size[1] as u32).saturating_sub(bounded_y))
                        .max(1),
                );
                if bounded.2 >= 3 && bounded.3 >= 3 {
                    self.set_region_value(interaction.kind, interaction.index, bounded);
                }
            }

            let draw_region = |kind: RegionKind,
                               index: usize,
                               region: (u32, u32, u32, u32),
                               color: egui::Color32,
                               label: &str| {
                let screen_rect = region_to_screen(region);
                let selected = self.region_selected == Some((kind, index));
                painter.rect_stroke(
                    screen_rect,
                    0.0,
                    egui::Stroke::new(if selected { 3.0 } else { 2.0 }, color),
                );
                painter.text(
                    screen_rect.left_top() + egui::vec2(3.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(11.0),
                    color,
                );
                if selected {
                    for corner in [
                        screen_rect.left_top(),
                        screen_rect.right_top(),
                        screen_rect.left_bottom(),
                        screen_rect.right_bottom(),
                    ] {
                        painter.rect_filled(
                            egui::Rect::from_center_size(corner, egui::vec2(9.0, 9.0)),
                            1.0,
                            color,
                        );
                    }
                }
            };
            if self.enable_crop {
                draw_region(
                    RegionKind::Screenshot,
                    0,
                    (self.crop_x, self.crop_y, self.crop_w, self.crop_h),
                    egui::Color32::from_rgb(255, 90, 90),
                    "Skärmklipp",
                );
            }
            for (index, region) in self.ocr_regions.iter().copied().enumerate() {
                draw_region(
                    RegionKind::Ocr,
                    index,
                    region,
                    egui::Color32::from_rgb(90, 170, 255),
                    &format!("OCR {}", index + 1),
                );
            }
            for (index, region) in self.measurement_regions.iter().copied().enumerate() {
                draw_region(
                    RegionKind::Measurement,
                    index,
                    region,
                    egui::Color32::from_rgb(255, 210, 70),
                    &format!("Mät {}", index + 1),
                );
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
                if self.region_interaction.take().is_some() {
                    self.region_drag_start = None;
                } else if let (Some(start), Some(position)) = (
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
                                self.region_selected = Some((RegionKind::Screenshot, 0));
                            }
                            "ocr" => {
                                self.enable_ocr = true;
                                self.ocr_regions.push(region);
                                self.region_selected =
                                    Some((RegionKind::Ocr, self.ocr_regions.len() - 1));
                            }
                            _ => {
                                self.measurement_regions.push(region);
                                self.region_selected = Some((
                                    RegionKind::Measurement,
                                    self.measurement_regions.len() - 1,
                                ));
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
        let recording_stopped_by_hotkey = self
            .automation_recorder
            .as_ref()
            .is_some_and(macro_recorder::MacroRecorder::is_finished);
        if recording_stopped_by_hotkey {
            self.stop_macro_recording();
        } else if self.automation_recorder.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

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
                        self.automation_status = if self.automation_dry_run {
                            "Torrkörning av RPA-sekvensen pågår.".to_string()
                        } else {
                            "RPA-sekvensen körs.".to_string()
                        };
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

        let mut presence_stopped = false;
        let mut presence_changed = false;
        if let Some(receiver) = self.presence_receiver.as_ref() {
            while let Ok(event) = receiver.try_recv() {
                match event {
                    presence::Event::Sample { status, ocr_text } => {
                        self.presence_last_ocr = ocr_text;
                        if status == presence::PresenceStatus::Unknown
                            && self.presence_current_status.is_none()
                        {
                            self.presence_status_text =
                                "Väntar på två samstämmiga statusavläsningar…".to_string();
                            continue;
                        }
                        if self.presence_current_status != Some(status) {
                            let now = Local::now();
                            if let Some(started) = self.presence_current_started.take() {
                                if let Some(period) = self.presence_periods.last_mut() {
                                    if period.ended_at.is_none() {
                                        period.duration_seconds = started.elapsed().as_secs();
                                        period.ended_at = Some(now.to_rfc3339());
                                    }
                                }
                            }
                            self.presence_current_status = Some(status);
                            self.presence_current_started = Some(Instant::now());
                            self.presence_periods.push(PresencePeriod {
                                person: self.presence_person.trim().to_string(),
                                status,
                                started_at: now.to_rfc3339(),
                                ended_at: None,
                                duration_seconds: 0,
                            });
                            self.presence_status_text =
                                format!("Status ändrad till {}.", status.label());
                            presence_changed = true;
                        }
                    }
                    presence::Event::Error(error) => {
                        self.presence_status_text = format!("Statuskontroll: {error}");
                    }
                    presence::Event::Stopped => presence_stopped = true,
                }
            }
        }
        if presence_stopped {
            if let Some(started) = self.presence_current_started.take() {
                if let Some(period) = self.presence_periods.last_mut() {
                    if period.ended_at.is_none() {
                        period.duration_seconds = started.elapsed().as_secs();
                        period.ended_at = Some(Local::now().to_rfc3339());
                    }
                }
            }
            self.presence_running = false;
            self.presence_status_text = "Statusövervakningen stoppades.".to_string();
            self.presence_receiver = None;
            self.presence_control_sender = None;
            presence_changed = true;
        } else if self.presence_running {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        if presence_changed {
            if let Err(error) = self.export_presence() {
                self.presence_status_text = error;
            }
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
            self.control_sender = None;
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
                    "presence".to_string(),
                    if self.presence_running {
                        "Teams-status • körs"
                    } else {
                        "Teams-status"
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
        if self.region_editor_active || self.region_result_preview {
            egui::CentralPanel::default().show(ctx, |ui| self.show_preview_panel(ui));
            return;
        }
        if self.active_tab == "mouse" {
            self.show_mouse_tab(ctx);
            return;
        }
        if self.active_tab == "automation" {
            self.show_automation_tab(ctx);
            return;
        }
        if self.active_tab == "presence" {
            self.show_presence_tab(ctx);
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
                    ui.heading("Anteckningar - ASC");
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
                        "Stoppa efterhandsanalys"
                    };
                    if ui
                        .add_sized(
                            [ui.available_width(), 40.0],
                            egui::Button::new(btn_text).fill(egui::Color32::from_rgb(180, 60, 60)),
                        )
                        .clicked()
                    {
                        if self.mode == "live" {
                            self.stop_monitoring();
                        } else {
                            self.stop_offline_analysis();
                        }
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

fn presence_color(status: presence::PresenceStatus) -> egui::Color32 {
    match status {
        presence::PresenceStatus::Available => egui::Color32::from_rgb(50, 180, 90),
        presence::PresenceStatus::Busy => egui::Color32::from_rgb(210, 65, 65),
        presence::PresenceStatus::DoNotDisturb => egui::Color32::from_rgb(150, 35, 45),
        presence::PresenceStatus::Away | presence::PresenceStatus::BeRightBack => {
            egui::Color32::from_rgb(220, 175, 45)
        }
        presence::PresenceStatus::Offline => egui::Color32::from_gray(125),
        presence::PresenceStatus::Unknown => egui::Color32::from_rgb(100, 125, 160),
    }
}

fn is_teams_window(window: &capture::WindowInfo) -> bool {
    let identity = format!("{} {}", window.app_name, window.title).to_lowercase();
    identity.contains("teams")
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours} h {minutes} min")
    } else if minutes > 0 {
        format!("{minutes} min {seconds} s")
    } else {
        format!("{seconds} s")
    }
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

    let mut csv = String::from(
        "timestamp,file_name,pixel_diff_percent,changed_pixels,change_x,change_y,change_width,change_height,change_summary,ocr_text,is_changed\n",
    );
    for item in logs {
        let (x, y, width, height) = item
            .change_bounds
            .map(|bounds| {
                (
                    bounds.x.to_string(),
                    bounds.y.to_string(),
                    bounds.width.to_string(),
                    bounds.height.to_string(),
                )
            })
            .unwrap_or_default();
        let _ = writeln!(
            csv,
            "{},{},{:.6},{},{},{},{},{},{},{},{}",
            csv_field(&item.timestamp),
            csv_field(&item.file_name),
            item.pixel_diff * 100.0,
            item.changed_pixels,
            x,
            y,
            width,
            height,
            csv_field(&item.change_summary),
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

fn change_summary(
    changed_pixels: u64,
    bounds: Option<analysis::ChangeBounds>,
    ocr_changed: bool,
) -> String {
    let pixel_summary = match bounds {
        Some(bounds) => format!(
            "{changed_pixels} px vid x={}, y={} ({} × {})",
            bounds.x, bounds.y, bounds.width, bounds.height
        ),
        None => "Inga bildpixlar ändrades".to_string(),
    };
    if ocr_changed {
        format!("OCR-text ändrad; {pixel_summary}")
    } else {
        pixel_summary
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Anteckningar - ASC")
            .with_inner_size([1024.0, 768.0])
            .with_resizable(true)
            .with_maximize_button(true),
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
    use super::{
        csv_field, stop_requested, write_analysis, ControlMessage, KeywordColorResult, LogItem,
    };
    use std::fs;
    use std::sync::mpsc::channel;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn csv_fields_escape_quotes() {
        assert_eq!(csv_field("text, \"citat\""), "\"text, \"\"citat\"\"\"");
    }

    #[test]
    fn stop_signal_is_detected_by_worker() {
        let (sender, receiver) = channel();
        assert!(!stop_requested(&receiver));
        sender
            .send(ControlMessage::Stop)
            .expect("stoppmeddelandet ska kunna skickas");
        assert!(stop_requested(&receiver));
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
            change_bounds: Some(crate::analysis::ChangeBounds {
                x: 10,
                y: 20,
                width: 4,
                height: 5,
            }),
            change_summary: "42 px vid x=10, y=20 (4 × 5)".to_string(),
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
        assert!(csv.contains(",10,20,4,5,"));
        assert!(csv.contains("42 px vid x=10"));
        assert!(csv.contains("\"Rad 1, \"\"test\"\"\""));
        assert!(json.contains("\"is_changed\": true"));
        assert!(json.contains("\"change_bounds\""));
        let keyword_csv = fs::read_to_string(directory.join("asc-keyword-colors.csv"))
            .expect("ord-/färg-CSV ska finnas");
        assert!(keyword_csv.contains("\"öppet\",grön"));

        fs::remove_dir_all(directory).expect("testmappen ska kunna tas bort");
    }
}
