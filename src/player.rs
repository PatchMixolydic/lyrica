use midir::MidiOutputConnection;
use std::time::Instant;

use crate::{all_sound_off, file::MidiFile};

pub struct MidiPlayer {
    maybe_midi_file: Option<MidiFile>,
    connection: MidiOutputConnection,
    last_update_time: Instant,
}

impl MidiPlayer {
    pub fn new(connection: MidiOutputConnection) -> Self {
        Self {
            maybe_midi_file: None,
            connection,
            last_update_time: Instant::now(),
        }
    }

    pub fn set_midi_file(&mut self, midi_file: MidiFile) {
        all_sound_off(&mut self.connection);
        self.maybe_midi_file = Some(midi_file);
    }

    pub fn set_paused(&mut self, paused: bool) {
        if let Some(midi_file) = &mut self.maybe_midi_file {
            midi_file.set_paused(paused, &mut self.connection);
        }

        // Don't suddenly jump ahead when unpausing.
        self.last_update_time = Instant::now();
    }

    pub fn is_finished(&self) -> bool {
        match &self.maybe_midi_file {
            Some(midi_file) => midi_file.is_finished(),
            None => true,
        }
    }

    /// Sets the current MIDI file to loop at the given time in seconds.
    pub fn set_loop_point(&mut self, loop_point: Option<f64>) {
        if let Some(midi_file) = &mut self.maybe_midi_file {
            midi_file.set_loop_point(loop_point);
        }
    }

    /// Seek to the given time in seconds.
    pub fn seek_to(&mut self, seconds: f64) {
        if let Some(midi_file) = &mut self.maybe_midi_file {
            midi_file.seek_to(seconds, &mut self.connection);
        }
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let delta_time = now.duration_since(self.last_update_time).as_micros() as f64;

        if let Some(midi_file) = &mut self.maybe_midi_file {
            midi_file.update(delta_time, &mut self.connection);
        }

        self.last_update_time = now;
    }
}
