use shrimply_components_skia::canvas::{UVec2, Vec2, vec2};
use shrimply_editor_state::player_state::{self, SharedPlayerState};
use shrimply_editor_state::preferences as preferences_store;
use shrimply_playback_performance as playback_performance;
use shrimply_project_document::project::{Project, Time};
use shrimply_resource_pipeline::{Event as PipelineEvent, TryNext};
use shrimply_surface_gl_skia::TimelineRenderer;
use shrimply_timeline_edit::selection_state::SharedSelectionState;
use shrimply_timeline_qt::{
    ContextMenu, ContextMenuAction, ContextMenuControl, ContextMenuRequest, CursorTool,
    DragCollisionMode, TrackAddAction, VideoFrameSelection,
};
use shrimply_timeline_skia::scene::{
    Event as TimelineEvent, PointerButton, Scene, TimelineModifiers, TrackAddMenuRequest,
};
use shrimply_timeline_skia::{TimelineTools, ToolState, TrackAddOutcome};
use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolkitPointerButton {
    Primary,
    Middle,
}

pub struct RenderedVideoFrame {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub struct TrackAddMenuPresentation {
    pub kind: shrimply_timeline_edit::TrackKind,
    pub x: f32,
    pub y: f32,
}

pub struct ToolkitTimeline {
    project: Rc<RefCell<Project>>,
    player_state: SharedPlayerState,
    selection_state: SharedSelectionState,
    tools: TimelineTools,
    context_menu: ContextMenu,
    context_file_path: Option<PathBuf>,
    track_add_request: Option<TrackAddMenuRequest>,
    track_add_presentation: Option<TrackAddMenuPresentation>,
    pending_track_import: Option<shrimply_timeline_skia::import::TrackImportInspection>,
    track_import_error: RefCell<Option<String>>,
    scene: RefCell<Scene>,
    renderer: TimelineRenderer,
}

impl ToolkitTimeline {
    pub fn new(
        project: Rc<RefCell<Project>>,
        player_state: SharedPlayerState,
        playback_performance: playback_performance::SharedCollector,
        selection_state: SharedSelectionState,
        preferences: preferences_store::SharedPreferences,
        property_clipboard: shrimply_property_transfer::SharedClipboard,
    ) -> Self {
        let tools = TimelineTools::new(preferences.clone());
        let scene = Scene::new(
            project.clone(),
            player_state.clone(),
            selection_state.clone(),
            preferences,
            property_clipboard,
            playback_performance,
        );
        Self {
            project,
            player_state,
            selection_state,
            tools,
            context_menu: ContextMenu::default(),
            context_file_path: None,
            track_add_request: None,
            track_add_presentation: None,
            pending_track_import: None,
            track_import_error: RefCell::new(None),
            scene: RefCell::new(scene),
            renderer: TimelineRenderer::new(),
        }
    }

    pub fn render(
        &mut self,
        width: u32,
        height: u32,
        pixels_per_point: f32,
        accent_color: shrimply_math_color::Color,
    ) -> Result<(), String> {
        self.poll_track_import();
        let painter = self.renderer.begin_frame(
            UVec2::new(width.max(1), height.max(1)),
            pixels_per_point,
            shrimply_cross_ui_theme::current().view_bg,
        )?;
        let mut scene = self.scene.borrow_mut();
        scene.draw_frame(
            painter.canvas(),
            vec2(
                width as f32 / pixels_per_point,
                height as f32 / pixels_per_point,
            ),
            shrimply_timeline_skia::scene::Frame {
                before_seek: None,
                accent_color,
                active_audio_recording_key: None,
                active_video_recording_key: None,
                live_recording: None,
                live_video_recording: None,
            },
        );
        self.renderer.end_frame()?;
        let requests = scene.take_requests();
        if requests.pause_playback {
            player_state::set_playing(&self.player_state, false);
        }
        if let Some(key) = requests.audio_record
            && let Err(error) = scene.toggle_audio_recording(key)
        {
            self.set_error(error);
        }
        if let Some(request) = requests.track_add {
            let row = shrimply_timeline_skia::items::row_for_track(
                &self.project.borrow(),
                request.key.kind,
                request.key.track_index,
            )
            .expect("add menu track must exist");
            self.track_add_presentation = Some(TrackAddMenuPresentation {
                kind: request.key.kind,
                x: shrimply_timeline_skia::metrics::TRACK_LABEL_ADD_X as f32,
                y: shrimply_timeline_skia::track_controls::track_label_button_y(
                    shrimply_timeline_skia::drawing::row_screen_y(row, scene.view()),
                ) as f32,
            });
            self.track_add_request = Some(request);
        }
        Ok(())
    }

    pub fn take_track_add_menu(&mut self) -> Option<TrackAddMenuPresentation> {
        self.track_add_presentation.take()
    }

    pub fn activate_track_add_action(&mut self, action: TrackAddAction) -> bool {
        let Some(request) = self.track_add_request.as_ref() else {
            return false;
        };
        let scene = self.scene.borrow();
        let default_text_font_family = scene.default_text_font_family.clone();
        let settings = shrimply_timeline_skia::TrackAddSettings {
            default_visual_duration: scene.default_visual_duration,
            default_text_font_family: &default_text_font_family,
        };
        drop(scene);
        shrimply_timeline_skia::activate_track_add(
            &self.project,
            &self.player_state,
            &self.selection_state,
            request.key,
            action,
            settings,
        ) != TrackAddOutcome::Unchanged
    }

    pub fn import_track_file(&mut self, path: PathBuf) -> Result<(), String> {
        let request = self
            .track_add_request
            .as_ref()
            .ok_or_else(|| "track add menu is no longer active".to_string())?;
        if request.import_targets.is_empty() {
            return Err("no import tracks were selected".to_string());
        }
        let kind = request.import_targets[0].kind;
        if !request
            .import_targets
            .iter()
            .all(|target| target.kind == kind)
        {
            return Err("selected tracks must have the same type".to_string());
        }
        let track_indices = request
            .import_targets
            .iter()
            .map(|target| target.track_index)
            .collect::<Vec<_>>();
        let start = player_state::snapshot(&self.player_state).position;
        let started = shrimply_timeline_skia::import::start_track_import(
            &mut self.project.borrow_mut(),
            path,
            kind,
            track_indices,
            start,
            self.scene.borrow().default_visual_duration,
        )?;
        match started {
            shrimply_timeline_skia::import::TrackImportStart::Inspect(inspection) => {
                self.pending_track_import = Some(inspection);
                Ok(())
            }
            shrimply_timeline_skia::import::TrackImportStart::Complete(result) => {
                shrimply_timeline_skia::import::finish_track_import(
                    &self.player_state,
                    &self.selection_state,
                    Ok(result),
                )
            }
        }
    }

    pub fn take_track_import_error(&mut self) -> Option<String> {
        self.track_import_error.borrow_mut().take()
    }

    fn poll_track_import(&mut self) {
        let event = match self.pending_track_import.as_mut() {
            Some(pending) => pending.subscription.try_next(),
            None => return,
        };
        let result = match event {
            TryNext::Event(PipelineEvent::Finished(info)) => {
                let pending = self
                    .pending_track_import
                    .take()
                    .expect("finished track import must exist");
                shrimply_timeline_skia::import::finish_track_import_inspection(
                    &mut self.project.borrow_mut(),
                    pending.context,
                    &info,
                )
            }
            TryNext::Event(PipelineEvent::Failed(error)) => {
                self.pending_track_import = None;
                Err(error.to_string())
            }
            TryNext::Event(PipelineEvent::Cancelled) | TryNext::Closed => {
                self.pending_track_import = None;
                return;
            }
            TryNext::Event(PipelineEvent::Progress(_)) | TryNext::Empty => return,
        };
        if let Err(error) = shrimply_timeline_skia::import::finish_track_import(
            &self.player_state,
            &self.selection_state,
            result,
        ) {
            self.set_error(error);
        }
    }

    pub fn pointer_move(&self, x: f32, y: f32, ctrl: bool, shift: bool) {
        self.scene.borrow_mut().event(TimelineEvent::Motion {
            point: vec2(x, y),
            modifiers: TimelineModifiers { ctrl, shift },
        });
    }

    pub fn pointer_cursor(&self) -> shrimply_timeline_skia::view::TimelineCursor {
        self.scene.borrow().pointer_cursor()
    }

    pub fn pointer_leave(&self) {
        self.scene.borrow_mut().event(TimelineEvent::Leave);
    }

    pub fn pointer_press(
        &self,
        button: ToolkitPointerButton,
        x: f32,
        y: f32,
        ctrl: bool,
        shift: bool,
    ) {
        self.scene.borrow_mut().event(TimelineEvent::Press {
            point: vec2(x, y),
            double: false,
            modifiers: TimelineModifiers { ctrl, shift },
            button: button.into(),
        });
    }

    pub fn pointer_release(
        &self,
        button: ToolkitPointerButton,
        x: f32,
        y: f32,
        ctrl: bool,
        shift: bool,
    ) {
        self.scene.borrow_mut().event(TimelineEvent::Release {
            point: vec2(x, y),
            modifiers: TimelineModifiers { ctrl, shift },
            button: button.into(),
        });
    }

    pub unsafe fn begin_pointer_lock(
        &mut self,
        _display: *mut c_void,
        _surface: *mut c_void,
        _seat: *mut c_void,
        _software_cursor: shrimply_components_skia::cursor::SoftwareCursor,
    ) -> bool {
        false
    }

    pub fn end_pointer_lock(&mut self, _ctrl: bool, _shift: bool) {}

    pub fn scroll(&self, dx: f32, dy: f32, ctrl: bool, shift: bool) {
        let mut scene = self.scene.borrow_mut();
        let pointer = scene.pointer_state().position.unwrap_or(Vec2::ZERO);
        scene.event(TimelineEvent::Modifiers(TimelineModifiers {
            ctrl,
            shift,
        }));
        scene.scroll(
            pointer,
            vec2(dx, dy),
            ctrl,
            shrimply_timeline_skia::view::TimelineScrollInput::Wheel,
        );
    }

    pub fn prepare_context_menu(&mut self, x: f32, y: f32) -> usize {
        let mut scene = self.scene.borrow_mut();
        self.context_menu = scene.prepare_context_menu(vec2(x, y));
        self.context_file_path = scene.context_file_path();
        self.context_menu.sections.iter().map(Vec::len).sum()
    }

    pub fn context_menu(&self) -> &ContextMenu {
        &self.context_menu
    }

    pub fn set_context_menu_control(&mut self, control: ContextMenuControl, value: f64) {
        if let Err(error) = self.scene.borrow_mut().set_context_menu_control(control, value) {
            self.set_error(error);
        }
    }

    pub fn activate_context_menu_action(
        &mut self,
        action: ContextMenuAction,
    ) -> Option<ContextMenuRequest> {
        match self.scene.borrow_mut().activate_context_menu_action(action) {
            Ok(request) => {
                self.context_menu = ContextMenu::default();
                request
            }
            Err(error) => {
                self.set_error(error);
                None
            }
        }
    }

    pub fn render_context_video_frame(
        &self,
        selection: VideoFrameSelection,
    ) -> Result<RenderedVideoFrame, String> {
        let (project, position, item_ids) =
            shrimply_timeline_skia::video_selection::prepare_selected_video_frame(
                &self.project,
                &self.player_state,
                &self.selection_state,
                selection,
            );
        render_video_frame(project, position, &item_ids)
    }

    pub fn context_file_path(&self) -> Option<&Path> {
        self.context_file_path.as_deref()
    }

    pub fn delete_context_folded_track(&mut self) {
        if let Err(error) = self.scene.borrow_mut().delete_context_folded_track() {
            self.set_error(error);
        }
    }

    pub fn paste_context_clipboard_text(&self, text: String) {
        let mut scene = self.scene.borrow_mut();
        let result = if text == shrimply_timeline_qt::TIMELINE_CLIPBOARD_MARKER {
            scene.paste_context_clipboard().map(|_| true)
        } else if scene.insert_external_text(text, None) {
            Ok(true)
        } else {
            Err("Could not insert clipboard text at the playhead".to_string())
        };
        if let Err(error) = result {
            self.set_error(error);
        }
    }

    pub fn tool_state(&self) -> ToolState {
        self.tools.state()
    }

    pub fn set_magnet(&self, enabled: bool) {
        self.tools.set_magnet(enabled);
    }

    pub fn set_beat_grid(&self, enabled: bool) {
        self.tools.set_beat_grid(enabled);
    }

    pub fn set_cursor_tool(&self, cursor: CursorTool) {
        self.tools.set_cursor(cursor);
    }

    pub fn set_drag_collision_mode(&self, mode: DragCollisionMode) {
        self.tools.set_drag_collision(mode);
    }

    pub fn destroy(&mut self) {
        self.scene.borrow_mut().suspend();
        self.renderer.destroy();
    }

    fn set_error(&self, error: String) {
        *self.track_import_error.borrow_mut() = Some(error);
    }
}

impl From<ToolkitPointerButton> for PointerButton {
    fn from(button: ToolkitPointerButton) -> Self {
        match button {
            ToolkitPointerButton::Primary => Self::Primary,
            ToolkitPointerButton::Middle => Self::Middle,
        }
    }
}

fn render_video_frame(
    project: Project,
    position: Time,
    item_ids: &[uuid::Uuid],
) -> Result<RenderedVideoFrame, String> {
    let canvas_size = project.canvas_size;
    let mut renderer = shrimply_visual_cuda::compositor::VideoExportRenderer::new(48_000)?;
    let frame = renderer.render_items(&project, position, 0, item_ids)?;
    let mut rgba = ffmpeg_next::frame::Video::new(
        ffmpeg_next::format::Pixel::RGBA,
        canvas_size.width,
        canvas_size.height,
    );
    renderer.copy_to_rgba_frame(frame, &mut rgba)?;
    let width = i32::try_from(canvas_size.width)
        .map_err(|_| "selected frame width is too large".to_string())?;
    let height = i32::try_from(canvas_size.height)
        .map_err(|_| "selected frame height is too large".to_string())?;
    let row_bytes = canvas_size.width as usize * std::mem::size_of::<u32>();
    let stride = rgba.stride(0);
    let mut pixels = Vec::with_capacity(row_bytes * canvas_size.height as usize);
    for row in rgba
        .data(0)
        .chunks_exact(stride)
        .take(canvas_size.height as usize)
    {
        pixels.extend_from_slice(&row[..row_bytes]);
    }
    Ok(RenderedVideoFrame {
        width,
        height,
        pixels,
    })
}
