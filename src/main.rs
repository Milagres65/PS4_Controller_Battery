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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tray_item::{IconSource, TrayItem};

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
    // Falls noch keine HidApi-Instanz existiert, einmalig initialisieren
    if api.is_none() {
        if let Ok(new_api) = HidApi::new() {
            *api = Some(new_api);
        }
    }

    let hid = match api.as_mut() {
        Some(h) => h,
        None => return ControllerStatus::Disconnected,
    };

    // Geraeteliste neu einlesen
    let _ = hid.refresh_devices();

    let device = match hid.open(VENDOR_ID, PRODUCT_ID) {
        Ok(dev) => dev,
        Err(_) => return ControllerStatus::Disconnected,
    };

    let mut buf = [0u8; 64];
    match device.read_timeout(&mut buf, HID_READ_TIMEOUT) {
        Ok(n) if n > BATTERY_BYTE => {
            let (percent, charging) = parse_battery(buf[BATTERY_BYTE]);

            // Pruefe Cooldown fuer Windows Toast Notification
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

// ── Hauptprogramm ────────────────────────────────────────────

fn main() {
    // 1. Tray-Icon in der Windows-Taskleiste registrieren
    let mut tray = match TrayItem::new("PS4 Controller: Initialisiere...", IconSource::Resource("tray-default")) {
        Ok(t) => t,
        Err(_) => return,
    };

    // Beenden-Kanal fuer das Menue
    let (exit_tx, exit_rx): (Sender<()>, Receiver<()>) = channel();
    let is_running = Arc::new(AtomicBool::new(true));

    // Rechtsklick-Menue: Info-Zeile & Beenden
    let _ = tray.add_label("PS4 Controller Monitor");
    
    let exit_tx_clone = exit_tx.clone();
    let is_running_clone = Arc::clone(&is_running);
    let _ = tray.add_menu_item("Beenden", move || {
        is_running_clone.store(false, Ordering::SeqCst);
        let _ = exit_tx_clone.send(());
    });

    // Kanal fuer Status-Updates vom Hintergrund-Worker zum Tray
    let (status_tx, status_rx): (Sender<ControllerStatus>, Receiver<ControllerStatus>) = channel();

    // 2. Hintergrund-Thread fuer Controller-Abfrage starten
    let worker_running = Arc::clone(&is_running);
    thread::spawn(move || {
        let mut api: Option<HidApi> = None;
        let mut last_notify: Option<Instant> = None;
        let cooldown_dur = Duration::from_secs(NOTIFY_COOLDOWN);

        while worker_running.load(Ordering::SeqCst) {
            let status = read_controller_status(&mut api, &mut last_notify, &cooldown_dur);
            let _ = status_tx.send(status);

            // Schlafen in kleinen Haeppchen (insgesamt 10s), damit das Beenden sofort reagiert
            for _ in 0..(CHECK_INTERVAL * 2) {
                if !worker_running.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(500));
            }
        }
    });

    // 3. Haupt-Schleife fuer Tray-Aktualisierung
    let mut last_tooltip = String::new();

    loop {
        // Wurde im Kontextmenue "Beenden" geklickt?
        if exit_rx.try_recv().is_ok() || !is_running.load(Ordering::SeqCst) {
            break;
        }

        // Neue Statusmeldung vom Hintergrundthread verarbeiten
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

            // Tooltip nur aktualisieren, wenn sich der Text geaendert hat
            if new_tooltip != last_tooltip {
                let _ = tray.inner_mut().set_tooltip(&new_tooltip);
                last_tooltip = new_tooltip;
            }
        }

        // Hauptthread schlaeft kurz (verhindert Busy-Waiting -> 0 % CPU)
        thread::sleep(Duration::from_millis(200));
    }
}

