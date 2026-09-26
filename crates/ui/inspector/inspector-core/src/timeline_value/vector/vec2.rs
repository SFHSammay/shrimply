use glam::Vec2;
use shrimply_project_document::project::Time;
use shrimply_property_model::timeline_value::{
    CurveEditPolicy, CurveKeyframeInsert, Interpolation, TimelineBase, TimelineValue,
    TimelineValueType, edit_curve_value, insert_curve_keyframe,
};
use uuid::Uuid;

use crate::keyframe_model::{self, KeyframeClipboard};

const VALUE_EPSILON_SQUARED: f32 = 0.000_001;

pub type Vec2Timeline = TimelineValue<Vec2>;

pub fn value_at(value: &Vec2Timeline, time: Time) -> Vec2 {
    value.value_at(time)
}

pub fn set_component(
    value: &mut Vec2Timeline,
    evaluation_time: Time,
    keyframe_time: Time,
    component: usize,
    next: f64,
) -> bool {
    if !next.is_finite() {
        return false;
    }
    let mut value_at_time = value_at(value, evaluation_time);
    match component {
        0 => value_at_time.x = next as f32,
        1 => value_at_time.y = next as f32,
        _ => return false,
    }
    set_value(value, keyframe_time, value_at_time)
}

pub fn set_value(value: &mut Vec2Timeline, time: Time, next: Vec2) -> bool {
    edit_curve_value(
        value,
        time,
        next,
        |current, next| (*current - *next).length_squared() <= VALUE_EPSILON_SQUARED,
        CurveEditPolicy {
            unchanged_keyframe_is_noop: false,
            insert: CurveKeyframeInsert::InheritPreviousInterpolation,
        },
    )
}

pub fn set_keyframes_enabled(
    value: &mut Vec2Timeline,
    evaluation_time: Time,
    keyframe_time: Time,
    enabled: bool,
) -> bool {
    let current = value_at(value, evaluation_time);
    keyframe_model::set_keyframes_enabled(value, keyframe_time, current, enabled)
}

pub fn set_expression_enabled(value: &mut Vec2Timeline, enabled: bool) -> bool {
    keyframe_model::set_expression_enabled(
        value,
        enabled,
        crate::timeline_value::VECTOR2_EXPRESSION_DEFAULT,
    )
}

pub fn set_expression_source(value: &mut Vec2Timeline, source: String) -> bool {
    let Some(expression) = value.expression.as_mut() else {
        return false;
    };
    if expression.source == source {
        return false;
    }
    expression.source = source;
    true
}

pub fn add_keyframe(value: &mut Vec2Timeline, time: Time) -> bool {
    let current = value_at(value, time);
    let TimelineBase::Keyframes(keyframes) = &mut value.base else {
        return false;
    };
    if let Some(keyframe) = keyframes
        .iter_mut()
        .find(|keyframe| keyframe.time.approx_eq(time))
    {
        if keyframe.time == time {
            return false;
        }
        keyframe.time = time;
        keyframes.sort_by_key(|keyframe| keyframe.time);
    } else {
        insert_curve_keyframe(
            keyframes,
            Vec2::keyframe(time, current),
            CurveKeyframeInsert::InheritPreviousInterpolation,
        );
    }
    true
}

pub fn delete_keyframe(value: &mut Vec2Timeline, time: Time) -> bool {
    keyframe_model::delete_discrete_keyframe(value, time, Time::ZERO)
}

pub fn move_keyframes(value: &mut Vec2Timeline, moves: &[(Time, Time)]) -> bool {
    keyframe_model::move_discrete_keyframes(value, moves)
}

pub fn copy_keyframes(value: &Vec2Timeline, selected: &[Time]) -> Option<KeyframeClipboard> {
    keyframe_model::copy_keyframes(value, selected)
}

pub fn paste_keyframes(
    value: &mut Vec2Timeline,
    clipboard: &KeyframeClipboard,
    times: &[Time],
) -> Option<Vec<Time>> {
    keyframe_model::paste_keyframes(value, clipboard, times)
}

pub fn set_interpolation(
    value: &mut Vec2Timeline,
    owner_id: Uuid,
    interpolation: Interpolation,
) -> bool {
    keyframe_model::set_interpolation(value, owner_id, interpolation)
}

pub fn format_value(
    value: Vec2,
    first_prefix: &str,
    second_prefix: &str,
    digits: usize,
    unit: &str,
) -> String {
    format!(
        "{} {:.*}{}  {} {:.*}{}",
        first_prefix, digits, value.x, unit, second_prefix, digits, value.y, unit,
    )
}
