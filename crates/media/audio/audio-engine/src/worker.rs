use hashbrown::HashMap;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use shrimply_math_core::Fraction;

use shrimply_project_document::project::{
    Project, default_playback_speed, fraction_denominator, fraction_numerator,
};

use super::output::{self, OutputState, PlaybackWindow};
use super::streaming::{self, AudioRenderSession, AudioSourceKey};

const CHUNK_MS: u64 = 80;
const LOOKAHEAD_MS: u64 = 1_500;
const LOOKBEHIND_MS: u64 = 250;
const MAX_WINDOW_MS: u64 = 2_500;
const IDLE_WAIT_MS: u64 = 20;

pub(super) enum AudioCommand {
    SetProject(Box<Project>),
    SetPlaybackSpeed(Fraction),
    Seek { frame: u64 },
    PlayFrom { frame: u64 },
    Preview { frame: u64, frames: usize },
    Pause,
    Stop,
}

#[derive(Clone, Copy)]
enum FillMode {
    Idle,
    Play {
        next_output_frame: u64,
        next_timeline_frame: u64,
    },
    Preview {
        frame: u64,
        frames: usize,
    },
}

struct WorkerState {
    project: Project,
    sessions: HashMap<AudioSourceKey, AudioRenderSession>,
    generation: u64,
    mode: FillMode,
    sample_rate: u32,
    playback_speed: Fraction,
}

pub(super) fn spawn(
    project: Project,
    sample_rate: u32,
    shared: OutputState,
) -> (Sender<AudioCommand>, JoinHandle<()>) {
    let (command_tx, command_rx) = mpsc::channel();
    let worker = thread::spawn(move || worker_loop(project, sample_rate, shared, command_rx));
    (command_tx, worker)
}

fn worker_loop(
    project: Project,
    sample_rate: u32,
    shared: OutputState,
    command_rx: Receiver<AudioCommand>,
) {
    let mut state = WorkerState {
        project,
        sessions: HashMap::new(),
        generation: 0,
        mode: FillMode::Idle,
        sample_rate,
        playback_speed: default_playback_speed(),
    };

    loop {
        if matches!(state.mode, FillMode::Idle) {
            match command_rx.recv() {
                Ok(command) => {
                    if handle_command(command, &mut state, &shared) {
                        return;
                    }
                }
                Err(_) => return,
            }
        }

        while let Ok(command) = command_rx.try_recv() {
            if handle_command(command, &mut state, &shared) {
                return;
            }
        }

        match state.mode {
            FillMode::Idle => {}
            FillMode::Preview { frame, frames } => {
                match fill_preview(&mut state, &shared, frame, frames) {
                    Ok(()) => state.mode = FillMode::Idle,
                    Err(error) => fail_playback(&mut state, &shared, error),
                }
            }
            FillMode::Play {
                next_output_frame,
                next_timeline_frame,
            } => {
                match fill_play(
                    &mut state,
                    &shared,
                    next_output_frame,
                    next_timeline_frame,
                    &command_rx,
                ) {
                    Ok(ControlFlow::Continue(mode)) => state.mode = mode,
                    Ok(ControlFlow::Break(())) => return,
                    Err(error) => fail_playback(&mut state, &shared, error),
                }
            }
        }
    }
}

fn handle_command(command: AudioCommand, state: &mut WorkerState, shared: &OutputState) -> bool {
    match command {
        AudioCommand::SetProject(project) => {
            state.project = *project;
            streaming::retain_project_sessions(&state.project, &mut state.sessions);
            shared.duration_frames.store(
                state.project.duration().as_sample_frame(state.sample_rate),
                Ordering::SeqCst,
            );
            clear_window(state, shared);
            state.mode = if shared.playing.load(Ordering::SeqCst) {
                let frame = shared.cursor_frame.load(Ordering::SeqCst);
                FillMode::Play {
                    next_output_frame: frame,
                    next_timeline_frame: frame,
                }
            } else {
                FillMode::Idle
            };
            false
        }
        AudioCommand::SetPlaybackSpeed(playback_speed) => {
            state.playback_speed = playback_speed;
            if shared.playing.load(Ordering::SeqCst) {
                clear_window(state, shared);
                let frame = shared.cursor_frame.load(Ordering::SeqCst);
                state.mode = FillMode::Play {
                    next_output_frame: frame,
                    next_timeline_frame: frame,
                };
            }
            false
        }
        AudioCommand::Seek { frame } => {
            clear_window(state, shared);
            state.mode = if shared.playing.load(Ordering::SeqCst) {
                FillMode::Play {
                    next_output_frame: frame,
                    next_timeline_frame: frame,
                }
            } else {
                FillMode::Idle
            };
            false
        }
        AudioCommand::PlayFrom { frame } => {
            clear_window(state, shared);
            state.mode = FillMode::Play {
                next_output_frame: frame,
                next_timeline_frame: frame,
            };
            false
        }
        AudioCommand::Preview { frame, frames } => {
            clear_window(state, shared);
            state.mode = FillMode::Preview { frame, frames };
            false
        }
        AudioCommand::Pause => {
            state.mode = FillMode::Idle;
            false
        }
        AudioCommand::Stop => true,
    }
}

fn fill_preview(
    state: &mut WorkerState,
    shared: &OutputState,
    frame: u64,
    frames: usize,
) -> Result<(), String> {
    let duration_frames = shared.duration_frames.load(Ordering::SeqCst);
    if frame >= duration_frames || frames == 0 {
        return Ok(());
    }
    let frames = frames.min(duration_frames.saturating_sub(frame) as usize);
    let samples = streaming::mix_project_range_result(
        &state.project,
        &mut state.sessions,
        frame,
        frames,
        state.sample_rate,
    )?;
    store_chunk(state, shared, frame, samples);

    shared.cursor_frame.store(frame, Ordering::SeqCst);
    shared
        .preview_end_frame
        .store(frame.saturating_add(frames as u64), Ordering::SeqCst);
    shared.previewing.store(true, Ordering::SeqCst);
    Ok(())
}

fn fill_play(
    state: &mut WorkerState,
    shared: &OutputState,
    next_output_frame: u64,
    next_timeline_frame: u64,
    command_rx: &Receiver<AudioCommand>,
) -> Result<ControlFlow<(), FillMode>, String> {
    if !shared.playing.load(Ordering::SeqCst) {
        return Ok(ControlFlow::Continue(FillMode::Idle));
    }

    let cursor = shared.cursor_frame.load(Ordering::SeqCst);
    let lookahead_frames = frames_for_ms(state.sample_rate, LOOKAHEAD_MS) as u64;
    let fill_until = cursor.saturating_add(lookahead_frames);
    let (next_output_frame, next_timeline_frame) = if cursor > next_output_frame {
        (
            cursor,
            next_timeline_frame.saturating_add(scaled_frame_count(
                cursor.saturating_sub(next_output_frame) as usize,
                state.playback_speed,
            ) as u64),
        )
    } else {
        (next_output_frame, next_timeline_frame)
    };
    if next_output_frame >= fill_until {
        match command_rx.recv_timeout(Duration::from_millis(IDLE_WAIT_MS)) {
            Ok(command) => {
                if handle_command(command, state, shared) {
                    return Ok(ControlFlow::Break(()));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(ControlFlow::Break(())),
        }
        return Ok(ControlFlow::Continue(state.mode));
    }

    let duration_frames = shared.duration_frames.load(Ordering::SeqCst);
    if next_timeline_frame >= duration_frames {
        return Ok(ControlFlow::Continue(FillMode::Play {
            next_output_frame,
            next_timeline_frame,
        }));
    }

    let frames = frames_for_ms(state.sample_rate, CHUNK_MS)
        .min(fill_until.saturating_sub(next_output_frame) as usize);
    let timeline_frames = scaled_frame_count(frames, state.playback_speed)
        .min(duration_frames.saturating_sub(next_timeline_frame) as usize);
    let samples = mix_playback_range(state, next_timeline_frame, frames, timeline_frames)?;
    store_chunk(state, shared, next_output_frame, samples);

    Ok(ControlFlow::Continue(FillMode::Play {
        next_output_frame: next_output_frame.saturating_add(frames as u64),
        next_timeline_frame: next_timeline_frame.saturating_add(timeline_frames as u64),
    }))
}

fn mix_playback_range(
    state: &mut WorkerState,
    timeline_frame: u64,
    output_frames: usize,
    timeline_frames: usize,
) -> Result<Vec<f32>, String> {
    if output_frames == 0 {
        return Ok(Vec::new());
    }
    if timeline_frames == output_frames {
        return streaming::mix_project_range_result(
            &state.project,
            &mut state.sessions,
            timeline_frame,
            output_frames,
            state.sample_rate,
        );
    }

    let source = streaming::mix_project_range_result(
        &state.project,
        &mut state.sessions,
        timeline_frame,
        timeline_frames.max(1),
        state.sample_rate,
    )?;
    let fallback = trim_or_pad_samples(source.clone(), output_frames);
    Ok(streaming::pitch_preserving_speed(
        &source,
        state.playback_speed,
        state.sample_rate,
        output_frames,
    )
    .unwrap_or(fallback))
}

fn clear_window(state: &mut WorkerState, shared: &OutputState) {
    state.generation = state.generation.wrapping_add(1);
    let mut next = PlaybackWindow::new();
    next.clear(state.generation);
    match shared.window.write() {
        Ok(mut window) => *window = Arc::new(next),
        Err(error) => *error.into_inner() = Arc::new(next),
    }
}

fn store_chunk(state: &WorkerState, shared: &OutputState, start_frame: u64, samples: Vec<f32>) {
    let cursor = shared.cursor_frame.load(Ordering::SeqCst);
    let keep_before = cursor.saturating_sub(frames_for_ms(state.sample_rate, LOOKBEHIND_MS) as u64);
    let max_frames = frames_for_ms(state.sample_rate, MAX_WINDOW_MS);
    let current = match shared.window.read() {
        Ok(window) => window.clone(),
        Err(error) => error.into_inner().clone(),
    };
    let mut next = (*current).clone();
    next.store_chunk(
        state.generation,
        start_frame,
        samples,
        keep_before,
        max_frames,
    );
    match shared.window.write() {
        Ok(mut window) => *window = Arc::new(next),
        Err(error) => *error.into_inner() = Arc::new(next),
    }
}

fn fail_playback(state: &mut WorkerState, shared: &OutputState, error: String) {
    let message = format!("Audio rendering failed: {error}");
    output::fail_output(message, shared);
    state.mode = FillMode::Idle;
}

fn frames_for_ms(sample_rate: u32, milliseconds: u64) -> usize {
    (sample_rate as u64 * milliseconds / 1_000) as usize
}

fn scaled_frame_count(frames: usize, playback_speed: Fraction) -> usize {
    let numerator = fraction_numerator(playback_speed).max(1) as u128;
    let denominator = fraction_denominator(playback_speed).max(1) as u128;
    let frames = frames as u128;
    frames
        .saturating_mul(numerator)
        .saturating_add(denominator.saturating_sub(1))
        .saturating_div(denominator)
        .min(usize::MAX as u128) as usize
}

fn trim_or_pad_samples(mut samples: Vec<f32>, output_frames: usize) -> Vec<f32> {
    let output_len = output_frames * super::CHANNELS;
    samples.truncate(output_len);
    samples.resize(output_len, 0.0);
    samples
}
