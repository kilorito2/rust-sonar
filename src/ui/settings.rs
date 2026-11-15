//! Vista Ajustes: dispositivos virtuales, aplicaciones, generales y API.

use super::SonarApp;
use super::icons;
use super::theme::{self, *};
use super::widgets::{self, card, primary_button, secondary_button, section_header};
use crate::config::{Channel, Settings};
use crate::state::DeviceInfo;
use crate::virtual_devices::{self as vdev, Role};
use eframe::egui::{self, Align, CornerRadius, Layout, Margin, RichText, Stroke, Ui};

impl SonarApp {
    pub(super) fn settings_view(&mut self, ui: &mut Ui, s: &mut Settings) {
        self.virtual_devices_card(ui, s);
        ui.add_space(16.0);
        self.apps_card(ui, s);
        ui.add_space(16.0);
        self.general_card(ui, s);
        ui.add_space(16.0);
        api_card(ui, s);
    }

    fn virtual_devices_card(&mut self, ui: &mut Ui, s: &mut Settings) {
        let devices = self.shared.devices.read().clone();
        let cables: Vec<DeviceInfo> = devices.outputs.iter().filter(|d| vdev::is_virtual_cable(&d.name)).cloned().collect();
        // Lo que se ofrece en los desplegables: solo virtuales, o todos si se pide.
        let choices = if self.show_all_devices { devices.outputs.clone() } else { cables.clone() };

        card(ui, Some(ACCENT), false, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::VIRTUAL_DEVICES).font(theme::icon(18.0)).color(TEXT_2));
                ui.label(RichText::new(tr!("Dispositivos virtuales", "Virtual devices")).font(semibold(15.0)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (text, color) = match cables.len() {
                        0 => (tr!("Ningún dispositivo virtual", "No virtual devices").to_string(), WARN),
                        n if n < 5 => (trf!("{n} de 5 dispositivos virtuales", "{n} of 5 virtual devices"), WARN),
                        n => (trf!("{n} dispositivos virtuales", "{n} virtual devices"), OK),
                    };
                    widgets::pill(ui, &text, color);
                });
            });
            ui.label(
                RichText::new(
                    tr!(
                        "Cada canal tiene su propia salida en Windows. Las apps suenan en ella, Rust Sonar la captura, \
                         la mezcla y la envía a tus auriculares. Al mover una app de canal, Windows cambia su salida. \
                         El volumen de cada canal es el de su dispositivo en Windows.",
                        "Each channel has its own output in Windows. Apps play into it, Rust Sonar captures it, \
                         mixes it and sends it to your headphones. Moving an app to another channel changes its \
                         output in Windows. Each channel's volume is its device's volume in Windows."
                    ),
                )
                .size(12.5)
                .color(TEXT_3),
            );
            ui.add_space(10.0);

            egui::Grid::new("vdev").num_columns(3).spacing([16.0, 8.0]).show(ui, |ui| {
                for role in Role::ALL {
                    let (icon, color) = match role {
                        Role::Channel(ch) => (theme::channel_icon(ch), channel_color(ch)),
                        Role::Mic => (icons::MICROPHONE, MIC),
                    };
                    ui.horizontal(|ui| {
                        widgets::badge_icon(ui, icon, color, 26.0);
                        ui.allocate_ui_with_layout(egui::vec2(96.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
                            ui.label(RichText::new(role.label()).font(medium(13.5)));
                        });
                    });
                    let mut value = role.get(s);
                    widgets::device_combo(ui, &format!("vdev_{}", role.label()), icons::PLUG, &mut value, &choices, tr!("Sin asignar", "Unassigned"), 320.0);
                    let assigned = value.is_some();
                    role.set(s, value);
                    let hint = match role {
                        Role::Channel(_) if assigned => tr!("Windows enruta aquí sus apps", "Windows routes its apps here").to_string(),
                        Role::Channel(_) => tr!("Captura por aplicación", "Per-app capture").to_string(),
                        Role::Mic => match vdev::virtual_mic_input(s, &devices.outputs, &devices.inputs)
                            .and_then(|id| devices.inputs.iter().find(|d| d.id == id))
                        {
                            Some(input) => trf!("Las apps lo ven como «{}»", "Apps see it as “{}”", input.name.split(" (").next().unwrap_or(&input.name)),
                            None => tr!("Sin micrófono virtual", "No virtual microphone").to_string(),
                        },
                    };
                    ui.label(RichText::new(hint).size(12.0).color(TEXT_3));
                    ui.end_row();
                }
            });

            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                if primary_button(ui, icons::MAGIC, tr!("Configurar automáticamente", "Set up automatically"))
                    .on_hover_text(tr!("Asigna los dispositivos virtuales, elige tus auriculares y tu micro, y activa el micro virtual", "Assigns the virtual devices, picks your headphones and mic, and turns on the virtual mic"))
                    .clicked()
                {
                    self.run_auto_setup();
                }
                let any_assigned = Role::ALL.iter().any(|r| r.get(s).is_some());
                let tidy = secondary_button(ui, icons::TAG, tr!("Ordenar dispositivos de Windows", "Tidy up Windows devices"), any_assigned);
                if tidy
                    .on_hover_text(
                        tr!("Les pone «Rust Sonar - Juego», «- Chat»… y oculta el resto de dispositivos virtuales y micros: en Windows y Discord solo quedan tus salidas, tu micro y los de Rust Sonar. Pide permisos de administrador", "Names them “Rust Sonar - Game”, “- Chat”… and hides the other virtual devices and mics: Windows and Discord only show your outputs, your mic and Rust Sonar's. Asks for administrator rights"),
                    )
                    .clicked()
                {
                    let shared = self.shared.clone();
                    self.run_action(move || vdev::tidy_windows_devices(&shared));
                }
                if !s.hidden_devices.is_empty() {
                    let show = secondary_button(ui, icons::RESET, &trf!("Mostrar los ocultos ({})", "Show hidden ({})", s.hidden_devices.len()), true);
                    if show.on_hover_text(tr!("Vuelve a mostrar en Windows lo que ocultó «Ordenar dispositivos»", "Shows again in Windows what “Tidy up devices” hid")).clicked() {
                        let shared = self.shared.clone();
                        self.run_action(move || vdev::show_hidden_devices(&shared));
                    }
                }
                let reset = secondary_button(ui, icons::RESET, tr!("Restablecer salidas de apps", "Reset app outputs"), true);
                if reset.on_hover_text(tr!("Todas las apps vuelven a la salida predeterminada de Windows", "All apps go back to the Windows default output")).clicked() {
                    self.run_action(|| {
                        crate::audio::routing::clear_all_app_outputs()?;
                        Ok(tr!("Salidas de las apps restablecidas.", "App outputs reset.").into())
                    });
                }
            });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                widgets::badge_icon(ui, icons::SPEAKER_OFF, TEXT_2, 26.0);
                ui.allocate_ui_with_layout(egui::vec2(96.0, 22.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new(tr!("Salida silenciosa", "Silent output")).font(medium(13.5)));
                });
                let headphones = self.shared.active_output.read().clone();
                let real: Vec<DeviceInfo> = devices
                    .outputs
                    .iter()
                    .filter(|d| !vdev::is_virtual_cable(&d.name) && Some(&d.id) != headphones.as_ref())
                    .cloned()
                    .collect();
                widgets::device_combo(ui, "sink_dev", icons::PLUG, &mut s.sink_device, &real, tr!("No usar", "Don't use"), 320.0);
                ui.add(egui::Label::new(RichText::new(tr!("Sin dispositivos virtuales: las apps suenan aquí (sin altavoces) y se capturan una a una", "Without virtual devices: apps play here (no speakers attached) and are captured one by one")).size(12.0).color(TEXT_3)).wrap());
            });
            ui.add_space(6.0);
            ui.checkbox(&mut self.show_all_devices, RichText::new(tr!("Mostrar todos los dispositivos en los desplegables", "Show every device in the drop-downs")).size(12.5).color(TEXT_2));

            for problem in s.virtual_device_problems() {
                ui.add_space(6.0);
                widgets::notice(ui, icons::WARNING, DANGER, &problem);
            }

            ui.add_space(8.0);
            egui::CollapsingHeader::new(RichText::new(trf!("{}  ¿De dónde salen los dispositivos virtuales?", "{}  Where do the virtual devices come from?", icons::HELP)).color(TEXT_2))
                .id_salt("help_cables")
                .show(ui, |ui| {
                    for (n, text) in [
                        tr!("Rust Sonar no puede traer driver propio (Windows exige firma de Microsoft): usa dispositivos virtuales firmados y gratuitos de vb-audio.com.", "Rust Sonar can't ship its own driver (Windows requires a Microsoft signature): it uses free, signed virtual devices from vb-audio.com."),
                        tr!("Instala como administrador VB-CABLE (cable del micrófono), Hi-Fi Cable y Voicemeeter Potato (4 salidas de canal) y reinicia. Están descargados en la carpeta «instaladores» del proyecto.", "Install VB-CABLE (mic cable), Hi-Fi Cable and Voicemeeter Potato (4 channel outputs) as administrator and restart. The installer can set them up for you."),
                        tr!("Los de SteelSeries GG no sirven: solo existen con GG abierto, y entonces Rust Sonar se pone en pausa.", "SteelSeries GG's devices don't work: they only exist while GG is open, and then Rust Sonar pauses."),
                        tr!("Pulsa «Configurar automáticamente» y, si quieres, «Ordenar dispositivos de Windows» para que se llamen «Rust Sonar - Juego», «- Chat»… y desaparezcan los sobrantes.", "Press “Set up automatically” and, optionally, “Tidy up Windows devices” so they're named “Rust Sonar - Game”, “- Chat”… and the leftovers disappear."),
                    ]
                    .iter()
                    .enumerate()
                    {
                        ui.horizontal_wrapped(|ui| {
                            widgets::badge_icon(ui, &(n + 1).to_string(), ACCENT, 22.0);
                            ui.label(RichText::new(*text).color(TEXT_2));
                        });
                    }
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(trf!(
                            "{}  Son drivers firmados: funcionan sin modo de prueba y con anti-cheats como Vanguard.", "{}  They're signed drivers: they work without test mode and with anti-cheats like Vanguard.",
                            icons::SHIELD
                        ))
                        .size(12.0)
                        .color(TEXT_3),
                    );
                });
        });
    }

    fn apps_card(&mut self, ui: &mut Ui, s: &mut Settings) {
        let apps = self.shared.apps.read().clone();
        card(ui, None, false, |ui| {
            section_header(ui, icons::APP, tr!("Aplicaciones", "Apps"), Some(tr!("Apps que están reproduciendo audio ahora mismo", "Apps playing audio right now")));
            if apps.is_empty() {
                ui.label(RichText::new(tr!("Ninguna aplicación está sonando.", "No app is playing.")).color(TEXT_3));
                return;
            }
            egui::Grid::new("apps").striped(false).num_columns(5).spacing([18.0, 8.0]).show(ui, |ui| {
                for h in [tr!("Aplicación", "App"), tr!("Nivel", "Level"), tr!("Canal", "Channel"), tr!("Estado", "Status"), ""] {
                    widgets::caption(ui, h);
                }
                ui.end_row();
                for app in &apps {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icons::APP).font(theme::icon(15.0)).color(TEXT_2));
                        ui.label(RichText::new(&app.name).font(medium(13.5)));
                        ui.label(RichText::new(format!("PID {}", app.pid)).size(11.5).color(TEXT_3));
                    });
                    let level = ((crate::dsp::lin_to_db(app.peak) + 60.0) / 60.0).clamp(0.0, 1.0);
                    widgets::hmeter(ui, level, 110.0, Some(channel_color(app.channel)));
                    let mut ch = s.route_for(&app.name);
                    egui::ComboBox::from_id_salt(("route", app.pid))
                        .selected_text(RichText::new(format!("{}  {}", theme::channel_icon(ch), ch.label())).color(channel_color(ch)))
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for c in Channel::ALL {
                                ui.selectable_value(&mut ch, c, format!("{}  {}", theme::channel_icon(c), c.label()));
                            }
                        });
                    if ch != s.route_for(&app.name) {
                        s.app_routes.insert(app.name.to_ascii_lowercase(), ch);
                    }
                    match &app.error {
                        Some(e) => widgets::pill(ui, tr!("Error", "Error"), DANGER).on_hover_text(e),
                        None if app.via_device => widgets::pill(ui, tr!("Enrutada", "Routed"), ACCENT),
                        None if app.direct => widgets::pill(ui, tr!("Directa", "Direct"), WARN)
                            .on_hover_text(tr!("Suena directamente en tus auriculares, fuera del mezclador. Dale un dispositivo virtual a su canal para controlarla", "Plays straight to your headphones, outside the mixer. Give its channel a virtual device to control it")),
                        None => widgets::pill(ui, tr!("Capturando", "Capturing"), OK),
                    };
                    if secondary_button(ui, icons::CLOSE, tr!("Ignorar", "Ignore"), true).on_hover_text(tr!("No capturar nunca esta app", "Never capture this app")).clicked() {
                        s.ignored_apps.insert(app.name.to_ascii_lowercase());
                    }
                    ui.end_row();
                }
            });
        });
    }
}

impl SonarApp {
    fn general_card(&mut self, ui: &mut Ui, s: &mut Settings) {
        card(ui, None, false, |ui| {
        section_header(ui, icons::SETTINGS, tr!("General", "General"), None);
        let mut autostart = self.autostart;
        if widgets::toggle_row(ui, &mut autostart, tr!("Iniciar con Windows", "Start with Windows"), tr!("Arranca oculto en la bandeja del sistema al iniciar sesión", "Starts hidden in the system tray when you sign in")).changed() {
            match crate::system::set_autostart(autostart) {
                Ok(()) => self.autostart = autostart,
                Err(e) => *self.action_msg.lock() = Some((false, format!("{e:#}"))),
            }
        }
        ui.add_space(6.0);
        widgets::toggle_row(ui, &mut s.close_to_tray, tr!("Cerrar a la bandeja", "Close to tray"), tr!("La X esconde la ventana y el audio sigue; para salir, clic derecho en el icono → Salir", "The X hides the window and audio keeps going; to quit, right-click the tray icon → Quit"));
        ui.add_space(6.0);
        widgets::toggle_row(
            ui,
            &mut s.manage_defaults,
            tr!("Gestionar los predeterminados de Windows", "Manage Windows defaults"),
            tr!("Al abrir: Juego como salida y el micro virtual como micrófono. Al salir: vuelven tus auriculares y tu micro", "On start: Game as output and the virtual mic as microphone. On quit: your headphones and mic come back"),
        );
        ui.add_space(12.0);
        egui::Grid::new("general").num_columns(2).spacing([24.0, 10.0]).show(ui, |ui| {
            ui.label(RichText::new(tr!("Idioma · Language", "Language · Idioma")).color(TEXT_2));
            egui::ComboBox::from_id_salt("language").selected_text(s.language.label()).show_ui(ui, |ui| {
                for lang in crate::i18n::Language::ALL {
                    ui.selectable_value(&mut s.language, lang, lang.label());
                }
            });
            ui.end_row();
            ui.label(RichText::new(tr!("Canal para apps nuevas", "Channel for new apps")).color(TEXT_2));
            egui::ComboBox::from_id_salt("default_ch")
                .selected_text(format!("{}  {}", theme::channel_icon(s.default_channel), s.default_channel.label()))
                .show_ui(ui, |ui| {
                    for c in Channel::ALL {
                        ui.selectable_value(&mut s.default_channel, c, format!("{}  {}", theme::channel_icon(c), c.label()));
                    }
                });
            ui.end_row();
            ui.label(RichText::new(tr!("Puerto de la API", "API port")).color(TEXT_2));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut s.api_port).range(1024..=65535));
                ui.label(RichText::new(tr!("se aplica al reiniciar", "applies after restart")).size(12.0).color(TEXT_3));
            });
            ui.end_row();
            ui.label(RichText::new(tr!("Archivo de configuración", "Config file")).color(TEXT_2));
            ui.label(RichText::new(Settings::path().display().to_string()).monospace().color(TEXT_3));
            ui.end_row();
        });

        ui.add_space(12.0);
        widgets::caption(ui, tr!("Aplicaciones ignoradas", "Ignored apps"));
        ui.label(RichText::new(tr!("No se capturan nunca. Evita bucles con otras apps de audio.", "Never captured. Prevents loops with other audio apps.")).size(12.0).color(TEXT_3));
        let mut unignore = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for name in &s.ignored_apps {
                let text = RichText::new(format!("{name}   {}", icons::CLOSE)).size(12.5);
                let chip = egui::Button::new(text)
                    .fill(SURFACE_2)
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(CornerRadius::same(12));
                if ui.add(chip).on_hover_text(tr!("Volver a capturar", "Capture again")).clicked() {
                    unignore = Some(name.clone());
                }
            }
        });
        if let Some(n) = unignore {
            s.ignored_apps.remove(&n);
        }
        ui.add_space(12.0);
        if secondary_button(ui, icons::CLOSE, tr!("Salir de Rust Sonar", "Quit Rust Sonar"), true)
            .on_hover_text(tr!("Cierra el programa y devuelve el sonido a tus auriculares", "Closes the program and sends sound back to your headphones"))
            .clicked()
        {
            crate::request_quit(&self.shared);
        }
        });
    }
}

fn api_card(ui: &mut Ui, s: &Settings) {
    card(ui, None, false, |ui| {
        section_header(ui, icons::BROADCAST, tr!("API local", "Local API"), Some(tr!("Controla Rust Sonar desde Stream Deck, scripts o una web", "Control Rust Sonar from a Stream Deck, scripts or a web page")));
        let port = s.api_port;
        egui::Frame::new()
            .fill(BG)
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (method, path, body) in [
                    ("GET", format!("http://127.0.0.1:{port}/api/state"), ""),
                    ("PATCH", "/api/channels/game".into(), r#"{"volume":0.5,"muted":false}"#),
                    ("PUT", "/api/chatmix".into(), r#"{"value":-0.3}"#),
                    ("PUT", "/api/apps/discord".into(), r#"{"channel":"chat"}"#),
                    ("POST", "/api/channels/mic/eq/preset/Voz clara".into(), ""),
                    ("PATCH", "/api/mic".into(), r#"{"muted":true}"#),
                ] {
                    ui.horizontal(|ui| {
                        let color = match method {
                            "GET" => OK,
                            "POST" => WARN,
                            _ => ACCENT,
                        };
                        ui.add_sized([52.0, 16.0], egui::Label::new(RichText::new(method).monospace().color(color)));
                        ui.label(RichText::new(path).monospace().color(TEXT));
                        ui.label(RichText::new(body).monospace().color(TEXT_3));
                    });
                }
            });
    });
}
