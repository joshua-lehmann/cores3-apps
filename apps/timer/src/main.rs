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
    primitives::{Line, PrimitiveStyle, Rectangle},
};

esp_bootloader_esp_idf::esp_app_desc!();

use cores3_core::timer::{Countdown, HoldRepeat, Intervals, Tilt, step_duration};

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
    let mut intervals = Intervals::default();
    let mut interval_mode = false;
    let mut edit_phase = 0usize;
    let mut hold = HoldRepeat::default();
    let mut chime = 0usize;
    let mut wave = SineWave::new(783.99);
    let mut redraw = true;
    let mut last_second = u64::MAX;
    let mut last_battery = None;
    let mut diagnostics = false;
    let mut last_diagnostic = 0;
    loop {
        let now = Instant::now().as_millis();
        if if interval_mode {
            intervals.update(now)
        } else {
            timer.update(now)
        } {
            chime = 1;
            redraw = true;
        }
        if let Some(sample) = imu.latest() {
            if diagnostics && now.saturating_sub(last_diagnostic) >= 1_000 {
                last_diagnostic = now;
                log::info!(
                    "timer imu roll={:.1} pitch={:.1} down={:?} gyro_deg_s={:?}",
                    sample.attitude.roll_deg,
                    sample.attitude.pitch_deg,
                    sample.attitude.down,
                    sample.angular_velocity_deg_s
                );
            }
            if !(if interval_mode {
                intervals.running()
            } else {
                timer.running()
            }) && sample.acceleration_m_s2.is_some()
            {
                if let Some(direction) = tilt.sample(sample.attitude.down, now) {
                    if interval_mode {
                        adjust_interval(&mut intervals, edit_phase, direction);
                    } else {
                        timer.select(direction);
                    }
                    redraw = true;
                }
            } else {
                tilt.clear();
            }
        }
        while let Some(event) = touch.next_event() {
            if matches!(event, TouchEvent::Released(_) | TouchEvent::Pressed(_)) {
                hold.cancel();
            }
            if let TouchEvent::Moved(p) = event {
                let direction = if p.y >= 180 && p.x < 80 {
                    Some(-1)
                } else if p.y >= 180 && p.x >= 240 {
                    Some(1)
                } else {
                    None
                };
                if direction != hold.direction() {
                    hold.cancel();
                }
            }
            if let TouchEvent::Pressed(p) = event
                && p.y < 40
                && p.x < 110
                && !timer.running()
                && !intervals.running()
            {
                interval_mode = !interval_mode;
                timer.reset();
                intervals.reset();
                tilt.clear();
                redraw = true;
            } else if let TouchEvent::Pressed(p) = event
                && (40..70).contains(&p.y)
                && interval_mode
                && !intervals.running()
            {
                edit_phase = (p.x.max(0) as usize / 107).min(2);
                redraw = true;
            } else if let TouchEvent::Pressed(p) = event
                && (145..180).contains(&p.y)
            {
                diagnostics = !diagnostics;
                redraw = true;
            } else if let TouchEvent::Pressed(p) = event
                && p.y >= 180
            {
                if p.x < 80 || p.x >= 240 {
                    let direction = if p.x < 80 { -1 } else { 1 };
                    hold.press(direction, now);
                    if interval_mode && !intervals.running() {
                        adjust_interval(&mut intervals, edit_phase, direction);
                    } else if !interval_mode {
                        timer.adjust(direction);
                    }
                } else if p.x < 160 {
                    if interval_mode {
                        intervals.toggle(now);
                    } else {
                        timer.toggle(now);
                    }
                    tilt.clear();
                } else {
                    if interval_mode {
                        intervals.reset();
                    } else {
                        timer.reset();
                    }
                    speaker.clear();
                    chime = 0;
                }
                redraw = true;
            }
        }
        if let Some(direction) = hold.update(now) {
            if interval_mode && !intervals.running() {
                adjust_interval(&mut intervals, edit_phase, direction);
            } else if !interval_mode {
                timer.adjust(direction);
            }
            redraw = true;
        }
        let remaining = if interval_mode {
            intervals.remaining(now)
        } else {
            timer.remaining(now)
        };
        if chime > 0 {
            feed_chime(&mut speaker, &mut wave, &mut chime);
        }
        let second = remaining.div_ceil(1000);
        if second != last_second {
            last_second = second;
            redraw = true;
        }
        let battery = cores3_device::capabilities::battery::reading();
        if battery != last_battery {
            last_battery = battery;
            redraw = true;
        }
        if redraw {
            draw(
                &mut canvas,
                timer.duration(),
                remaining,
                if interval_mode {
                    intervals.running()
                } else {
                    timer.running()
                },
                if interval_mode {
                    intervals.finished()
                } else {
                    timer.finished
                },
                diagnostics,
                if interval_mode {
                    Some((
                        intervals.phase(),
                        edit_phase,
                        intervals.duration(0),
                        intervals.duration(1),
                        intervals.round(),
                        intervals.rounds(),
                    ))
                } else {
                    None
                },
            );
            canvas.show(&mut display.surface(SCREEN));
            redraw = false;
        }
        Timer::after(Duration::from_millis(10)).await;
    }
}

/// Change the selected phase duration or the number of rounds.
fn adjust_interval(timer: &mut Intervals, editing: usize, direction: i8) {
    if editing == 2 {
        let count = (i16::from(timer.rounds()) + i16::from(direction)).clamp(1, 99) as u8;
        timer.set_rounds(count);
    } else {
        timer.configure(editing, step_duration(timer.duration(editing), direction));
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

fn draw(
    canvas: &mut Canvas,
    duration: u64,
    remaining: u64,
    running: bool,
    finished: bool,
    diagnostics: bool,
    interval: Option<(usize, usize, u64, u64, u8, u8)>,
) {
    let bg = Rgb565::new(2, 5, 8);
    let accent = Rgb565::new(8, 50, 25);
    canvas.clear(bg);
    let area = |x, y, w, h| Rectangle::new(Point::new(x, y), Size::new(w, h));
    use core::fmt::Write;
    let button = Rgb565::new(5, 13, 16);
    let _ = area(8, 5, 100, 30)
        .into_styled(PrimitiveStyle::with_fill(button))
        .draw(canvas);
    common::centered_text(
        canvas,
        area(8, 5, 100, 30),
        if interval.is_some() {
            "INTERVALS"
        } else {
            "SINGLE"
        },
        common::DENSE_FONT,
        accent,
    );
    if interval.is_none() {
        common::centered_text(
            canvas,
            area(8, 40, 304, 28),
            "10s STEPS - HOLD TO SPEED UP",
            common::BODY_FONT,
            Rgb565::WHITE,
        );
    }
    if let Some((_, editing, work, rest, _, rounds)) = interval {
        for (index, (name, value)) in [
            ("WORK", work / 1000),
            ("BREAK", rest / 1000),
            ("ROUNDS", u64::from(rounds)),
        ]
        .iter()
        .enumerate()
        {
            let rect = area(index as i32 * 107, 40, 105, 28);
            let _ = rect
                .into_styled(PrimitiveStyle::with_fill(if editing == index {
                    accent
                } else {
                    button
                }))
                .draw(canvas);
            let mut label = arrayvec::ArrayString::<20>::new();
            let _ = write!(
                label,
                "{} {}{}",
                name,
                value,
                if index < 2 { "s" } else { "" }
            );
            common::centered_text(canvas, rect, &label, common::DENSE_FONT, Rgb565::WHITE);
        }
    }
    let seconds = remaining.div_ceil(1000);
    draw_time(canvas, seconds);
    let _ = area(30, 139, 260, 4)
        .into_styled(PrimitiveStyle::with_fill(Rgb565::new(5, 12, 14)))
        .draw(canvas);
    let total = interval.map_or(
        duration,
        |(phase, _, first, second, _, _)| if phase == 0 { first } else { second },
    );
    let width = (260 * remaining / total) as u32;
    let _ = area(30, 139, width, 4)
        .into_styled(PrimitiveStyle::with_fill(accent))
        .draw(canvas);
    let mut hint = arrayvec::ArrayString::<48>::new();
    if finished {
        let _ = write!(hint, "COMPLETE");
    } else if let Some((phase, _, _, _, round, rounds)) = interval {
        if running {
            let _ = write!(
                hint,
                "ROUND {}/{} - {}",
                round,
                rounds,
                if phase == 0 { "WORK" } else { "BREAK" }
            );
        } else {
            let _ = write!(hint, "TAP A SETTING, ADJUST WITH < >");
        }
    } else if running {
        let _ = write!(hint, "RUNNING");
    } else if diagnostics {
        let _ = write!(hint, "IMU LOG ON - TAP TO STOP");
    } else {
        let _ = write!(hint, "TAP +/- 10s - HOLD FOR FASTER");
    }
    common::centered_text(
        canvas,
        area(0, 150, 320, 22),
        &hint,
        common::DENSE_FONT,
        Rgb565::WHITE,
    );
    let mut battery_label = arrayvec::ArrayString::<12>::new();
    let battery = cores3_device::capabilities::battery::reading();
    let color = if battery.is_some_and(|b| b.charging) {
        accent
    } else if battery.is_some_and(|b| b.percent <= 20) {
        Rgb565::RED
    } else {
        Rgb565::WHITE
    };
    if let Some(battery) = battery {
        let _ = write!(
            battery_label,
            "{}%",
            (u16::from(battery.percent) + 5) / 10 * 10
        );
    } else {
        let _ = write!(battery_label, "--");
    }
    common::centered_text(
        canvas,
        area(236, 9, 44, 20),
        &battery_label,
        common::DENSE_FONT,
        color,
    );
    let _ = area(283, 12, 25, 14)
        .into_styled(PrimitiveStyle::with_stroke(color, 1))
        .draw(canvas);
    let _ = area(308, 16, 3, 6)
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(canvas);
    if let Some(battery) = battery {
        let _ = area(285, 14, u32::from(battery.percent) * 21 / 100, 10)
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(canvas);
        if battery.charging {
            for (a, b) in [
                ((228, 10), (222, 19)),
                ((222, 19), (229, 19)),
                ((229, 19), (223, 28)),
            ] {
                let _ = Line::new(Point::new(a.0, a.1), Point::new(b.0, b.1))
                    .into_styled(PrimitiveStyle::with_stroke(accent, 2))
                    .draw(canvas);
            }
        }
    }
    for (rect, fill) in [
        (area(82, 184, 76, 50), accent),
        (area(164, 184, 72, 50), button),
    ] {
        let _ = rect
            .into_styled(PrimitiveStyle::with_fill(fill))
            .draw(canvas);
    }
    common::centered_text(
        canvas,
        area(0, 180, 80, 60),
        "-",
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
        "+",
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
                let _ = Rectangle::new(Point::new(x + dx, 75 + dy), Size::new(w, h))
                    .into_styled(style)
                    .draw(canvas);
            }
        }
    }
    for y in [92, 113] {
        let _ = Rectangle::new(Point::new(157, y), Size::new(4, 4))
            .into_styled(style)
            .draw(canvas);
    }
}
