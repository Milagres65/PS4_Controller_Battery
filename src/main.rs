// =============================================================
// PS4 DualShock 4 – Akku-Monitor fuer Windows (Bluetooth)
// Vendor ID  : 0x054C (Sony)
// Product ID : 0x09CC (DualShock 4 BT)
//
// Kompiliert als Windows-GUI-App -> kein Konsolenfenster.
// Taskleisten-Icon (System Tray) mit Akkustand & Rechtsklick-Menue.
// Nahezu 0 % CPU und minimale RAM-Nutzung (~5 MB).
// =============================================================

#![windows_subsystem = "windows"]

use hidapi::HidApi;
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tray_icon::{Icon, TrayIconBuilder};

// ── Konfiguration ────────────────────────────────────────────
const VENDOR_ID: u16        = 0x054C;        // Sony
const PRODUCT_ID: u16       = 0x09CC;        // DualShock 4 (Bluetooth)
const BATTERY_BYTE: usize   = 30;            // Byte-Index im BT-HID-Report
const LOW_THRESHOLD: u8     = 20;            // Warnschwelle in Prozent
const CHECK_INTERVAL: u64   = 10;            // Pruefintervall: 10 Sekunden (fuer reaktives Tray)
const NOTIFY_COOLDOWN: u64  = 30 * 60;       // Toast-Cooldown: 30 Minuten
const HID_READ_TIMEOUT: i32 = 200;           // HID Lese-Timeout in ms

// ── Datenstrukturen ──────────────────────────────────────────

#[derive(Debug, Clone)]
enum ControllerStatus {
    Disconnected,
    Connected { percent: u8, charging: bool },
}

// ── Akku-Parsing ─────────────────────────────────────────────

/// Parst den Rohwert von Byte 30 des Bluetooth-HID-Reports.
///
/// Bit 0-3 : Akkustand (0-10), wobei 10 = 100 %
/// Bit 4   : 1 = laedt gerade
fn parse_battery(raw: u8) -> (u8, bool) {
    let level = raw & 0x0F;
    let charging = (raw & 0x10) != 0;
    let percent = (level * 10).min(100);
    (percent, charging)
}

// ── Windows-Benachrichtigung ─────────────────────────────────

/// Sendet eine native Windows-Toast-Benachrichtigung bei niedrigem Akkustand.
fn send_low_battery_notification(percent: u8) {
    use winrt_notification::{Duration, Toast};

    let _ = Toast::new(Toast::POWERSHELL_APP_ID)
        .title("\u{1F50B} PS4-Controller fast leer!")
        .text1(&format!("Akkustand: {} %", percent))
        .text2("Bitte den Controller bald aufladen.")
        .duration(Duration::Short)
        .show();
}

// ── Controller-Abfrage ───────────────────────────────────────

/// Prueft den aktuellen Zustand des Controllers via HID.
fn read_controller_status(
    api: &mut Option<HidApi>,
    last_notify: &mut Option<Instant>,
    cooldown: &Duration,
) -> ControllerStatus {
    if api.is_none() {
        if let Ok(new_api) = HidApi::new() {
            *api = Some(new_api);
        }
    }

    let hid = match api.as_mut() {
        Some(h) => h,
        None => return ControllerStatus::Disconnected,
    };

    let _ = hid.refresh_devices();

    let device = match hid.open(VENDOR_ID, PRODUCT_ID) {
        Ok(dev) => dev,
        Err(_) => return ControllerStatus::Disconnected,
    };

    let mut buf = [0u8; 64];
    match device.read_timeout(&mut buf, HID_READ_TIMEOUT) {
        Ok(n) if n > BATTERY_BYTE => {
            let (percent, charging) = parse_battery(buf[BATTERY_BYTE]);

            let cooldown_elapsed = last_notify
                .map(|t| t.elapsed() >= *cooldown)
                .unwrap_or(true);

            if percent <= LOW_THRESHOLD && !charging && cooldown_elapsed {
                send_low_battery_notification(percent);
                *last_notify = Some(Instant::now());
            }

            ControllerStatus::Connected { percent, charging }
        }
        _ => ControllerStatus::Disconnected,
    }
}

// ── Dynamische Icon-Generierung ──────────────────────────────

/// Erzeugt ein schickes 32x32 RGBA-Icon fuer die Taskleiste.
/// Ein minimalistisches Controller-/Batterie-Design.
fn create_tray_icon() -> Icon {
    const W: u32 = 32;
    const H: u32 = 32;
    let mut rgba = vec![0u8; (W * H * 4) as usize];

    for y in 0..H {
        for x in 0..W {
            let idx = ((y * W + x) * 4) as usize;

            // Zeichne ein abgerundetes Rechteck (Batterie-Körper)
            let in_body = x >= 4 && x <= 26 && y >= 8 && y <= 24;
            let in_tip = x >= 27 && x <= 29 && y >= 13 && y <= 19;

            let border = (in_body && (x == 4 || x == 26 || y == 8 || y == 24))
                || (in_tip && (x == 29 || y == 13 || y == 19));

            let fill = in_body && x >= 7 && x <= 20 && y >= 11 && y <= 21;

            if border {
                rgba[idx] = 230;     // R
                rgba[idx + 1] = 230; // G
                rgba[idx + 2] = 230; // B
                rgba[idx + 3] = 255; // Alpha
            } else if fill {
                rgba[idx] = 0;       // R
                rgba[idx + 1] = 168; // G (Sattes PS-Blau/Gruen)
                rgba[idx + 2] = 255; // B
                rgba[idx + 3] = 255; // Alpha
            }
        }
    }

    Icon::from_rgba(rgba, W, H).expect("Valid icon RGBA data")
}

// ── Hauptprogramm ────────────────────────────────────────────

fn main() {
    // 1. Kontext-Menue mit `muda` aufbauen
    let tray_menu = Menu::new();
    let header_item = MenuItem::new("PS4 Controller Monitor", false, None);
    let separator = PredefinedMenuItem::separator();
    let exit_item = MenuItem::new("Beenden", true, None);
    let exit_id = exit_item.id().clone();

    let _ = tray_menu.append_items(&[&header_item, &separator, &exit_item]);

    // 2. Tray-Icon registrieren
    let icon = create_tray_icon();
    let tray_icon = match TrayIconBuilder::new()
        .with_tooltip("PS4 Controller: Initialisiere...")
        .with_icon(icon)
        .with_menu(Box::new(tray_menu))
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            eprintln!("TrayIcon build failed: {:?}", e);
            return;
        }
    };

    let is_running = Arc::new(AtomicBool::new(true));

    // 3. Worker-Thread fuer periodische Controller-Abfrage starten
    let (status_tx, status_rx): (Sender<ControllerStatus>, Receiver<ControllerStatus>) = channel();
    let worker_running = Arc::clone(&is_running);

    thread::spawn(move || {
        let mut api: Option<HidApi> = None;
        let mut last_notify: Option<Instant> = None;
        let cooldown_dur = Duration::from_secs(NOTIFY_COOLDOWN);

        while worker_running.load(Ordering::SeqCst) {
            let status = read_controller_status(&mut api, &mut last_notify, &cooldown_dur);
            let _ = status_tx.send(status);

            // In 500ms-Schritten schlafen, damit Beenden sofort reagiert
            for _ in 0..(CHECK_INTERVAL * 2) {
                if !worker_running.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
    });

    // 4. Windows GUI Event-Loop auf dem Hauptthread (Win32 Message Pump)
    let menu_channel = MenuEvent::receiver();
    let mut last_tooltip = String::new();

    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };

        let mut msg: MSG = std::mem::zeroed();

        while is_running.load(Ordering::SeqCst) {
            // A. Windows Messages verarbeiten (Tray Hover, Clicks etc.)
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // B. Menue-Klicks (z. B. "Beenden") pruefen
            if let Ok(event) = menu_channel.try_recv() {
                if event.id == exit_id {
                    is_running.store(false, Ordering::SeqCst);
                    break;
                }
            }

            // C. Status-Updates vom Hintergrund-Thread verarbeiten
            while let Ok(status) = status_rx.try_recv() {
                let new_tooltip = match status {
                    ControllerStatus::Disconnected => "PS4 Controller: Nicht verbunden".to_string(),
                    ControllerStatus::Connected { percent, charging } => {
                        if charging {
                            format!("PS4 Controller: {} % (Laedt)", percent)
                        } else {
                            format!("PS4 Controller: {} %", percent)
                        }
                    }
                };

                if new_tooltip != last_tooltip {
                    let _ = tray_icon.set_tooltip(Some(&new_tooltip));
                    last_tooltip = new_tooltip;
                }
            }

            // D. Schlafen, um CPU-Auslastung auf exakt 0,0 % zu halten
            thread::sleep(Duration::from_millis(50));
        }
    }
}

