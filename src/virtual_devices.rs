//! Asistente de dispositivos virtuales: detecta los dispositivos de audio virtuales instalados,
//! propone a qué función va cada uno y qué dispositivos reales usar.
//!
//! Rust Sonar no trae driver propio (un driver sin firma de Microsoft exige el modo de prueba de
//! Windows). Usa los dispositivos virtuales firmados que ya haya en el equipo, hablando con ellos
//! solo por la API pública de audio de Windows:
//!
//! - **SteelSeries Sonar Virtual Audio Device** (se instala gratis con SteelSeries GG): trae justo
//!   Gaming / Chat / Media / Aux y un par de micrófono.
//! - **VB-CABLE**, **Voicemeeter** u otros cables virtuales.
//!
//! Lógica pura (sin Windows) para poder probarla con tests.

use crate::config::{Channel, Settings};
use crate::state::DeviceInfo;

/// Funciones que necesitan un dispositivo virtual: los 4 canales y el micrófono.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Channel(Channel),
    Mic,
}

impl Role {
    pub const ALL: [Role; 5] = [
        Role::Channel(Channel::Game),
        Role::Channel(Channel::Chat),
        Role::Channel(Channel::Media),
        Role::Channel(Channel::Aux),
        Role::Mic,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Role::Channel(ch) => ch.label(),
            Role::Mic => tr!("Micrófono", "Microphone"),
        }
    }

    /// Nombre que se pone a la salida virtual en Windows al renombrar.
    pub fn device_name(self) -> String {
        match self {
            Role::Channel(ch) => format!("Rust Sonar - {}", ch.label()),
            // Lado de reproducción del cable del micro: aquí escribe Rust Sonar.
            Role::Mic => tr!("Rust Sonar - Envío micro", "Rust Sonar - Mic Send").into(),
        }
    }

    /// Trozos de nombre que identifican un dispositivo hecho para esta función.
    fn keywords(self) -> &'static [&'static str] {
        match self {
            Role::Channel(Channel::Game) => &["rust sonar - juego", "rust sonar - game", "- gaming", "- game"],
            Role::Channel(Channel::Chat) => &["rust sonar - chat", "- chat"],
            Role::Channel(Channel::Media) => &["rust sonar - media", "- media"],
            Role::Channel(Channel::Aux) => &["rust sonar - aux", "- aux"],
            Role::Mic => &["rust sonar - envío micro", "rust sonar - mic send", "- microphone", "- mic"],
        }
    }

    pub fn get(self, s: &Settings) -> Option<String> {
        match self {
            Role::Channel(ch) => s.channel(ch).device.clone(),
            Role::Mic => s.mic.output_device.clone(),
        }
    }

    pub fn set(self, s: &mut Settings, id: Option<String>) {
        match self {
            Role::Channel(ch) => s.channel_mut(ch).device = id,
            Role::Mic => s.mic.output_device = id,
        }
    }
}

/// Nombre de la entrada de micrófono virtual que verán Discord, OBS, etc. al renombrar.
pub fn mic_capture_name() -> &'static str {
    tr!("Rust Sonar - Micrófono", "Rust Sonar - Microphone")
}

/// Ejecutables de SteelSeries que usan los mismos dispositivos virtuales.
pub const STEELSERIES_PROCESSES: [&str; 2] = ["steelseriessonar", "steelseriesgg"];

/// ¿Parece un dispositivo virtual (no unos auriculares o un micro de verdad)?
pub fn is_virtual_cable(name: &str) -> bool {
    let n = name.to_lowercase();
    const MARKS: [&str; 9] = [
        "vb-audio",
        "voicemeeter",
        "steelseries sonar",
        "virtual audio",
        "virtual cable",
        "hi-fi cable",
        "steam streaming",
        "rust sonar - ",
        "nvidia broadcast",
    ];
    n.starts_with("cable") || MARKS.iter().any(|m| n.contains(m))
}

/// ¿Es uno de los dispositivos virtuales de SteelSeries Sonar?
pub fn is_steelseries(name: &str) -> bool {
    name.to_lowercase().contains("steelseries sonar")
}

/// Texto del último paréntesis: "CABLE-A Input (VB-Audio Cable A)" -> "VB-Audio Cable A".
/// Es el nombre del driver, que comparten la salida y la entrada de un mismo cable.
fn interface_name(name: &str) -> Option<&str> {
    let open = name.rfind('(')?;
    let close = name[open..].find(')')? + open;
    Some(name[open + 1..close].trim())
}

/// Propone un dispositivo para cada función. Primero los que están hechos para esa función
/// ("… - Gaming", "Rust Sonar - Chat"…) y luego reparte el resto de cables en orden.
pub fn auto_assign(outputs: &[DeviceInfo]) -> Vec<(Role, Option<String>)> {
    let mut cables: Vec<&DeviceInfo> = outputs.iter().filter(|d| is_virtual_cable(&d.name)).collect();
    // VB-CABLE trae dos entradas del MISMO cable ("CABLE Input" y "CABLE In 16ch"): las dos salen
    // por "CABLE Output". Si una fuera el micro y la otra un canal, el canal sonaría en el micro.
    // Nos quedamos solo con la normal.
    let twins: Vec<String> = cables
        .iter()
        .filter(|d| {
            d.name.to_lowercase().contains("16ch")
                && cables.iter().any(|o| o.id != d.id && interface_name(&o.name) == interface_name(&d.name))
        })
        .map(|d| d.id.clone())
        .collect();
    cables.retain(|d| !twins.contains(&d.id));
    cables.sort_by(|a, b| a.name.cmp(&b.name));

    let mut result: Vec<(Role, Option<String>)> = Role::ALL.iter().map(|&r| (r, None)).collect();
    let mut used: Vec<String> = Vec::new();

    for (role, slot) in result.iter_mut() {
        let found = role.keywords().iter().find_map(|k| {
            cables.iter().find(|d| !used.contains(&d.id) && d.name.to_lowercase().contains(k))
        });
        if let Some(d) = found {
            *slot = Some(d.id.clone());
            used.push(d.id.clone());
        }
    }
    // Los dispositivos con nombre de otra función no se reparten como comodín.
    let named_for_role = |d: &DeviceInfo| {
        let n = d.name.to_lowercase();
        Role::ALL.iter().any(|r| r.keywords().iter().any(|k| n.contains(k)))
    };
    let mut free: Vec<&DeviceInfo> = cables.iter().copied().filter(|d| !used.contains(&d.id) && !named_for_role(d)).collect();

    // El micrófono necesita un cable de verdad (lo que entra por su salida aparece en su entrada):
    // VB-CABLE o Hi-Fi Cable. Las entradas virtuales de Voicemeeter no lo son, así que se reserva
    // primero un cable para el micro y los canales se reparten el resto.
    let mic_rank = |d: &DeviceInfo| {
        let n = d.name.to_lowercase();
        if n.contains("voicemeeter") {
            None
        } else if n.starts_with("cable") {
            Some(0)
        } else if n.contains("cable") {
            Some(1)
        } else {
            Some(2)
        }
    };
    let empty_channels = result.iter().filter(|(role, slot)| *role != Role::Mic && slot.is_none()).count();
    let enough_for_channels = free.len() > empty_channels || free.iter().any(|d| mic_rank(d).is_none());
    if let Some((_, slot)) = result.iter_mut().find(|(role, slot)| *role == Role::Mic && slot.is_none()) {
        // Con pocos cables y todos válidos para canal, los canales van antes que el micro.
        if enough_for_channels {
            let best = free.iter().enumerate().filter_map(|(i, d)| mic_rank(d).map(|r| (r, i))).min();
            if let Some((_, i)) = best {
                *slot = Some(free.remove(i).id.clone());
            }
        }
    }
    let mut free = free.into_iter();
    for (role, slot) in result.iter_mut() {
        if slot.is_none() {
            let next = if *role == Role::Mic { free.find(|d| mic_rank(d).is_some()) } else { free.next() };
            *slot = next.map(|d| d.id.clone());
        }
    }
    result
}

/// Lado de grabación del mismo cable que `render` (donde aparece lo que escribimos).
pub fn paired_input<'a>(render: &DeviceInfo, inputs: &'a [DeviceInfo]) -> Option<&'a DeviceInfo> {
    let iface = interface_name(&render.name)?;
    inputs.iter().find(|d| is_virtual_cable(&d.name) && interface_name(&d.name) == Some(iface))
}

/// ID de la entrada virtual por la que sale el micro procesado, según la configuración.
pub fn virtual_mic_input(s: &Settings, outputs: &[DeviceInfo], inputs: &[DeviceInfo]) -> Option<String> {
    let id = s.mic.output_device.as_ref()?;
    let render = outputs.iter().find(|d| &d.id == id)?;
    paired_input(render, inputs).map(|d| d.id.clone())
}

/// Elige un dispositivo real (ni virtual ni asignado a una función). Prefiere `preferred` si es
/// real, luego algo que parezca unos auriculares / un micro de cascos, y si no el primero.
pub fn pick_real(devices: &[DeviceInfo], assigned: &[String], preferred: Option<&str>) -> Option<String> {
    let real: Vec<&DeviceInfo> =
        devices.iter().filter(|d| !is_virtual_cable(&d.name) && !assigned.contains(&d.id)).collect();
    if let Some(p) = preferred {
        if real.iter().any(|d| d.id == p) {
            return Some(p.to_string());
        }
    }
    const HEADSET: [&str; 6] = ["headset", "headphone", "auricular", "cascos", "arctis", "usb"];
    real.iter()
        .find(|d| {
            let n = d.name.to_lowercase();
            HEADSET.iter().any(|k| n.contains(k))
        })
        .or(real.first())
        .map(|d| d.id.clone())
}

/// Configuración en un clic: reparte los dispositivos virtuales, elige auriculares y micro reales y
/// activa el micro si tiene cable. `default_out` / `default_in` son los predeterminados actuales de
/// Windows (se respetan si son dispositivos reales). Devuelve un resumen legible.
pub fn auto_setup(
    s: &mut Settings,
    outputs: &[DeviceInfo],
    inputs: &[DeviceInfo],
    default_out: Option<&str>,
    default_in: Option<&str>,
) -> Result<String, String> {
    let plan = auto_assign(outputs);
    let any_virtual = plan.iter().any(|(_, id)| id.is_some());
    for (role, id) in &plan {
        role.set(s, id.clone());
    }
    s.sink_device = None;
    let assigned = s.virtual_device_ids();

    let keep = s.output_device.as_deref().or(default_out);
    s.output_device = pick_real(outputs, &assigned, keep);
    let Some(headphones) = s.output_device.clone() else {
        return Err(tr!("No encuentro unos auriculares o altavoces reales por los que sacar el sonido.", "Can't find real headphones or speakers to play sound through.").into());
    };
    s.manage_defaults = true;

    if !any_virtual {
        // Sin dispositivos virtuales: otra salida real hace de salida silenciosa.
        let sink = outputs.iter().find(|d| !is_virtual_cable(&d.name) && d.id != headphones);
        let Some(sink) = sink else {
            return Err("No hay dispositivos de audio virtuales ni una segunda salida que usar como salida silenciosa. \
                        Instala VB-CABLE (gratis) y vuelve a pulsar."
                .into());
        };
        s.sink_device = Some(sink.id.clone());
        return Ok(format!(
            "modo sin dispositivos virtuales. Las apps sonarán en «{}» (no debe tener altavoces conectados) y Rust Sonar \
             las mezclará en tus auriculares. Sin micrófono virtual.",
            sink.name
        ));
    }

    let channels = Channel::ALL.iter().filter(|ch| s.channel(**ch).device.is_some()).count();
    let mut summary = trf!("{channels} de 4 canales con dispositivo virtual", "{channels} of 4 channels with a virtual device");
    if s.mic.output_device.is_some() {
        let keep = s.mic.input_device.as_deref().or(default_in);
        s.mic.input_device = pick_real(inputs, &[], keep);
        if s.mic.input_device.is_some() {
            s.mic.enabled = true;
            summary.push_str(tr!(", micrófono virtual activado", ", virtual microphone on"));
        }
    }
    summary.push('.');
    Ok(summary)
}

/// Lista de (id, nombre nuevo) para renombrar en Windows según la configuración actual.
pub fn rename_plan(s: &Settings, outputs: &[DeviceInfo], inputs: &[DeviceInfo]) -> Vec<(String, String)> {
    let mut plan = Vec::new();
    for role in Role::ALL {
        let Some(id) = role.get(s) else { continue };
        let Some(dev) = outputs.iter().find(|d| d.id == id) else { continue };
        plan.push((id.clone(), role.device_name()));
        if role == Role::Mic {
            if let Some(input) = paired_input(dev, inputs) {
                plan.push((input.id.clone(), mic_capture_name().into()));
            }
        }
    }
    plan
}

/// Qué ocultar en Windows para que solo queden tus salidas reales, tu micro y los dispositivos de
/// Rust Sonar. Todo lo demás que esté activo (cables y entradas de Voicemeeter sobrantes, los de
/// SteelSeries, otros micros) se oculta. Sin micro configurado se dejan todos los micros reales.
pub fn hide_plan(s: &Settings, outputs: &[DeviceInfo], inputs: &[DeviceInfo]) -> Vec<String> {
    let mut keep = s.virtual_device_ids();
    keep.extend(virtual_mic_input(s, outputs, inputs));
    keep.extend(outputs.iter().filter(|d| !is_virtual_cable(&d.name)).map(|d| d.id.clone()));
    let real_mic = s.mic.input_device.as_ref().filter(|id| inputs.iter().any(|d| &d.id == *id && !is_virtual_cable(&d.name)));
    match real_mic {
        Some(id) => keep.push(id.clone()),
        None => keep.extend(inputs.iter().filter(|d| !is_virtual_cable(&d.name)).map(|d| d.id.clone())),
    }
    outputs.iter().chain(inputs).filter(|d| !keep.contains(&d.id)).map(|d| d.id.clone()).collect()
}

/// «Ordenar dispositivos de Windows»: renombra los de Rust Sonar y oculta el resto, con un solo
/// aviso de administrador. Recuerda lo ocultado en la configuración.
pub fn tidy_windows_devices(shared: &crate::state::Shared) -> anyhow::Result<String> {
    use crate::audio::{routing::DeviceChange, wasapi};
    use windows::Win32::Media::Audio::{eCapture, eRender};
    let outputs = wasapi::list_devices(eRender)?;
    let inputs = wasapi::list_devices(eCapture)?;
    let s = shared.snapshot();
    if Role::ALL.iter().all(|r| r.get(&s).is_none()) {
        anyhow::bail!(trf!("Primero pulsa «Configurar automáticamente»: no hay dispositivos de Rust Sonar asignados.", "Press “Set up automatically” first: no Rust Sonar devices are assigned."));
    }
    let renames = rename_plan(&s, &outputs, &inputs);
    let hides = hide_plan(&s, &outputs, &inputs);
    let mut changes: Vec<DeviceChange> =
        renames.iter().map(|(id, name)| DeviceChange::Rename { id: id.clone(), name: name.clone() }).collect();
    changes.extend(hides.iter().cloned().map(DeviceChange::Hide));
    crate::audio::routing::apply_device_changes_elevated(&changes)?;
    shared.update(|s| {
        // Nombre de fábrica de lo que acabamos de renombrar (si no lo teníamos ya), para el desinstalador.
        for (id, _) in &renames {
            let original = outputs.iter().chain(&inputs).find(|d| &d.id == id).and_then(|d| original_name(&d.name));
            if let Some(name) = original {
                s.renamed_devices.entry(id.clone()).or_insert(name);
            }
        }
        for id in &hides {
            if !s.hidden_devices.contains(id) {
                s.hidden_devices.push(id.clone());
            }
        }
    });
    Ok(trf!("{} dispositivos renombrados y {} ocultos.", "{} devices renamed and {} hidden.", renames.len(), hides.len()))
}

/// Vuelve a mostrar en Windows lo que ocultó `tidy_windows_devices`.
pub fn show_hidden_devices(shared: &crate::state::Shared) -> anyhow::Result<String> {
    use crate::audio::routing::DeviceChange;
    let hidden = shared.snapshot().hidden_devices;
    let changes: Vec<DeviceChange> = hidden.iter().cloned().map(DeviceChange::Show).collect();
    crate::audio::routing::apply_device_changes_elevated(&changes)?;
    shared.update(|s| s.hidden_devices.clear());
    Ok(trf!("{} dispositivos vuelven a verse en Windows.", "{} devices are visible in Windows again.", hidden.len()))
}

/// Nombre "de fábrica" a partir del nombre completo: "CABLE Input (VB-Audio Virtual Cable)" ->
/// "CABLE Input". None si ya es uno nuestro ("Rust Sonar - …"): ese no es el original.
fn original_name(full: &str) -> Option<String> {
    let desc = match full.rfind(" (") {
        Some(i) => &full[..i],
        None => full,
    };
    (!desc.starts_with("Rust Sonar - ") && !desc.is_empty()).then(|| desc.to_string())
}

/// Lo que hace el desinstalador antes de borrar los archivos (`rust_sonar.exe --uninstall-cleanup`,
/// ya como administrador): deja Windows como estaba antes de Rust Sonar. Vuelve a mostrar los
/// dispositivos ocultos, les devuelve su nombre, quita la salida forzada de cada app, devuelve los
/// predeterminados a los dispositivos reales y quita el inicio automático. Los drivers no se tocan.
/// Devuelve el número de pasos que fallaron.
pub fn uninstall_cleanup() -> usize {
    use crate::audio::{routing, wasapi};
    use windows::Win32::Media::Audio::{eCapture, eRender};
    let _com = wasapi::ComGuard::init();
    let s = Settings::load();
    let mut errors = 0;
    let mut check = |what: &str, r: anyhow::Result<()>| {
        if let Err(e) = r {
            eprintln!("[desinstalar] {what}: {e:#}");
            errors += 1;
        }
    };
    for id in &s.hidden_devices {
        check("mostrar dispositivo", routing::set_device_visible(id, true));
    }
    for (id, name) in &s.renamed_devices {
        check("devolver nombre", routing::rename_device(id, name));
    }
    check("salidas por app", routing::clear_all_app_outputs());

    let outputs = wasapi::list_devices(eRender).unwrap_or_default();
    let inputs = wasapi::list_devices(eCapture).unwrap_or_default();
    let virtual_ids = s.virtual_device_ids();
    if let Some(real) = pick_real(&outputs, &virtual_ids, s.output_device.as_deref()) {
        for role in routing::ALL_ROLES {
            if wasapi::default_device_id(eRender, role).is_some_and(|id| virtual_ids.contains(&id)) {
                check("salida predeterminada", routing::set_default_device(&real, &[role]));
            }
        }
    }
    if let (Some(virt), Some(real)) = (virtual_mic_input(&s, &outputs, &inputs), pick_real(&inputs, &[], s.mic.input_device.as_deref())) {
        for role in routing::ALL_ROLES {
            if wasapi::default_device_id(eCapture, role).as_deref() == Some(virt.as_str()) {
                check("micrófono predeterminado", routing::set_default_device(&real, &[role]));
            }
        }
    }
    check("inicio automático", crate::system::set_autostart(false));
    errors
}

/// Primer arranque (p. ej. recién instalado y reiniciado): si hay cables virtuales y ninguna
/// función asignada, configura todo solo. Devuelve el resumen si ha configurado algo.
/// Sin cables todavía (drivers sin activar hasta reiniciar) no marca nada y lo intenta en el siguiente arranque.
pub fn first_run_setup(shared: &crate::state::Shared) -> anyhow::Result<Option<String>> {
    use crate::audio::wasapi;
    use windows::Win32::Media::Audio::{eCapture, eConsole, eRender};
    let s = shared.snapshot();
    if s.auto_setup_done {
        return Ok(None);
    }
    if Role::ALL.iter().any(|r| r.get(&s).is_some()) {
        shared.update(|s| s.auto_setup_done = true);
        return Ok(None);
    }
    let outputs = wasapi::list_devices(eRender)?;
    if !outputs.iter().any(|d| is_virtual_cable(&d.name)) {
        return Ok(None);
    }
    let inputs = wasapi::list_devices(eCapture)?;
    let default_out = wasapi::default_device_id(eRender, eConsole);
    let default_in = wasapi::default_device_id(eCapture, eConsole);
    let summary = shared
        .update(|s| {
            let r = auto_setup(s, &outputs, &inputs, default_out.as_deref(), default_in.as_deref());
            s.auto_setup_done = true;
            r
        })
        .map_err(anyhow::Error::msg)?;
    Ok(Some(summary))
}

#[cfg(test)]
mod tests {
    #[test]
    fn original_name_strips_interface_and_ignores_our_names() {
        assert_eq!(original_name("CABLE Input (VB-Audio Virtual Cable)").as_deref(), Some("CABLE Input"));
        assert_eq!(original_name("Voicemeeter In 1 (VB-Audio Voicemeeter VAIO)").as_deref(), Some("Voicemeeter In 1"));
        assert_eq!(original_name("Rust Sonar - Juego (VB-Audio Hi-Fi Cable)"), None);
    }

    #[test]
    fn hide_plan_keeps_only_real_outputs_the_mic_and_rust_sonar_devices() {
        let outs = vec![
            dev("hs", "Altavoces (2- VSG Singularity Gaming Headset)"),
            dev("hd", "Altavoces (High Definition Audio Device)"),
            dev("cable", "CABLE Input (VB-Audio Virtual Cable)"),
            dev("c16", "CABLE In 16ch (VB-Audio Virtual Cable)"),
            dev("hifi", "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)"),
            dev("vm3", "Voicemeeter In 3 (VB-Audio Voicemeeter VAIO)"),
            dev("ss", &format!("SteelSeries Sonar - Gaming {SS}")),
        ];
        let ins = vec![
            dev("mic-hd", "Micrófono (High Definition Audio Device)"),
            dev("mic-hs", "Micrófono (2- VSG Singularity Gaming Headset)"),
            dev("cable-out", "CABLE Output (VB-Audio Virtual Cable)"),
            dev("hifi-out", "Hi-Fi Cable Output (VB-Audio Hi-Fi Cable)"),
            dev("vm-b1", "Voicemeeter Out B1 (VB-Audio Voicemeeter VAIO)"),
        ];
        let mut s = Settings::default();
        Role::Channel(Channel::Game).set(&mut s, Some("hifi".into()));
        Role::Mic.set(&mut s, Some("cable".into()));
        s.mic.input_device = Some("mic-hd".into());
        let mut hide = hide_plan(&s, &outs, &ins);
        hide.sort();
        assert_eq!(hide, ["c16", "hifi-out", "mic-hs", "ss", "vm-b1", "vm3"]);
        // Sin micro elegido no se oculta ningún micro real.
        s.mic.input_device = None;
        assert!(!hide_plan(&s, &outs, &ins).iter().any(|id| id.starts_with("mic-")));
    }

    use super::*;

    fn dev(id: &str, name: &str) -> DeviceInfo {
        DeviceInfo { id: id.into(), name: name.into() }
    }

    fn vb_outputs() -> Vec<DeviceInfo> {
        vec![
            dev("hs", "Altavoces (VSG Singularity Gaming Headset)"),
            dev("c0", "CABLE Input (VB-Audio Virtual Cable)"),
            dev("ca", "CABLE-A Input (VB-Audio Cable A)"),
            dev("cb", "CABLE-B Input (VB-Audio Cable B)"),
            dev("cc", "CABLE-C Input (VB-Audio Cable C)"),
            dev("cd", "CABLE-D Input (VB-Audio Cable D)"),
        ]
    }

    const SS: &str = "(SteelSeries Sonar Virtual Audio Device)";

    fn steelseries_outputs() -> Vec<DeviceInfo> {
        vec![
            dev("hs", "Altavoces (2- VSG Singularity Gaming Headset)"),
            dev("hd", "Altavoces (High Definition Audio Device)"),
            dev("aux", &format!("SteelSeries Sonar - Aux {SS}")),
            dev("chat", &format!("SteelSeries Sonar - Chat {SS}")),
            dev("game", &format!("SteelSeries Sonar - Gaming {SS}")),
            dev("media", &format!("SteelSeries Sonar - Media {SS}")),
            dev("micout", &format!("SteelSeries Sonar - Microphone {SS}")),
        ]
    }

    fn steelseries_inputs() -> Vec<DeviceInfo> {
        vec![
            dev("mic-hs", "Micrófono (2- VSG Singularity Gaming Headset)"),
            dev("mic-hd", "Micrófono (High Definition Audio Device)"),
            dev("mic-virtual", &format!("SteelSeries Sonar - Microphone {SS}")),
        ]
    }

    #[test]
    fn assigns_five_cables_in_order_and_skips_real_devices() {
        let plan = auto_assign(&vb_outputs());
        let ids: Vec<_> = plan.iter().map(|(_, id)| id.clone().unwrap()).collect();
        // El micro se queda el VB-CABLE "principal"; los canales, el resto en orden.
        assert_eq!(ids, ["ca", "cb", "cc", "cd", "c0"]);
    }

    #[test]
    fn free_vb_audio_set_gives_the_mic_a_real_cable() {
        // VB-CABLE + Hi-Fi Cable + Voicemeeter Potato: todo gratuito, 5 salidas.
        let outs = vec![
            dev("hs", "Altavoces (VSG Singularity Gaming Headset)"),
            dev("cable", "CABLE Input (VB-Audio Virtual Cable)"),
            dev("hifi", "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)"),
            dev("vm-aux", "Voicemeeter AUX Input (VB-Audio Voicemeeter AUX VAIO)"),
            dev("vm", "Voicemeeter Input (VB-Audio Voicemeeter VAIO)"),
            dev("vm3", "Voicemeeter VAIO3 Input (VB-Audio Voicemeeter VAIO3)"),
        ];
        let plan = auto_assign(&outs);
        let ids: Vec<_> = plan.iter().map(|(_, id)| id.clone().unwrap()).collect();
        assert_eq!(ids, ["hifi", "vm-aux", "vm", "vm3", "cable"]);
        // Solo con Voicemeeter no hay micro virtual (sus entradas no son cables).
        let plan = auto_assign(&outs[3..]);
        assert_eq!(plan[4].1, None);
        assert_eq!(plan[0].1.as_deref(), Some("vm-aux"));
    }

    #[test]
    fn vb_cable_16ch_twin_is_not_used_as_a_second_cable() {
        // Lo que instala de verdad VB-CABLE + Hi-Fi Cable + Voicemeeter Potato (con VAIO extra).
        let vm = "(VB-Audio Voicemeeter VAIO)";
        let mut outs = vec![
            dev("hs", "Altavoces (2- VSG Singularity Gaming Headset)"),
            dev("c16", "CABLE In 16ch (VB-Audio Virtual Cable)"),
            dev("cable", "CABLE Input (VB-Audio Virtual Cable)"),
            dev("hifi", "Hi-Fi Cable Input (VB-Audio Hi-Fi Cable)"),
            dev("vm-aux", &format!("Voicemeeter AUX Input {vm}")),
            dev("vm", &format!("Voicemeeter Input {vm}")),
        ];
        for i in 1..=5 {
            outs.push(dev(&format!("vm{i}"), &format!("Voicemeeter In {i} {vm}")));
        }
        let plan = auto_assign(&outs);
        let ids: Vec<_> = plan.iter().map(|(_, id)| id.clone().unwrap()).collect();
        assert_eq!(ids[4], "cable");
        assert!(!ids.contains(&"c16".to_string()), "ningún canal puede ir al mismo cable que el micro: {ids:?}");
    }

    #[test]
    fn keeps_already_renamed_devices_in_their_role() {
        let mut outs = vb_outputs();
        outs[3].name = "Rust Sonar - Juego (VB-Audio Cable B)".into();
        let plan = auto_assign(&outs);
        assert_eq!(plan[0], (Role::Channel(Channel::Game), Some("cb".into())));
        assert_eq!(plan[1].1.as_deref(), Some("ca"));
        assert_eq!(plan[4].1.as_deref(), Some("c0"));
    }

    #[test]
    fn missing_cables_leave_roles_empty() {
        let plan = auto_assign(&vb_outputs()[..2]);
        assert_eq!(plan[0].1.as_deref(), Some("c0"));
        assert!(plan[1..].iter().all(|(_, id)| id.is_none()));
    }

    #[test]
    fn steelseries_devices_go_to_their_named_role() {
        let plan = auto_assign(&steelseries_outputs());
        let ids: Vec<_> = plan.iter().map(|(_, id)| id.clone().unwrap()).collect();
        assert_eq!(ids, ["game", "chat", "media", "aux", "micout"]);
    }

    #[test]
    fn named_devices_are_not_used_as_wildcards() {
        // Solo hay "Chat" y "Microphone": Juego no debe quedarse con el del micro.
        let outs: Vec<DeviceInfo> = steelseries_outputs().into_iter().filter(|d| ["chat", "micout"].contains(&d.id.as_str())).collect();
        let plan = auto_assign(&outs);
        assert_eq!(plan[0].1, None);
        assert_eq!(plan[1].1.as_deref(), Some("chat"));
        assert_eq!(plan[4].1.as_deref(), Some("micout"));
    }

    #[test]
    fn auto_setup_picks_real_headset_and_mic() {
        let mut s = Settings::default();
        // El predeterminado de Windows es un dispositivo virtual: no vale como auriculares.
        let msg = auto_setup(&mut s, &steelseries_outputs(), &steelseries_inputs(), Some("game"), Some("mic-virtual")).unwrap();
        assert_eq!(s.output_device.as_deref(), Some("hs"));
        assert_eq!(s.mic.input_device.as_deref(), Some("mic-hs"));
        assert_eq!(s.mic.output_device.as_deref(), Some("micout"));
        assert!(s.mic.enabled);
        assert!(s.virtual_device_problems().is_empty());
        assert!(msg.starts_with("4 de 4"));
        assert_eq!(virtual_mic_input(&s, &steelseries_outputs(), &steelseries_inputs()).as_deref(), Some("mic-virtual"));
    }

    #[test]
    fn auto_setup_respects_a_real_default() {
        let mut s = Settings::default();
        auto_setup(&mut s, &steelseries_outputs(), &steelseries_inputs(), Some("hd"), Some("mic-hd")).unwrap();
        assert_eq!(s.output_device.as_deref(), Some("hd"));
        assert_eq!(s.mic.input_device.as_deref(), Some("mic-hd"));
    }

    #[test]
    fn auto_setup_without_virtual_devices_uses_second_output_as_sink() {
        let mut s = Settings::default();
        s.channel_mut(Channel::Game).device = Some("game".into()); // de una configuración anterior
        auto_setup(&mut s, &steelseries_outputs()[..2], &steelseries_inputs()[..2], Some("hs"), None).unwrap();
        assert_eq!(s.output_device.as_deref(), Some("hs"));
        assert_eq!(s.sink_device.as_deref(), Some("hd"));
        assert!(s.channels.iter().all(|c| c.device.is_none()));
        assert!(s.virtual_device_problems().is_empty());
    }

    #[test]
    fn auto_setup_fails_with_a_single_output() {
        let mut s = Settings::default();
        assert!(auto_setup(&mut s, &steelseries_outputs()[..1], &[], None, None).is_err());
    }

    #[test]
    fn mic_rename_includes_paired_input() {
        let outs = vb_outputs();
        let ins = vec![dev("in-d", "CABLE-D Output (VB-Audio Cable D)"), dev("mic", "Micrófono (USB)")];
        let mut s = Settings::default();
        Role::Mic.set(&mut s, Some("cd".into()));
        Role::Channel(Channel::Game).set(&mut s, Some("c0".into()));
        let plan = rename_plan(&s, &outs, &ins);
        assert!(plan.contains(&("c0".into(), "Rust Sonar - Juego".into())));
        assert!(plan.contains(&("cd".into(), "Rust Sonar - Envío micro".into())));
        assert!(plan.contains(&("in-d".into(), mic_capture_name().into())));
    }
}
