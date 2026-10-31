// SPDX-License-Identifier: GPL-3.0-or-later

//! Thin wrappers around KeyOS services: app seed, device backup, haptics, LED.

use std::time::Duration;

use rgb_led::RgbColor;
use slint_keyos_platform::slint::Timer;

security::use_api!();
haptics::use_api!();
rgb_led::use_api!();
backup::use_api!();

/// Where the app seed came from.
pub enum Seed {
    /// The real per-app secret from the security service.
    Device([u8; 32]),
    /// Simulator only: the security service refused, so a fixed dev seed is used and the
    /// UI says so. Never compiled into device builds.
    #[cfg(not(target_os = "xous"))]
    Dev([u8; 32]),
}

impl Seed {
    pub fn bytes(&self) -> [u8; 32] {
        match self {
            Seed::Device(b) => *b,
            #[cfg(not(target_os = "xous"))]
            Seed::Dev(b) => *b,
        }
    }

    pub fn is_dev(&self) -> bool { !matches!(self, Seed::Device(_)) }
}

/// `GetAppSeed` is grant-on-first-use: the security server shows a consent prompt through
/// our GUI connection and blocks until it is answered. Call this only from the UI thread
/// once the event loop is running (a Timer or a callback), never before `ui.run()`
/// (SDK MIGRATIONS.md gotcha #24).
pub fn app_seed() -> Result<Seed, String> {
    match Security::default().app_seed() {
        Ok(seed) => Ok(Seed::Device(*seed.as_bytes())),
        Err(e) => {
            log::error!("app seed unavailable: {e:?}");
            #[cfg(not(target_os = "xous"))]
            {
                log::warn!("simulator: falling back to a fixed DEV seed");
                Ok(Seed::Dev(*b"nip-ping-simulator-dev-seed-v1!!"))
            }
            #[cfg(target_os = "xous")]
            {
                Err(format!("App seed unavailable ({e:?})"))
            }
        }
    }
}

/// Ask KeyOS for a fresh encrypted device backup. Blocks until it answers: call it off the UI
/// thread.
pub fn create_backup() -> Result<(), String> { BackupApi::default().create_backup().map_err(|e| e.to_string()) }

fn flash(color: RgbColor, secs: u64) {
    RgbApi::default().set_all_to(color);
    Timer::single_shot(Duration::from_secs(secs), || RgbApi::default().set_all_to(RgbColor::BLACK));
}

/// A message arrived: double buzz and a Nostr-purple LED for 2 s.
pub fn feedback_message() {
    HapticsApi::default().double_click();
    flash(RgbColor::new(142, 48, 235), 2);
}

/// Something was refused: a red LED for 2 s.
pub fn feedback_err() { flash(RgbColor::new(200, 0, 0), 2); }
