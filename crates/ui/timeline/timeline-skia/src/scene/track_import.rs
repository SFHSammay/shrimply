use super::*;
use std::path::PathBuf;

impl Scene {
    pub fn activate_track_add(
        &mut self,
        key: TrackKey,
        action: TrackAddAction,
    ) -> Result<TrackAddOutcome, String> {
        crate::activate_track_add_checked(
            &self.project,
            &self.player,
            &self.selection,
            key,
            action,
            TrackAddSettings {
                default_visual_duration: self.default_visual_duration,
                default_text_font_family: &self.default_text_font_family,
            },
        )
    }

    pub fn import_track_file(
        &mut self,
        path: PathBuf,
        targets: &[TrackKey],
    ) -> Result<crate::import_queue::ImportStart, String> {
        self.external_imports.enqueue_tracks(
            vec![path],
            &self.project.borrow(),
            targets,
            player_state::current_time(&self.player),
            self.default_visual_duration,
        )
    }
}
