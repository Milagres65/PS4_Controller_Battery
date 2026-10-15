// =============================================================
// PS4 DualShock 4 – Akku-Monitor fuer Windows (Bluetooth)
// Vendor ID  : 0x054C (Sony)
// Product ID : 0x09CC (DualShock 4 BT)
//
// Kompiliert als Windows-GUI-App -> kein Konsolenfenster.
// Eine einzige .exe, keine Laufzeitabhaengigkeiten.
// =============================================================

// Verhindert das schwarze Konsolenfenster unter Windows.
#![windows_subsystem = "windows"]

use hidapi::HidApi;
use std::thread;
use std::time::{Duration, Instant};

// ── Konfiguration ────────────────────────────────────────────
const VENDOR_ID: u16       = 0x054C;          // Sony
const PRODUCT_ID: u16      = 0x09CC;          // DualShock 4 (Bluetooth)
const BATTERY_BYTE: usize  = 30;              // Byte-Index im BT-HID-Report
const LOW_THRESHOLD: u8    = 20;              // Warnschwelle in Prozent
const CHECK_INTERVAL: u64  = 5 * 60;         // 5 Minuten in Sekunden
const NOTIFY_COOLDOWN: u64 = 30 * 60;        // 30 Minuten in Sekunden
const HID_READ_TIMEOUT: i32 = 1_000;         // Lese-Timeout in Millisekunden

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

/// Sendet eine native Windows-Toast-Benachrichtigung.
/// Schlaegt die Benachrichtigung fehl, wird der Fehler
/// schweigend ignoriert (Monitor laeuft weiter).
fn send_low_battery_notification(percent: u8) {
    use winrt_notification::{Duration, Toast};

    // POWERSHELL_APP_ID ist immer auf jedem Windows-System
    // registriert und kann daher ohne eigene AppID genutzt werden.
    let _ = Toast::new(Toast::POWERSHELL_APP_ID)
        .title("\u{1F50B} PS4-Controller fast leer!")
        .text1(&format!("Akkustand: {} %", percent))
        .text2("Bitte den Controller bald aufladen.")
        .duration(Duration::Short)
        .show();
}

// ── Hauptschleife ────────────────────────────────────────────

fn main() {
    let check_sleep   = Duration::from_secs(CHECK_INTERVAL);
    let cooldown_dur  = Duration::from_secs(NOTIFY_COOLDOWN);
    let mut last_notify: Option<Instant> = None;

    loop {
        check_battery(&mut last_notify, &cooldown_dur);
        // Hauptschleife schlaeft 5 Minuten -> CPU-Last: nahezu 0 %
        thread::sleep(check_sleep);
    }
}

/// Prueft einmalig den Akkustand und sendet ggf. eine Warnung.
/// Schlaegt das HID-Oeffnen oder -Lesen fehl (Controller aus /
/// Bluetooth getrennt), wird der Fehler einfach ignoriert.
fn check_battery(last_notify: &mut Option<Instant>, cooldown: &Duration) {
    // HidApi pro Pruefzyklus neu initialisieren – guenstig,
    // weil nur alle 5 Min. aufgerufen.
    let api = match HidApi::new() {
        Ok(api) => api,
        Err(_)  => return,
    };

    let device = match api.open(VENDOR_ID, PRODUCT_ID) {
        Ok(dev) => dev,
        Err(_)  => return, // Controller nicht verbunden
    };

    let mut buf = [0u8; 64];

    // read_timeout blockiert maximal 1 Sekunde, dann weiter.
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
        }
        // n == 0 (kein Report im non-blocking) oder Fehler -> ignorieren
        _ => {}
    }
    // device wird hier gedroppt -> HID-Handle geschlossen
}
