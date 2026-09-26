use shrimply_components_skia::canvas::{FontId, Rect, Stroke, StrokeKind, TimelinePainter, vec2};
use shrimply_math_color::Color;
use shrimply_math_core::{Time, fraction_floor_i64, fraction_from_integer};

pub const CONTENT_HEIGHT: i32 = 52;
pub const MAX_FRAME_WIDTH: f64 = 48.0;
const FRAME_CELL_MIN_WIDTH: f64 = 6.0;
const FRAME_LABEL_MIN_WIDTH: f64 = 64.0;
const GRAPH_PAD: f64 = 12.0;
const FRAME_LABEL_ALPHA: f32 = 0.7;
const FRAME_SHADE_ALPHA: f32 = 0.22;

pub struct Draw<'a> {
    pub painter: &'a TimelinePainter,
    pub width: f64,
    pub content_height: f64,
    pub ruler_height: f64,
    pub domain: (Time, Time),
    pub frame_step: Time,
    pub border_color: Color,
    pub foreground_color: Color,
    pub shade_color: Color,
}

pub fn draw(draw: Draw<'_>) {
    if draw.frame_step <= Time::ZERO {
        return;
    }
    let frame_width = draw.frame_step.as_secs_f64() / seconds_per_pixel(draw.width, draw.domain);
    let minimum_stride = (FRAME_CELL_MIN_WIDTH / frame_width).ceil().max(1.0) as u64;
    let stride = minimum_stride.next_power_of_two() as i64;

    let first_visible_frame = fraction_floor_i64(draw.domain.0.seconds / draw.frame_step.seconds)
        .expect("visible keyframe domain exceeds the exact frame range");
    let first_frame = first_visible_frame.div_euclid(stride) * stride;
    let last_frame = fraction_floor_i64(draw.domain.1.seconds / draw.frame_step.seconds)
        .expect("visible keyframe domain exceeds the exact frame range")
        + 1;
    let minimum_label_stride = (FRAME_LABEL_MIN_WIDTH / frame_width).ceil().max(5.0) as i64;
    let label_stride = (minimum_label_stride + 4).div_euclid(5) * 5;
    let top = draw.ruler_height;
    let height = (draw.content_height - top).max(0.0);
    let border = Stroke::new(1.0, draw.border_color);
    let mut frame = first_frame;
    while frame <= last_frame {
        let start = Time {
            seconds: draw.frame_step.seconds * fraction_from_integer(frame),
        };
        let end = Time {
            seconds: draw.frame_step.seconds * fraction_from_integer(frame + stride),
        };
        let left = time_x(start, draw.width, draw.domain).max(GRAPH_PAD);
        let right = time_x(end, draw.width, draw.domain).min(draw.width - GRAPH_PAD);
        if right <= left {
            frame += stride;
            continue;
        }
        let bounds = Rect::from_min_size(
            vec2(left as f32, top as f32),
            vec2((right - left) as f32, height as f32),
        );
        if frame.rem_euclid(label_stride) == 0 {
            draw.painter.rect_filled(
                bounds,
                0,
                draw.shade_color.alpha_multiply(FRAME_SHADE_ALPHA),
            );
        }
        draw.painter
            .rect_stroke(bounds, 0, border, StrokeKind::Inside);
        if frame == 0 || frame.rem_euclid(label_stride) == 0 {
            let label = if frame == 0 {
                "1".to_string()
            } else {
                frame.to_string()
            };
            draw.painter.system_text(
                vec2((left + 2.0) as f32, (draw.ruler_height - 5.0) as f32),
                label,
                FontId::proportional(9.0),
                draw.foreground_color.alpha_multiply(FRAME_LABEL_ALPHA),
            );
        }
        frame += stride;
    }
}

pub fn key_y(content_height: f64, ruler_height: f64) -> f64 {
    ruler_height + (content_height - ruler_height) / 2.0
}

pub fn key_x(time: Time, width: f64, domain: (Time, Time), frame_step: Time) -> f64 {
    time_x(time, width, domain) + frame_width(width, domain, frame_step) / 2.0
}

pub fn frame_width(width: f64, domain: (Time, Time), frame_step: Time) -> f64 {
    frame_step.as_secs_f64() / seconds_per_pixel(width, domain)
}

fn time_x(time: Time, width: f64, domain: (Time, Time)) -> f64 {
    let duration = domain
        .1
        .saturating_sub(domain.0)
        .as_secs_f64()
        .max(f64::EPSILON);
    GRAPH_PAD + (time.as_secs_f64() - domain.0.as_secs_f64()) / duration * (width - GRAPH_PAD * 2.0)
}

fn seconds_per_pixel(width: f64, domain: (Time, Time)) -> f64 {
    domain
        .1
        .saturating_sub(domain.0)
        .as_secs_f64()
        .max(f64::EPSILON)
        / (width - GRAPH_PAD * 2.0).max(1.0)
}
