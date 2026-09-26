use shrimply_math_core::Time;

use crate::{
    CURSOR_LANE_HEIGHT, GRAPH_PAD, GraphDomain, KeyframeGraph, KeyframePoint, STEP_GRAPH_RANGE,
    raw_point, raw_range, segment_speed_at, speed_range,
};

pub(crate) fn graph_range(graph: &KeyframeGraph) -> (f64, f64) {
    match graph {
        KeyframeGraph::Step { .. } => STEP_GRAPH_RANGE,
        KeyframeGraph::RawValue {
            points, segments, ..
        } => raw_range(points, segments),
        KeyframeGraph::Speed {
            segments,
            static_value,
            ..
        } if segments.is_empty() => (0.0, static_value.max(1.0)),
        KeyframeGraph::Speed { segments, .. } => speed_range(segments),
    }
}

// A speed key can have distinct incoming and outgoing markers at the same time.
pub(crate) fn graph_points(graph: &KeyframeGraph) -> Vec<KeyframePoint> {
    match graph {
        KeyframeGraph::Step { points } | KeyframeGraph::RawValue { points, .. } => points.clone(),
        KeyframeGraph::Speed {
            segments,
            keys,
            static_value,
        } if segments.is_empty() => keys
            .iter()
            .map(|time| KeyframePoint {
                time: *time,
                value: *static_value,
            })
            .collect(),
        KeyframeGraph::Speed { segments, .. } => {
            let mut points = Vec::new();
            for segment in segments {
                points.extend([
                    KeyframePoint {
                        time: segment.start,
                        value: segment_speed_at(segment, 0.0).unwrap_or(0.0),
                    },
                    KeyframePoint {
                        time: segment.end,
                        value: segment_speed_at(segment, 1.0).unwrap_or(0.0),
                    },
                ]);
            }
            points
        }
    }
}

pub(crate) fn keyframe_positions(
    graph: &KeyframeGraph,
    domain: GraphDomain,
    width: f64,
    height: f64,
    frame_step: Time,
) -> impl Iterator<Item = (Time, glam::DVec2)> {
    let range = graph_range(graph);
    graph_points(graph).into_iter().filter_map(move |point| {
        let (x, y) = if matches!(graph, KeyframeGraph::Step { .. }) {
            let x =
                shrimply_discrete_keyframe_graph_skia::key_x(point.time, width, domain, frame_step);
            if x < GRAPH_PAD || x > width - GRAPH_PAD {
                return None;
            }
            (
                x,
                shrimply_discrete_keyframe_graph_skia::key_y(height, CURSOR_LANE_HEIGHT),
            )
        } else {
            raw_point(point, width, height, domain, range)
        };
        Some((point.time, glam::DVec2::new(x, y)))
    })
}
