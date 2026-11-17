//! Vista Micrófono: dispositivos, niveles, limpieza de voz con IA, puerta de ruido y EQ.

use super::SonarApp;
use super::eq_editor::eq_editor;
use super::icons;
use super::theme::{self, *};
use super::widgets::{self, card, hmeter, icon_button, section_header, toggle_row};
use crate::config::{EqTarget, Settings};
use eframe::egui::{self, Align, Layout, RichText, Ui};

impl SonarApp {
    pub(super) fn mic_view(&mut self, ui: &mut Ui, s: &mut Settings) {
        let devices = self.shared.devices.read().clone();
        let meters = self.shared.meters.snapshot();
        let status = self.shared.status.read().mic.clone();
        let lvl_in = self.meter_level("mic_in", meters.mic_in);
        let lvl_out = self.meter_level("mic_out_view", meters.mic_out);

        let col_w = ((ui.available_width() - 16.0) / 2.0).max(340.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;

            // Columna izquierda: entrada/salida y niveles.
            ui.vertical(|ui| {
                ui.set_width(col_w);
                card(ui, Some(MIC), false, |ui| {
                    let m = &mut s.mic;
                    toggle_row(ui, &mut m.enabled, tr!("Procesar el micrófono", "Process the microphone"), &status);
                    ui.add_space(10.0);
                    widgets::caption(ui, tr!("Micrófono físico", "Physical microphone"));
                    widgets::device_combo(ui, "mic_in", icons::MICROPHONE, &mut m.input_device, &devices.inputs, tr!("Predeterminado de Windows", "Windows default"), col_w - 40.0);
                    ui.add_space(6.0);
                    widgets::caption(ui, tr!("Salida virtual (lo que oye Discord)", "Virtual output (what Discord hears)"));
                    widgets::device_combo(ui, "mic_out", icons::PLUG, &mut m.output_device, &devices.outputs, tr!("Sin asignar", "Unassigned"), col_w - 40.0);
                    ui.add_space(12.0);

                    meter_row(ui, tr!("Entrada", "Input"), lvl_in, None);
                    meter_row(ui, tr!("Salida", "Output"), lvl_out, Some(MIC));
                    ui.add_space(8.0);

                    egui::Grid::new("mic_levels").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                        ui.label(RichText::new(tr!("Ganancia", "Gain")).color(TEXT_2));
                        ui.add(egui::Slider::new(&mut m.gain_db, -20.0..=30.0).suffix(" dB").fixed_decimals(1));
                        ui.end_row();
                        ui.label(RichText::new(tr!("Volumen", "Volume")).color(TEXT_2));
                        ui.horizontal(|ui| {
                            ui.add(egui::Slider::new(&mut m.volume, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
                            let icon = if m.muted { icons::MICROPHONE_OFF } else { icons::MICROPHONE };
                            if icon_button(ui, icon, if m.muted { tr!("Activar micro", "Unmute mic") } else { tr!("Silenciar micro", "Mute mic") }, m.muted, DANGER).clicked() {
                                m.muted = !m.muted;
                            }
                        });
                        ui.end_row();
                    });
                });
            });

            // Columna derecha: procesado.
            ui.vertical(|ui| {
                ui.set_width(col_w);
                card(ui, None, false, |ui| {
                    section_header(ui, icons::SPARKLE, tr!("Limpieza de voz con IA", "AI voice cleanup"), Some(tr!("Red neuronal RNNoise ejecutándose en local", "RNNoise neural network running locally")));
                    let m = &mut s.mic;
                    toggle_row(ui, &mut m.noise_suppression, tr!("Supresión de ruido", "Noise suppression"), tr!("Elimina teclado, ventiladores y ruido de fondo", "Removes keyboard, fans and background noise"));
                    ui.add_enabled_ui(m.noise_suppression, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tr!("Intensidad", "Strength")).size(12.5).color(TEXT_2));
                            for level in crate::config::Suppression::ALL {
                                let on = m.suppression == level;
                                let text = RichText::new(level.label()).font(theme::medium(12.5)).color(if on { BG } else { TEXT_2 });
                                let b = egui::Button::new(text)
                                    .fill(if on { ACCENT } else { SURFACE_2 })
                                    .stroke(egui::Stroke::new(1.0, if on { ACCENT } else { BORDER }))
                                    .corner_radius(egui::CornerRadius::same(8));
                                if ui.add(b).on_hover_text(level.hint()).clicked() {
                                    m.suppression = level;
                                }
                            }
                        });
                        ui.label(RichText::new(m.suppression.hint()).size(12.0).color(TEXT_3));
                    });
                    ui.add_space(6.0);
                    toggle_row(ui, &mut m.vad_gate, tr!("Cortar cuando no hablo", "Cut when I'm not talking"), tr!("Silencia el micro si no detecta voz", "Mutes the mic when no voice is detected"));
                    ui.add_enabled_ui(m.vad_gate, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tr!("Umbral", "Threshold")).size(12.5).color(TEXT_2));
                            ui.add(egui::Slider::new(&mut m.vad_threshold, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
                        });
                    });
                    ui.add_space(4.0);
                    let vad = meters.mic_vad;
                    let speaking = vad >= m.vad_threshold;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(tr!("Voz detectada", "Voice detected")).size(12.5).color(TEXT_2));
                        hmeter(ui, vad, 180.0, Some(if speaking { ACCENT } else { TEXT_3 }));
                        if speaking {
                            widgets::pill(ui, tr!("Hablando", "Talking"), ACCENT);
                        }
                    });
                });
                ui.add_space(16.0);
                card(ui, None, false, |ui| {
                    section_header(ui, icons::WAVEFORM, tr!("Dinámica y escucha", "Dynamics and monitoring"), None);
                    let m = &mut s.mic;
                    toggle_row(ui, &mut m.compressor, tr!("Compresor de voz", "Voice compressor"), tr!("Iguala el volumen entre hablar bajo y hablar alto", "Evens out the volume between speaking softly and loudly"));
                    ui.add_space(6.0);
                    toggle_row(ui, &mut m.monitor, tr!("Escucharme", "Hear myself"), tr!("Oye tu micro procesado por los auriculares, para probar cómo suenas", "Hear your processed mic in your headphones to check how you sound"));
                    ui.add_enabled_ui(m.monitor, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(tr!("Volumen", "Volume")).size(12.5).color(TEXT_2));
                            ui.add(egui::Slider::new(&mut m.monitor_volume, 0.0..=1.0).custom_formatter(|v, _| format!("{:.0} %", v * 100.0)));
                        });
                    });
                });
                ui.add_space(16.0);
                card(ui, None, false, |ui| {
                    section_header(ui, icons::FILTER, tr!("Puerta de ruido", "Noise gate"), Some(tr!("Corta la señal por debajo de un nivel", "Cuts the signal below a level")));
                    let m = &mut s.mic;
                    toggle_row(ui, &mut m.gate_enabled, tr!("Activada", "Enabled"), "");
                    ui.add_enabled_ui(m.gate_enabled, |ui| {
                        egui::Grid::new("gate").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                            ui.label(RichText::new(tr!("Umbral", "Threshold")).color(TEXT_2));
                            ui.add(egui::Slider::new(&mut m.gate_threshold_db, -80.0..=-10.0).suffix(" dB").fixed_decimals(0));
                            ui.end_row();
                            ui.label(RichText::new(tr!("Relajación", "Release")).color(TEXT_2));
                            ui.add(egui::Slider::new(&mut m.gate_release_ms, 20.0..=1000.0).suffix(" ms").fixed_decimals(0));
                            ui.end_row();
                        });
                    });
                });
            });
        });

        ui.add_space(16.0);
        if let Some(link) = eq_editor(ui, &mut self.eq_state, s, EqTarget::Mic) {
            self.import_sonar(link, EqTarget::Mic);
        }

        ui.add_space(16.0);
        let virtual_mic = crate::virtual_devices::virtual_mic_input(s, &devices.outputs, &devices.inputs)
            .and_then(|id| devices.inputs.iter().find(|d| d.id == id))
            .map(|d| d.name.split(" (").next().unwrap_or(&d.name).to_string());
        card(ui, None, false, |ui| {
            section_header(ui, icons::HELP, tr!("Usarlo en Discord, OBS o Teams", "Using it in Discord, OBS or Teams"), None);
            let step2 = match &virtual_mic {
                Some(name) => trf!("En Discord → Voz y vídeo → Dispositivo de entrada, elige «{name}» (o «Default»: Rust Sonar lo pone como micrófono predeterminado).", "In Discord → Voice & Video → Input Device, pick “{name}” (or “Default”: Rust Sonar makes it the default microphone)."),
                None => tr!("Cuando haya salida virtual, elige en Discord su micrófono virtual como dispositivo de entrada.", "Once there's a virtual output, pick its virtual microphone as the input device in Discord.").to_string(),
            };
            for (n, text) in [
                tr!("Pulsa «Configurar automáticamente» en Ajustes → Dispositivos virtuales (asigna la salida virtual del micro).", "Press “Set up automatically” in Settings → Virtual devices (it assigns the mic's virtual output)."),
                step2.as_str(),
                tr!("Desactiva la supresión de ruido de Discord para no procesar la voz dos veces.", "Turn off Discord's noise suppression so your voice isn't processed twice."),
            ]
            .iter()
            .enumerate()
            {
                ui.horizontal_wrapped(|ui| {
                    widgets::badge_icon(ui, &(n + 1).to_string(), ACCENT, 22.0);
                    ui.label(RichText::new(*text).color(TEXT_2));
                });
            }
        });
    }
}

fn meter_row(ui: &mut Ui, label: &str, level: f32, color: Option<egui::Color32>) {
    ui.horizontal(|ui| {
        ui.add_sized([60.0, 16.0], egui::Label::new(RichText::new(label).size(12.5).color(TEXT_2)));
        let w = (ui.available_width() - 50.0).max(120.0);
        hmeter(ui, level, w, color);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let db = if level <= 0.0 { "-∞".to_string() } else { format!("{:.0}", level * 60.0 - 60.0) };
            ui.label(RichText::new(format!("{db} dB")).font(theme::medium(11.0)).color(TEXT_3));
        });
    });
}
