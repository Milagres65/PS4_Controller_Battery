// =============================================================
// PS4 DualShock 4 – Akku-Monitor fuer Windows (Bluetooth & USB)
// Vendor ID  : 0x054C (Sony)
// Product ID : 0x09CC (DualShock 4 V2), 0x05C4 (DualShock 4 V1)
//
// Kompiliert als Windows-GUI-App -> kein Konsolenfenster.
// Dynamisches Taskleisten-Icon (System Tray) mit Akkustand,
// Ladestatus (Blitz), Farbcodierung & Rechtsklick-Menue.
// Nahezu 0 % CPU und minimale RAM-Nutzung (~5 MB).
// =============================================================

#![windows_subsystem = "windows"]

use hidapi::{BusType, HidApi};
use muda::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tray_icon::{Icon, TrayIconBuilder};

// ── Konfiguration ────────────────────────────────────────────
const VENDOR_ID: u16        = 0x054C;        // Sony
const PRODUCT_ID_V2: u16    = 0x09CC;        // DualShock 4 V2
const PRODUCT_ID_V1: u16    = 0x05C4;        // DualShock 4 V1
const USB_BATTERY_BYTE: usize = 30;          // Byte-Index im USB-Report 0x01
const BT_BATTERY_BYTE: usize  = 32;          // Byte-Index im Bluetooth-Report 0x11
const LOW_THRESHOLD: u8     = 20;            // Warnschwelle in Prozent
const CHECK_INTERVAL: u64   = 10;            // Pruefintervall: 10 Sekunden
const NOTIFY_COOLDOWN: u64  = 30 * 60;       // Toast-Cooldown: 30 Minuten
const HID_READ_TIMEOUT: i32 = 200;           // HID Lese-Timeout in ms
const MAX_REPORTS_TO_READ: usize = 10;       // BT kann erst generische Reports liefern

// ── Datenstrukturen ──────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum ControllerStatus {
    Disconnected,
    Connected { percent: u8, charging: bool },
}

// ── Akku-Parsing ─────────────────────────────────────────────

/// Parst den Akkustand aus einem DS4 USB- oder Bluetooth-HID-Report.
///
/// Bit 0-3 : Akkustand (0-10), wobei 10 = 100 % (oder 11 wenn voll)
/// Bit 4-5 : Ladestatus
///   - 0x0 : Entladen (Normaler Akkubetrieb)
///   - 0x1 : Laedt gerade ueber Kabel
///   - 0x2 : Vollstaendig geladen (am Kabel)
fn parse_battery_report(report: &[u8], bus_type: BusType) -> Option<(u8, bool)> {
    let battery_byte = match (bus_type, *report.first()?) {
        (BusType::Usb, 0x01) => USB_BATTERY_BYTE,
        (BusType::Bluetooth, 0x11) => BT_BATTERY_BYTE,
        _ => return None,
    };
    let raw = *report.get(battery_byte)?;
    let level = raw & 0x0F;
    let charge_status = (raw >> 4) & 0x03;
    if level > 11 || charge_status > 2 {
        return None;
    }
    let charging = charge_status == 1 || charge_status == 2;
    let percent = if charge_status == 2 || level >= 10 {
        100
    } else {
        (level * 10).min(100)
    };
    Some((percent, charging))
}

// ── Windows-Benachrichtigung ─────────────────────────────────

/// Sendet eine native Windows-Toast-Benachrichtigung bei niedrigem Akkustand.
fn send_low_battery_notification(percent: u8) {
    use winrt_notification::{Duration, Toast};

    let text = if percent == 0 {
        "Akkustand: Fast leer (< 10 %)".to_string()
    } else {
        format!("Akkustand: {} %", percent)
    };

    let _ = Toast::new(Toast::POWERSHELL_APP_ID)
        .title("\u{1F50B} PS4-Controller fast leer!")
        .text1(&text)
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

    // A VID/PID can expose several HID collections; open each path explicitly.
    let devices: Vec<_> = hid
        .device_list()
        .filter(|info| {
            info.vendor_id() == VENDOR_ID
                && (info.product_id() == PRODUCT_ID_V2 || info.product_id() == PRODUCT_ID_V1)
        })
        .cloned()
        .collect();

    for info in devices {
        let bus_type = info.bus_type();
        let device = match info.open_device(hid) {
            Ok(device) => device,
            Err(_) => continue,
        };
        let mut buf = [0u8; 128];
        let mut battery_status = None;
        for _ in 0..MAX_REPORTS_TO_READ {
            let n = match device.read_timeout(&mut buf, HID_READ_TIMEOUT) {
                Ok(n) => n,
                Err(_) => break,
            };
            battery_status = parse_battery_report(&buf[..n], bus_type);
            if battery_status.is_some() {
                break;
            }
        }
        let (percent, charging) = match battery_status {
            Some(status) => status,
            None => continue,
        };

        let cooldown_elapsed = last_notify
            .map(|t| t.elapsed() >= *cooldown)
            .unwrap_or(true);

        if percent <= LOW_THRESHOLD && !charging && cooldown_elapsed {
            send_low_battery_notification(percent);
            *last_notify = Some(Instant::now());
        }

        return ControllerStatus::Connected { percent, charging };
    }

    ControllerStatus::Disconnected
}

#[cfg(test)]
mod tests {
    use super::parse_battery_report;
    use hidapi::BusType;

    #[test]
    fn parses_bluetooth_battery_from_byte_32() {
        let mut report = [0; 33];
        report[0] = 0x11;
        report[30] = 0;
        report[32] = 0x08;

        assert_eq!(
            parse_battery_report(&report, BusType::Bluetooth),
            Some((80, false))
        );
    }

    #[test]
    fn parses_usb_battery_from_byte_30() {
        let mut report = [0; 31];
        report[0] = 0x01;
        report[30] = 0x05;

        assert_eq!(
            parse_battery_report(&report, BusType::Usb),
            Some((50, false))
        );
    }

    #[test]
    fn recognizes_charging_and_full_states() {
        let mut report = [0; 33];
        report[0] = 0x11;
        report[32] = 0x18;
        assert_eq!(
            parse_battery_report(&report, BusType::Bluetooth),
            Some((80, true))
        );

        report[32] = 0x20;
        assert_eq!(
            parse_battery_report(&report, BusType::Bluetooth),
            Some((100, true))
        );
    }

    #[test]
    fn rejects_short_and_unknown_reports() {
        assert_eq!(parse_battery_report(&[0x11; 32], BusType::Bluetooth), None);
        assert_eq!(parse_battery_report(&[0x02; 40], BusType::Usb), None);
    }

    #[test]
    fn rejects_generic_bluetooth_gamepad_reports() {
        let mut report = [0; 128];
        report[0] = 0x01;

        assert_eq!(parse_battery_report(&report, BusType::Bluetooth), None);
    }

    #[test]
    fn parses_live_bluetooth_report_shape() {
        let mut report = [0; 128];
        report[0] = 0x11;
        report[32] = 0x05;

        assert_eq!(
            parse_battery_report(&report, BusType::Bluetooth),
            Some((50, false))
        );
    }

    #[test]
    fn handles_full_and_rejects_impossible_battery_levels() {
        let mut report = [0; 33];
        report[0] = 0x11;
        report[32] = 0x0B;
        assert_eq!(
            parse_battery_report(&report, BusType::Bluetooth),
            Some((100, false))
        );

        report[32] = 0x0C;
        assert_eq!(parse_battery_report(&report, BusType::Bluetooth), None);
    }
}

// ── Dynamische Icon-Generierung ──────────────────────────────

/// Erzeugt ein dynamisches 32x32 RGBA-Icon fuer die Taskleiste.
/// - Nicht verbunden: Ausgegrauter Rahmen mit roter Diagonale
/// - Akku <= 15 %: Roter Warnrahmen & roter Restbalken
/// - Akku <= 30 %: Weisser Rahmen & oranger Balken
/// - Akku > 30 %: Weisser Rahmen & PS-Blauer Balken
/// - Ladevorgang: Gruener Balken mit sichtbarem gelben Blitz
fn create_tray_icon(percent: u8, charging: bool, connected: bool) -> Icon {
    const W: u32 = 32;
    const H: u32 = 32;
    let mut rgba = vec![0u8; (W * H * 4) as usize];

    let mut set_pixel = |x: u32, y: u32, color: [u8; 4]| {
        if x < W && y < H {
            let idx = ((y * W + x) * 4) as usize;
            rgba[idx] = color[0];
            rgba[idx + 1] = color[1];
            rgba[idx + 2] = color[2];
            rgba[idx + 3] = color[3];
        }
    };

    if !connected {
        // Disconnected: Gedimmte Batterie mit roter Durchstreichung
        let border_color = [140, 140, 140, 160];
        for y in 9..=23 {
            for x in 3..=24 {
                let is_corner = (x == 3 || x == 24) && (y == 9 || y == 23);
                let is_border = x == 3 || x == 24 || y == 9 || y == 23;
                if is_border && !is_corner {
                    set_pixel(x, y, border_color);
                }
            }
        }
        for y in 13..=19 {
            for x in 25..=27 {
                if x == 27 || y == 13 || y == 19 {
                    set_pixel(x, y, border_color);
                }
            }
        }
        for i in 0..11 {
            set_pixel(8 + i, 11 + i, [220, 60, 60, 220]);
            set_pixel(8 + i + 1, 11 + i, [220, 60, 60, 220]);
        }
        return Icon::from_rgba(rgba, W, H).expect("Valid icon RGBA data");
    }

    // Rahmenfarbe: Bei < 15 % Akku wird der gesamte Rahmen rot (Sofort-Alarm)
    let border_color = if percent <= 15 && !charging {
        [240, 50, 50, 255]
    } else {
        [240, 240, 240, 255]
    };

    // Hauptkoerper (Rahmen mit abgerundeten Ecken)
    for y in 9..=23 {
        for x in 3..=24 {
            let is_corner = (x == 3 || x == 24) && (y == 9 || y == 23);
            let is_border = x == 3 || x == 24 || y == 9 || y == 23;
            if is_border && !is_corner {
                set_pixel(x, y, border_color);
            }
        }
    }

    // Batterie-Pluspol (Nippel rechts)
    for y in 13..=19 {
        for x in 25..=27 {
            if x == 27 || y == 13 || y == 19 {
                set_pixel(x, y, border_color);
            }
        }
    }

    // Füllungsbereich: x von 5 bis 22 (18 Pixel Breite), y von 11 bis 21 (11 Pixel Hoehe)
    let fill_width = 18;
    let mut filled_pixels = ((percent as u32 * fill_width + 50) / 100).min(fill_width);
    if percent <= 10 && filled_pixels == 0 {
        // Bei < 10 % (Stufe 0) mindestens 1 Pixel Restanzeige in Rot
        filled_pixels = 1;
    }

    let fill_color = if charging {
        [40, 215, 80, 255]   // Lade-Gruen
    } else if percent <= 15 {
        [240, 50, 50, 255]   // Alarm-Rot
    } else if percent <= 30 {
        [255, 140, 20, 255]  // Warn-Orange
    } else {
        [0, 168, 255, 255]   // PS-Blau
    };

    for y in 11..=21 {
        for x in 5..(5 + filled_pixels) {
            set_pixel(x, y, fill_color);
        }
    }

    // Blitz-Symbol bei Ladevorgang einzeichnen
    if charging {
        let bolt_pixels = [
            (15, 10),
            (14, 11), (15, 11),
            (13, 12), (14, 12),
            (12, 13), (13, 13),
            (10, 14), (11, 14), (12, 14), (13, 14), (14, 14), (15, 14), (16, 14), (17, 14),
            (14, 15), (15, 15),
            (13, 16), (14, 16),
            (12, 17), (13, 17),
            (12, 18),
            (11, 19),
        ];

        // Dunkle Kontur fuer maximale Lesbarkeit
        for &(bx, by) in &bolt_pixels {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let nx = (bx as i32 + dx) as u32;
                    let ny = (by as i32 + dy) as u32;
                    if !bolt_pixels.contains(&(nx, ny)) {
                        set_pixel(nx, ny, [20, 20, 20, 240]);
                    }
                }
            }
        }

        // Blitz-Koerper in hellem Elektro-Gelb
        for &(bx, by) in &bolt_pixels {
            set_pixel(bx, by, [255, 240, 0, 255]);
        }
    }

    Icon::from_rgba(rgba, W, H).expect("Valid icon RGBA data")
}

// ── Hauptprogramm ────────────────────────────────────────────
fn main() {
    // 1. Kontext-Menue vorbereiten
    let exit_item = MenuItem::new("Beenden", true, None);
    let exit_id = exit_item.id().clone();

    // 2. Tray-Icon registrieren (mit Retry-Schleife fuer den Windows-Systemstart)
    let mut tray_icon = None;
    for attempt in 0..15 {
        let tray_menu = Menu::new();
        let header_item = MenuItem::new("PS4 Controller Monitor", false, None);
        let separator = PredefinedMenuItem::separator();
        let _ = tray_menu.append_items(&[&header_item, &separator, &exit_item]);

        let initial_icon = create_tray_icon(100, false, false);
        match TrayIconBuilder::new()
            .with_tooltip("PS4 Controller: Initialisiere...")
            .with_icon(initial_icon)
            .with_menu(Box::new(tray_menu))
            .build()
        {
            Ok(t) => {
                tray_icon = Some(t);
                break;
            }
            Err(e) => {
                if attempt == 14 {
                    eprintln!("TrayIcon build failed after 15 attempts: {:?}", e);
                    return;
                }
                thread::sleep(Duration::from_millis(1000));
            }
        }
    }

    let tray_icon = match tray_icon {
        Some(t) => t,
        None => return,
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
    let mut last_status: Option<ControllerStatus> = None;

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
                if Some(&status) != last_status.as_ref() {
                    let (new_tooltip, new_icon) = match &status {
                        ControllerStatus::Disconnected => (
                            "PS4 Controller: Nicht verbunden".to_string(),
                            create_tray_icon(0, false, false),
                        ),
                        ControllerStatus::Connected { percent, charging } => {
                            let tip = if *charging {
                                if *percent >= 100 {
                                    "PS4 Controller: 100 % (Voll geladen)".to_string()
                                } else {
                                    format!("PS4 Controller: {} % (Laedt)", percent)
                                }
                            } else if *percent == 0 {
                                "PS4 Controller: Fast leer (< 10 %)".to_string()
                            } else {
                                format!("PS4 Controller: {} %", percent)
                            };
                            (tip, create_tray_icon(*percent, *charging, true))
                        }
                    };

                    let _ = tray_icon.set_tooltip(Some(&new_tooltip));
                    let _ = tray_icon.set_icon(Some(new_icon));
                    last_status = Some(status);
                }
            }

            // D. Schlafen, um CPU-Auslastung auf exakt 0,0 % zu halten
            thread::sleep(Duration::from_millis(50));
        }
    }
}
