//! Coarse AXP2101 fuel-gauge readings, refreshed without blocking the UI.
use crate::board::i2c::SystemI2cBus;
use core::sync::atomic::{AtomicU16, Ordering};
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_hal_async::i2c::I2c;

/// Latest percentage, or 255 when no valid reading is available.
static PERCENT: AtomicU16 = AtomicU16::new(255);

/// Latest fuel-gauge percentage; unavailable when absent or a read fails.
pub fn percent() -> Option<u8> {
    reading().map(|r| r.percent)
}

/// Consistent battery level and charging state from the latest poll.
pub fn reading() -> Option<cores3_core::battery::Reading> {
    let raw = PERCENT.load(Ordering::Relaxed);
    cores3_core::battery::decode(8, if raw & 256 != 0 { 0x20 } else { 0 }, raw as u8)
}

/// Start the shared PMIC fuel-gauge polling task on CPU1.
pub(crate) fn spawn(spawner: &Spawner, bus: SystemI2cBus) {
    spawner.spawn(poll(bus).expect("battery task already spawned"));
}

/// Poll battery presence and state of charge at a low rate.
#[embassy_executor::task]
async fn poll(bus: SystemI2cBus) {
    loop {
        let mut status = [0; 2];
        let mut level = [255];
        let valid = {
            let mut bus = bus.lock().await;
            I2c::write_read(&mut *bus, 0x34, &[0x00], &mut status)
                .await
                .is_ok()
                && status[0] & 8 != 0
                && I2c::write_read(&mut *bus, 0x34, &[0xa4], &mut level)
                    .await
                    .is_ok()
                && level[0] <= 100
        };
        let value = if valid {
            u16::from(level[0]) | if status[1] >> 5 == 1 { 256 } else { 0 }
        } else {
            255
        };
        PERCENT.store(value, Ordering::Relaxed);
        Timer::after(Duration::from_secs(2)).await;
    }
}
