use crossbeam_channel::{Receiver, Sender};

use crate::pdf::{start_worker, Command, Event};

/// Top-level tabs in the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Tab {
    Pdf,
    Settings,
}

/// Status of the background Pdfium binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfiumStatus {
    Initializing,
    Ready,
    Error(String),
}

/// Main application state for pdfview.
pub struct PdfViewApp {
    #[allow(dead_code)]
    pub tx_cmd: Sender<Command>,
    pub rx_evt: Receiver<Event>,
    pub pdfium_status: PdfiumStatus,
    #[allow(dead_code)]
    pub current_tab: Tab,
}

impl PdfViewApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Enforce dark UI visuals as specified in GEMINI.md
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let (tx_cmd, rx_evt) = start_worker(cc.egui_ctx.clone());

        Self {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Initializing,
            current_tab: Tab::Pdf,
        }
    }
}

impl eframe::App for PdfViewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // UI thread must never block: non-blocking drain of worker events
        while let Ok(evt) = self.rx_evt.try_recv() {
            match evt {
                Event::PdfiumReady => {
                    self.pdfium_status = PdfiumStatus::Ready;
                }
                Event::PdfiumError(err) => {
                    self.pdfium_status = PdfiumStatus::Error(err);
                }
            }
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                match &self.pdfium_status {
                    PdfiumStatus::Initializing => {
                        ui.heading("Initializing Pdfium...");
                        ui.add_space(10.0);
                        ui.spinner();
                    }
                    PdfiumStatus::Ready => {
                        ui.heading("Pdfium ready");
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Worker thread bound to Pdfium successfully.")
                                .color(egui::Color32::LIGHT_GREEN),
                        );
                    }
                    PdfiumStatus::Error(err) => {
                        ui.heading(
                            egui::RichText::new("Pdfium Not Available")
                                .color(egui::Color32::LIGHT_RED),
                        );
                        ui.add_space(12.0);
                        egui::Frame::group(ui.style())
                            .fill(egui::Color32::from_rgb(30, 30, 30))
                            .show(ui, |ui| {
                                ui.set_max_width(650.0);
                                ui.label(egui::RichText::new(err).monospace());
                            });
                    }
                }
            });
        });
    }
}
