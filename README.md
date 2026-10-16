# PS4 Controller Battery Monitor for Windows 🎮🔋

A lightweight, resource-friendly Windows background utility that monitors the battery level of your **Sony DualShock 4 (PS4)** controller over Bluetooth. Features a **System Tray icon** with real-time status and native Windows Toast notifications when the battery is running low.

---

## ✨ Features

- 🪶 **Ultra-low Resource Footprint:** ~0.0% CPU usage and only ~4-5 MB of RAM. Sleeps in the background and only wakes up periodically to poll HID data.
- 📌 **Windows System Tray Icon:** Hover over the tray icon in the taskbar to immediately see the current battery percentage and charging state (e.g., `PS4 Controller: 80 %` or `PS4 Controller: 50 % (Charging)`).
- 🛑 **Context Menu:** Right-click the tray icon to easily view status or cleanly exit the application with one click.
- 🔔 **Native Windows Notifications:** Sends a non-intrusive Windows Toast notification when the controller drops to **≤ 20%** and is not plugged in.
- ⏱️ **Notification Cooldown:** Reminds you at most once every 30 minutes to prevent alert spam.
- 🔕 **No Annoying Console Window:** Runs silently as a true Windows GUI background process (`#![windows_subsystem = "windows"]`).

---

## 🔍 How It Works (Technical Details)

The DualShock 4 communicates via standard Bluetooth HID (*Human Interface Device*) input reports.

- **Vendor ID:** `0x054C` (Sony Interactive Entertainment)
- **Product ID:** `0x09CC` (DualShock 4 Bluetooth / Wireless Controller)

The battery byte depends on how the controller is connected:

- **USB report `0x01`:** Byte 30
- **Bluetooth report `0x11`:** Byte 32

In either report, the battery byte contains the battery and charging information:
- **Bits 0–3:** Battery level as a discrete integer from `0` to `10` (`level * 10 = percentage`).
- **Bits 4–5:** Charging state (`0` = discharging, `1` = charging, `2` = fully charged).

```text
Battery byte: [Bits 7..6: Reserved] [Bits 5..4: Charge state] [Bits 3..0: Level (0-10)]
```

---

## 🚀 Installation & Usage

### Option 1: Pre-built Binary
1. Download `ps4_battery_monitor.exe` from the latest release.
2. Double-click to start. The icon will appear in your system tray (bottom-right taskbar near the clock).
3. *(Optional Autostart)* Press `Win + R`, type `shell:startup`, and place a shortcut to the `.exe` inside.

### Option 2: Build from Source
Requires [Rust](https://rustup.rs/) installed.

```bash
# Clone the repository
git clone https://github.com/Milagres65/PS4_Controller_Battery.git
cd PS4_Controller_Battery

# Build release binary
cargo build --release
```

The compiled standalone executable will be located at `target/release/ps4_battery_monitor.exe` (approx. 230 KB).

---

## 📄 License
MIT License
