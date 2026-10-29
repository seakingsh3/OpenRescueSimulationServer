// src/gui.rs
use eframe::{egui, egui::RichText, App};
use egui::{Align, Color32, Context, Layout, TextEdit, Ui};
use serde_json::json;
use std::time::Instant;
use ureq;

use crate::helperscripts::kamera::{self, CameraSimple, CameraVendor, Protocol, StreamKind};

/// Kleines API-Helperchen: setzt eine Quelle am Server.
/// Erwartet, dass der Server POST /api/set_source mit {slot, kind, url} akzeptiert.
fn api_set_source(base: &str, slot: u8, kind: &str, url: &str) -> std::result::Result<(), String> {
    let endpoint = format!("{}/api/set_source", base.trim_end_matches('/'));
    let body = json!({
        "slot": slot,
        "kind": kind,  // z.B. "camera"
        "url":  url
    });
    let resp = ureq::post(&endpoint)
        .send_json(body)
        .map_err(|e| e.to_string())?;

    if (200..300).contains(&resp.status()) {
        Ok(())
    } else {
        Err(format!("HTTP {} {}", resp.status(), resp.status_text()))
    }
}

/// Dialogdaten für "Kamera hinzufügen"
#[derive(Debug, Clone)]
struct AddCamDialog {
    open: bool,
    slot: u8,

    vendor: CameraVendor,
    protocol: Protocol,
    stream_kind: StreamKind,

    host: String,
    port: u16,
    user: String,
    pass: String,
    channel: u8,

    axis_profile: String,
    rtsp_custom_path: String,

    built_rtsp: Option<String>,
    last_build_err: Option<String>,
}

impl Default for AddCamDialog {
    fn default() -> Self {
        Self {
            open: false,
            slot: 0,
            vendor: CameraVendor::Generic,
            protocol: Protocol::Rtsp,
            stream_kind: StreamKind::Main,

            host: String::new(),
            port: 0,
            user: String::new(),
            pass: String::new(),
            channel: 1,

            axis_profile: "default".into(),
            rtsp_custom_path: String::new(),

            built_rtsp: None,
            last_build_err: None,
        }
    }
}

impl AddCamDialog {
    fn build_cam_url(&mut self) {
        // Mappe GUI-Felder -> helperscripts::kamera::CameraSimple
        let cam = CameraSimple {
            host: self.host.trim().to_string(),
            port: if self.port > 0 { Some(self.port) } else { None },
            username: self.user.trim().to_string(),
            password: self.pass.trim().to_string(),
            channel: Some(self.channel),
            stream: Some(self.stream_kind),
            protocol: Some(self.protocol),
            axis_profile: if self.axis_profile.trim().is_empty() {
                None
            } else {
                Some(self.axis_profile.trim().to_string())
            },
            generic_path: if self.rtsp_custom_path.trim().is_empty() {
                None
            } else {
                Some(self.rtsp_custom_path.trim().to_string())
            },
        };

        match kamera::build_simple(&cam, self.vendor) {
            Ok(u) => {
                self.built_rtsp = Some(u);
                self.last_build_err = None;
            }
            Err(e) => {
                self.built_rtsp = None;
                self.last_build_err = Some(e);
            }
        }
    }
}

struct GuiApp {
    server_base: String,
    #[allow(dead_code)]
    started_at: Instant,
    log: Vec<String>,
    add_cam: AddCamDialog,
}

impl GuiApp {
    fn new(server_base: String) -> Self {
        Self {
            server_base,
            started_at: Instant::now(),
            log: vec!["GUI started.".into()],
            add_cam: AddCamDialog::default(),
        }
    }

    fn log_line(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        if self.log.len() > 5000 {
            let overflow = self.log.len() - 5000;
            self.log.drain(0..overflow);
        }
    }

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading("Open Rescue Simulation Server – Control");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Kamera hinzufügen").clicked() {
                    self.add_cam = AddCamDialog::default();
                    self.add_cam.open = true;
                }
            });
        });
        ui.separator();
    }

    fn log_panel(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .stick_to_bottom(true)
            .id_source("log_scroll")
            .show(ui, |ui| {
                for line in &self.log {
                    ui.monospace(line);
                }
            });
    }

    fn draw_add_camera(&mut self, ctx: &Context) {
        if !self.add_cam.open {
            return;
        }
        let mut open_ref = self.add_cam.open;

        egui::Window::new("Kamera hinzufügen")
            .open(&mut open_ref)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Slot:");
                    let mut s = self.add_cam.slot as i32;
                    ui.add(egui::DragValue::new(&mut s).clamp_range(0..=63));
                    self.add_cam.slot = s as u8;
                });

                ui.separator();

                ui.horizontal_wrapped(|ui| {
                    ui.label("Hersteller:");
                    for v in [
                        CameraVendor::Hikvision,
                        CameraVendor::Dahua,
                        CameraVendor::Axis,
                        CameraVendor::Reolink,
                        CameraVendor::Uniview,
                        CameraVendor::Generic,
                        CameraVendor::Onvif,
                    ] {
                        ui.selectable_value(&mut self.add_cam.vendor, v, vendor_label(&v));
                    }
                });

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Protokoll:");
                    ui.selectable_value(&mut self.add_cam.protocol, Protocol::Rtsp, "RTSP");
                    ui.selectable_value(&mut self.add_cam.protocol, Protocol::Rtmp, "RTMP");
                    ui.separator();
                    ui.label("Stream:");
                    ui.selectable_value(&mut self.add_cam.stream_kind, StreamKind::Main, "Main");
                    ui.selectable_value(&mut self.add_cam.stream_kind, StreamKind::Sub, "Sub");
                });

                ui.separator();

                ui.horizontal(|ui| {
                    ui.label("Host / IP:");
                    ui.add(TextEdit::singleline(&mut self.add_cam.host).hint_text("192.168.1.10"));
                });

                ui.horizontal(|ui| {
                    ui.label("Port:");
                    let mut p = self.add_cam.port as i32;
                    ui.add(egui::DragValue::new(&mut p).clamp_range(0..=65535));
                    self.add_cam.port = p.max(0) as u16;
                });

                ui.horizontal(|ui| {
                    ui.label("User:");
                    ui.add(TextEdit::singleline(&mut self.add_cam.user));
                    ui.label("Pass:");
                    ui.add(TextEdit::singleline(&mut self.add_cam.pass).password(true));
                });

                ui.horizontal(|ui| {
                    ui.label("Kanal:");
                    let mut c = self.add_cam.channel as i32;
                    ui.add(egui::DragValue::new(&mut c).clamp_range(0..=64));
                    self.add_cam.channel = c.max(0) as u8;
                });

                // Vendor-spezifisch
                match self.add_cam.vendor {
                    CameraVendor::Axis => {
                        ui.horizontal(|ui| {
                            ui.label("Axis Profil:");
                            ui.add(TextEdit::singleline(&mut self.add_cam.axis_profile));
                        });
                    }
                    CameraVendor::Generic | CameraVendor::Onvif => {
                        ui.horizontal(|ui| {
                            ui.label("Pfad (optional):");
                            ui.add(
                                TextEdit::singleline(&mut self.add_cam.rtsp_custom_path)
                                    .hint_text("/h264.sdp oder app/stream"),
                            );
                        });
                    }
                    _ => {}
                }

                ui.separator();

                if ui.button("URL bauen").clicked() {
                    self.add_cam.build_cam_url();
                }

                if let Some(err) = &self.add_cam.last_build_err {
                    ui.colored_label(Color32::RED, format!("Fehler: {}", err));
                }

                if let Some(preview) = &self.add_cam.built_rtsp {
                    ui.label(RichText::new("Vorschau:").strong());
                    ui.code(preview);
                }

                ui.separator();

                let can_add = self
                    .add_cam
                    .built_rtsp
                    .as_ref()
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false);

                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_add, egui::Button::new("Hinzufügen"))
                        .clicked()
                    {
                        if let Some(url) = &self.add_cam.built_rtsp {
                            match api_set_source(
                                &self.server_base,
                                self.add_cam.slot,
                                "camera",
                                url,
                            ) {
                                Ok(()) => {
                                    self.log_line(format!(
                                        "[add-cam] Slot {} gesetzt: {}",
                                        self.add_cam.slot, url
                                    ));
                                    self.add_cam.open = false;
                                }
                                Err(e) => {
                                    self.log_line(format!("[add-cam] Fehler: {}", e));
                                }
                            }
                        }
                    }

                    if ui.button("Abbrechen").clicked() {
                        self.add_cam.open = false;
                    }
                });
            });

        self.add_cam.open = open_ref;
    }
}

impl App for GuiApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            self.top_bar(ui);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.log_panel(ui);
        });

        self.draw_add_camera(ctx);
    }
}

fn vendor_label(v: &CameraVendor) -> &'static str {
    match v {
        CameraVendor::Hikvision => "Hikvision",
        CameraVendor::Dahua => "Dahua",
        CameraVendor::Axis => "Axis",
        CameraVendor::Reolink => "Reolink",
        CameraVendor::Uniview => "Uniview",
        CameraVendor::Generic => "Generic",
        CameraVendor::Onvif => "ONVIF",
    }
}

/// Starte die GUI.
pub fn run_gui(server_base: String) -> eframe::Result<()> {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Open Rescue Simulation Server GUI",
        native_options,
        Box::new(move |_cc| Box::new(GuiApp::new(server_base))),
    )
}
