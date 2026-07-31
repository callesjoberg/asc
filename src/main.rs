#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod analysis;
mod capture;
mod ocr;

use chrono::Local;
use eframe::egui;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

#[derive(Clone, serde::Serialize)]
struct LogItem {
    timestamp: String,
    pixel_diff: f64,
    ocr_text: String,
    is_changed: bool,
    file_name: String,
}

enum ControlMessage {
    Stop,
}

enum WorkerMessage {
    Log(LogItem),
    Preview(Vec<u8>, usize, usize), // RGBA-pixlar, bredd, höjd
    Error(String),
    OfflineDone(String),
}

struct AscApp {
    // Gränssnittsinställningar
    mode: String,        // "live" eller "offline"
    source_type: String, // "window" eller "screen"
    selected_source_id: u32,
    save_dir: String,
    interval_secs: u64,
    threshold_pct: f64,
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

    // Status och historik
    is_running: bool,
    status_text: String,
    logs: Vec<LogItem>,
    diffs_history: Vec<f64>,
    total_captures: usize,
    total_changes: usize,

    // Källlistor (cachar)
    windows: Vec<capture::WindowInfo>,
    monitors: Vec<capture::MonitorInfo>,

    // Bildförhandsgranskning
    preview_texture: Option<egui::TextureHandle>,
    preview_size: Option<[usize; 2]>,
    preview_zoom: f32,
    preview_pan: egui::Vec2,

    // Trådkommunikation
    log_receiver: Option<Receiver<WorkerMessage>>,
    control_sender: Option<Sender<ControlMessage>>,
}

impl Default for AscApp {
    fn default() -> Self {
        let mut app = Self {
            mode: "live".to_string(),
            source_type: "screen".to_string(),
            selected_source_id: 0,
            save_dir: String::new(),
            interval_secs: 5,
            threshold_pct: 1.0,
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
            is_running: false,
            status_text: "Klar att starta".to_string(),
            logs: Vec::new(),
            diffs_history: Vec::new(),
            total_captures: 0,
            total_changes: 0,
            windows: Vec::new(),
            monitors: Vec::new(),
            preview_texture: None,
            preview_size: None,
            preview_zoom: 1.0,
            preview_pan: egui::Vec2::ZERO,
            log_receiver: None,
            control_sender: None,
        };
        app.refresh_sources();
        app
    }
}

impl AscApp {
    fn export_analysis(&self) -> Result<(), String> {
        if self.save_dir.is_empty() {
            return Ok(());
        }

        write_analysis(Path::new(&self.save_dir), &self.logs)
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

        self.logs.clear();
        self.diffs_history.clear();
        self.total_captures = 0;
        self.total_changes = 0;
        self.preview_texture = None;
        self.preview_size = None;
        self.preview_zoom = 1.0;
        self.preview_pan = egui::Vec2::ZERO;

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

        std::thread::spawn(move || {
            let mut prev_img: Option<image::DynamicImage> = None;
            let mut prev_ocr: Option<String> = None;
            loop {
                // Kontrollera om vi ska stoppa tråden
                if let Ok(ControlMessage::Stop) = control_rx.try_recv() {
                    break;
                }

                // Ta skärmklipp
                match capture::capture_source(&source_type, source_id, crop_area) {
                    Ok(img) => {
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
                        if let Some(ref p_img) = prev_img {
                            diff = analysis::compare_images(p_img, &img);
                        }

                        // OCR (Textigenkänning)
                        let mut ocr_text = String::new();
                        if enable_ocr {
                            let ocr_img = if let Some(area) = ocr_area {
                                capture::crop_image(&img, area)
                            } else {
                                img.clone()
                            };

                            let temp_path = std::env::temp_dir().join(format!(
                                "asc_ocr_{}_{}.png",
                                std::process::id(),
                                file_timestamp
                            ));
                            if ocr_img.save(&temp_path).is_ok() {
                                match ocr::run_ocr(&temp_path.to_string_lossy()) {
                                    Ok(text) => ocr_text = text,
                                    Err(error) => ocr_text = format!("OCR-fel: {error}"),
                                }
                                let _ = fs::remove_file(temp_path);
                            }
                        }

                        let ocr_changed = enable_ocr
                            && prev_ocr
                                .as_ref()
                                .is_some_and(|previous| ocr_text.trim() != previous.trim());
                        let is_changed = analysis::change_detected(
                            prev_img.is_some(),
                            diff,
                            threshold,
                            ocr_changed,
                        );

                        prev_img = Some(img.clone());
                        if enable_ocr {
                            prev_ocr = Some(ocr_text.clone());
                        }

                        // Skicka logg
                        let log_item = LogItem {
                            timestamp,
                            pixel_diff: diff,
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

    fn start_offline_analysis(&mut self, ctx: egui::Context) {
        if self.save_dir.is_empty() || !Path::new(&self.save_dir).is_dir() {
            self.status_text = "Fel: Välj en befintlig analysmapp först.".to_string();
            return;
        }

        self.logs.clear();
        self.diffs_history.clear();
        self.total_captures = 0;
        self.total_changes = 0;
        self.preview_texture = None;
        self.preview_size = None;
        self.preview_zoom = 1.0;
        self.preview_pan = egui::Vec2::ZERO;

        let (log_tx, log_rx) = channel();
        self.log_receiver = Some(log_rx);
        self.is_running = true;
        self.status_text = "Kör offline-analys...".to_string();

        let save_dir = self.save_dir.clone();
        let threshold = self.threshold_pct / 100.0;
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
            let mut prev_ocr: Option<String> = None;
            for (idx, path) in entries.iter().enumerate() {
                match image::open(path) {
                    Ok(original_img) => {
                        let img = if let Some(area) = crop_area {
                            capture::crop_image(&original_img, area)
                        } else {
                            original_img
                        };
                        let file_name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let timestamp = format!("#{}", idx + 1);

                        // Beräkna bildskillnad
                        let mut diff = 0.0;
                        if let Some(ref p_img) = prev_img {
                            diff = analysis::compare_images(p_img, &img);
                        }

                        // OCR (Textigenkänning)
                        let mut ocr_text = String::new();
                        if enable_ocr {
                            let ocr_img = if let Some(area) = ocr_area {
                                capture::crop_image(&img, area)
                            } else {
                                img.clone()
                            };

                            let temp_path = std::env::temp_dir().join(format!(
                                "asc_ocr_{}_{}.png",
                                std::process::id(),
                                idx
                            ));
                            if ocr_img.save(&temp_path).is_ok() {
                                match ocr::run_ocr(&temp_path.to_string_lossy()) {
                                    Ok(text) => ocr_text = text,
                                    Err(error) => ocr_text = format!("OCR-fel: {error}"),
                                }
                                let _ = fs::remove_file(temp_path);
                            }
                        }

                        let ocr_changed = enable_ocr
                            && prev_ocr
                                .as_ref()
                                .is_some_and(|previous| ocr_text.trim() != previous.trim());
                        let is_changed = analysis::change_detected(
                            prev_img.is_some(),
                            diff,
                            threshold,
                            ocr_changed,
                        );

                        prev_img = Some(img.clone());
                        if enable_ocr {
                            prev_ocr = Some(ocr_text.clone());
                        }

                        // Skicka logg
                        let log_item = LogItem {
                            timestamp,
                            pixel_diff: diff,
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
    fn show_log_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Händelselogg:");
            if ui.button("Rensa logg").clicked() {
                self.logs.clear();
                self.diffs_history.clear();
                self.total_captures = 0;
                self.total_changes = 0;
                if let Err(error) = self.export_analysis() {
                    self.status_text = format!("Exportfel: {error}");
                }
            }
        });

        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            egui::Grid::new("log_grid")
                .striped(true)
                .num_columns(4)
                .spacing([10.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Tid");
                    ui.label("Diff %");
                    ui.label("OCR-text");
                    ui.label("Status");
                    ui.end_row();

                    for log in self.logs.iter().rev() {
                        ui.label(&log.timestamp).on_hover_text(&log.file_name);
                        ui.label(format!("{:.2}%", log.pixel_diff * 100.0));

                        let short_ocr = if log.ocr_text.chars().count() > 25 {
                            format!("{}...", log.ocr_text.chars().take(25).collect::<String>())
                        } else {
                            log.ocr_text.clone()
                        };
                        ui.label(short_ocr).on_hover_text(&log.ocr_text);

                        if log.is_changed {
                            ui.colored_label(egui::Color32::GREEN, "Ändrad");
                        } else {
                            ui.label("Ingen ändring");
                        }
                        ui.end_row();
                    }
                });
        });
    }

    fn show_preview_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label("Senaste skärmklipp:");
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
            ui.label("Rulla för zoom · dra för panorering · dubbelklicka för Anpassa");
        });
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
        if response.dragged_by(egui::PointerButton::Primary) {
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

        if response.hovered() {
            ui.ctx().set_cursor_icon(if response.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
    }
}

impl eframe::App for AscApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
                        self.preview_texture = Some(ctx.load_texture(
                            "preview_image",
                            color_image,
                            egui::TextureOptions::default(),
                        ));
                        self.preview_size = Some([w, h]);
                    }
                    WorkerMessage::Error(err) => {
                        self.status_text = format!("Fel: {}", err);
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
            if let Err(error) = self.export_analysis() {
                self.status_text = format!("Exportfel: {error}");
            }
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

                ui.add_enabled_ui(!self.is_running, |ui| {
                    // Lägeval
                    ui.label("Läge:");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.mode, "live".to_string(), "Live-övervakning");
                        ui.radio_value(&mut self.mode, "offline".to_string(), "Efterhandsanalys");
                    });
                    ui.add_space(5.0);

                    // Källa
                    if self.mode == "live" {
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
                        if ui.button("Bläddra...").clicked() {
                            if let Some(folder) =
                                rfd::FileDialog::new().set_title("Välj mapp").pick_folder()
                            {
                                self.save_dir = folder.to_string_lossy().to_string();
                            }
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
                        ui.add(egui::Slider::new(&mut self.threshold_pct, 0.1..=10.0).text("%"));
                    });
                    ui.add_space(8.0);

                    // Beskärning
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
                        });
                    }
                    ui.add_space(5.0);

                    // OCR
                    ui.checkbox(&mut self.enable_ocr, "Aktivera textigenkänning (OCR)");
                    if self.enable_ocr {
                        ui.group(|ui| {
                            ui.checkbox(&mut self.enable_ocr_crop, "Beskär OCR-område");
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
                        ui.heading(format!("{:.2}%", avg));
                    });
                });
            });
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

fn write_analysis(directory: &Path, logs: &[LogItem]) -> Result<(), String> {
    if !directory.is_dir() {
        return Err("analysmappen finns inte".to_string());
    }

    let json = serde_json::to_vec_pretty(logs)
        .map_err(|error| format!("kunde inte skapa JSON: {error}"))?;
    fs::write(directory.join("asc-analysis.json"), json)
        .map_err(|error| format!("kunde inte skriva asc-analysis.json: {error}"))?;

    let mut csv = String::from("timestamp,file_name,pixel_diff_percent,ocr_text,is_changed\n");
    for item in logs {
        let _ = writeln!(
            csv,
            "{},{},{:.6},{},{}",
            csv_field(&item.timestamp),
            csv_field(&item.file_name),
            item.pixel_diff * 100.0,
            csv_field(&item.ocr_text),
            item.is_changed
        );
    }
    fs::write(directory.join("asc-analysis.csv"), csv)
        .map_err(|error| format!("kunde inte skriva asc-analysis.csv: {error}"))?;

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
    use super::{csv_field, write_analysis, LogItem};
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
            ocr_text: "Rad 1, \"test\"".to_string(),
            is_changed: true,
            file_name: "capture.png".to_string(),
        }];

        write_analysis(&directory, &logs).expect("analysen ska kunna exporteras");

        let csv =
            fs::read_to_string(directory.join("asc-analysis.csv")).expect("CSV-filen ska finnas");
        let json =
            fs::read_to_string(directory.join("asc-analysis.json")).expect("JSON-filen ska finnas");
        assert!(csv.contains("1.250000"));
        assert!(csv.contains("\"Rad 1, \"\"test\"\"\""));
        assert!(json.contains("\"is_changed\": true"));

        fs::remove_dir_all(directory).expect("testmappen ska kunna tas bort");
    }
}
