//! Standalone countdown timer for the CoreS3.
#![no_std]
#![no_main]

use cores3_device::{
    Board,
    capabilities::{
        audio::{self, Speaker},
        display::{SCREEN, SIZE},
        touch::TouchEvent,
    },
    synth::SineWave,
    ui::{Canvas, common},
};
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

esp_bootloader_esp_idf::esp_app_desc!();

use cores3_core::timer::{Countdown, PRESETS, Tilt};
const LABELS: [&str; 6] = [
    "30 SECONDS",
    "1 MINUTE",
    "2 MINUTES",
    "5 MINUTES",
    "10 MINUTES",
    "25 MIN / POMODORO",
];

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    let Board {
        mut display,
        mut touch,
        mut imu,
        mut speaker,
        ..
    } = Board::init();
    let mut canvas = Canvas::new(SIZE);
    let mut timer = Countdown::default();
    let mut tilt = Tilt::default();
    let mut chime = 0usize;
    let mut wave = SineWave::new(783.99);
    let mut redraw = true;
    let mut last_second = u64::MAX;
    loop {
        let now = Instant::now().as_millis();
        if timer.update(now) {
            chime = 1;
            redraw = true;
        }
        if let Some(sample) = imu.latest() {
            if !timer.running() && sample.acceleration_m_s2.is_some() {
                if let Some(direction) = tilt.sample(sample.attitude.down, now) {
                    timer.select(direction);
                    redraw = true;
                }
            } else {
                tilt.clear();
            }
        }
        while let Some(event) = touch.next_event() {
            if let TouchEvent::Pressed(p) = event
                && p.y >= 180
            {
                if p.x < 80 {
                    timer.select(-1);
                } else if p.x >= 240 {
                    timer.select(1);
                } else if p.x < 160 {
                    timer.toggle(now);
                    tilt.clear();
                } else {
                    timer.reset();
                    speaker.clear();
                    chime = 0;
                }
                redraw = true;
            }
        }
        let remaining = timer.remaining(now);
        if chime > 0 {
            feed_chime(&mut speaker, &mut wave, &mut chime);
        }
        let second = remaining.div_ceil(1000);
        if second != last_second {
            last_second = second;
            redraw = true;
        }
        if redraw {
            draw(
                &mut canvas,
                timer.preset,
                remaining,
                timer.running(),
                timer.finished,
            );
            canvas.show(&mut display.surface(SCREEN));
            redraw = false;
        }
        Timer::after(Duration::from_millis(10)).await;
    }
}

fn feed_chime(speaker: &mut Speaker, wave: &mut SineWave, position: &mut usize) {
    const NOTE: usize = 3200;
    const TOTAL: usize = NOTE * 3;
    let mut samples = [0i16; 512];
    let count = speaker
        .available_frames()
        .min(256)
        .min(TOTAL - (*position - 1));
    for frame in samples[..count * 2].chunks_exact_mut(2) {
        let i = *position - 1;
        wave.set_frequency([783.99, 987.77, 1174.66][i / NOTE]);
        let edge = (i % NOTE).min(NOTE - 1 - i % NOTE).min(400) as f32 / 400.0;
        frame.fill(wave.next_sample(0.10 * edge));
        *position += 1;
    }
    speaker.write(&samples[..count * audio::CHANNELS]);
    if *position > TOTAL {
        *position = 0;
    }
}

fn draw(canvas: &mut Canvas, preset: usize, remaining: u64, running: bool, finished: bool) {
    let bg = Rgb565::new(2, 5, 8);
    let accent = Rgb565::new(8, 50, 25);
    canvas.clear(bg);
    let area = |x, y, w, h| Rectangle::new(Point::new(x, y), Size::new(w, h));
    common::centered_text(
        canvas,
        area(0, 15, 320, 25),
        if finished { "COMPLETE" } else { LABELS[preset] },
        common::TITLE_FONT,
        accent,
    );
    let seconds = remaining.div_ceil(1000);
    draw_time(canvas, seconds);
    let _ = area(30, 135, 260, 8)
        .into_styled(PrimitiveStyle::with_fill(Rgb565::new(5, 12, 14)))
        .draw(canvas);
    let width = (260 * remaining / PRESETS[preset]) as u32;
    let _ = area(30, 135, width, 8)
        .into_styled(PrimitiveStyle::with_fill(accent))
        .draw(canvas);
    common::centered_text(
        canvas,
        area(0, 180, 80, 60),
        "<",
        common::TITLE_FONT,
        Rgb565::WHITE,
    );
    common::centered_text(
        canvas,
        area(80, 180, 80, 60),
        if running { "PAUSE" } else { "START" },
        common::BODY_FONT,
        Rgb565::WHITE,
    );
    common::centered_text(
        canvas,
        area(160, 180, 80, 60),
        "RESET",
        common::BODY_FONT,
        Rgb565::WHITE,
    );
    common::centered_text(
        canvas,
        area(240, 180, 80, 60),
        ">",
        common::TITLE_FONT,
        Rgb565::WHITE,
    );
}

/// Draw five large seven-segment characters without allocating a framebuffer.
fn draw_time(canvas: &mut Canvas, seconds: u64) {
    const DIGITS: [u8; 10] = [0x3f, 0x06, 0x5b, 0x4f, 0x66, 0x6d, 0x7d, 0x07, 0x7f, 0x6f];
    let minutes = seconds / 60;
    let digits = [
        (minutes / 10) as usize,
        (minutes % 10) as usize,
        (seconds % 60 / 10) as usize,
        (seconds % 10) as usize,
    ];
    let segments = [
        (4, 0, 24, 4),
        (28, 4, 4, 23),
        (28, 31, 4, 23),
        (4, 54, 24, 4),
        (0, 31, 4, 23),
        (0, 4, 4, 23),
        (4, 27, 24, 4),
    ];
    let style = PrimitiveStyle::with_fill(Rgb565::WHITE);
    for (index, digit) in digits.iter().enumerate() {
        let x = 76 + index as i32 * 40 + if index >= 2 { 8 } else { 0 };
        for (bit, &(dx, dy, w, h)) in segments.iter().enumerate() {
            if DIGITS[*digit] & (1 << bit) != 0 {
                let _ = Rectangle::new(Point::new(x + dx, 55 + dy), Size::new(w, h))
                    .into_styled(style)
                    .draw(canvas);
            }
        }
    }
    for y in [72, 93] {
        let _ = Rectangle::new(Point::new(157, y), Size::new(4, 4))
            .into_styled(style)
            .draw(canvas);
    }
}
