//! Phantasmically simple MIDI file handling.
//!
//! Quoth [Merriam-Webster](https://www.merriam-webster.com/dictionary/phantasmic):
//! > Definition of phantasm
//! >
//! > 1: a product of fantasy: such as
//! >
//! >   a: delusive appearance : ILLUSION
//!
//! This crate provides the illusion of MIDI being really easy to work with
//! rather than a pain in your rear. As a tradeoff, it's fairly inflexible, and
//! only works with MIDI files.
//!
//! Here's what Lyrica can do with your MIDI files:
//! * Play
//! * Pause!
//! * Loop!!
//! * Switch to a different file!!!
//!
//! ... all without blocking the thread!
//! [Now how much do you think Lyrica is worth? ***Don't answer!***](https://www.youtube.com/watch?v=DgJS2tQPGKQ)

#![warn(clippy::single_char_lifetime_names)]

use midly::{
    live::LiveEvent,
    num::{u24, u28, u4, u7},
    MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
};
use std::{collections::VecDeque, time::Instant};

pub use midir::{MidiOutput, MidiOutputConnection, MidiOutputPort};

const ALL_SOUND_OFF_CC: u7 = u7::new(123);

enum MidiFileFormat {
    Sequential { current: usize },
    Parallel,
}

impl From<midly::Format> for MidiFileFormat {
    fn from(midly_format: midly::Format) -> Self {
        match midly_format {
            midly::Format::SingleTrack | midly::Format::Sequential => {
                Self::Sequential { current: 0 }
            }

            midly::Format::Parallel => Self::Parallel,
        }
    }
}

enum OwnedTrackEventKind {
    ToSynth(Vec<u8>),
    Tempo(u24),
    InessentialMeta,
}

impl<'file> From<TrackEventKind<'file>> for OwnedTrackEventKind {
    fn from(event: TrackEventKind<'file>) -> Self {
        match event {
            TrackEventKind::Midi { channel, message } => {
                let event = LiveEvent::Midi { channel, message };
                let mut event_bytes = Vec::new();
                event.write_std(&mut event_bytes).unwrap();
                Self::ToSynth(event_bytes)
            }

            TrackEventKind::SysEx(message) => {
                // converting to a `LiveEvent` won't work for split SysEx,
                // so let's do our best! `message` is missing a leading 0xF0,
                // so we need to put it back
                let mut event_bytes = Vec::with_capacity(message.len() + 1);
                event_bytes.push(0xF0);
                event_bytes.extend_from_slice(message);
                Self::ToSynth(event_bytes)
            }

            TrackEventKind::Escape(_) => todo!("MIDI escape events are unimplemented"),

            TrackEventKind::Meta(event) => match event {
                MetaMessage::Tempo(tempo) => Self::Tempo(tempo),
                _ => Self::InessentialMeta,
            },
        }
    }
}

struct OwnedTrackEvent {
    delta: u28,
    kind: OwnedTrackEventKind,
}

impl<'file> From<TrackEvent<'file>> for OwnedTrackEvent {
    fn from(event: TrackEvent<'file>) -> Self {
        Self {
            delta: event.delta,
            kind: event.kind.into(),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct TrackProgress {
    ticks_since_last_update: u32,
    next_event: usize,
}

/// Sends an [All Sound Off](http://midi.teragonaudio.com/tech/midispec/ntnoff.htm)
/// message to all channels.
fn all_sound_off(connection: &mut MidiOutputConnection) {
    let mut event_bytes = Vec::new();

    for i in 0..16 {
        let event = LiveEvent::Midi {
            channel: u4::new(i),
            message: MidiMessage::Controller {
                controller: ALL_SOUND_OFF_CC,
                value: u7::new(0),
            },
        };

        event.write_std(&mut event_bytes).unwrap();
        connection.send(&event_bytes).unwrap();
        event_bytes.clear();
    }
}

pub struct MidiFile {
    ticks_per_beat: u16,
    // borrowed for life from `nodi`
    microseconds_per_tick: f64,
    timer: f64,
    loop_point: Option<f64>,
    format: MidiFileFormat,
    tracks: Vec<VecDeque<OwnedTrackEvent>>,
    progress: Vec<TrackProgress>,
    paused: bool,
}

impl MidiFile {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let parsed_file = Smf::parse(bytes).unwrap();

        let ticks_per_beat = match parsed_file.header.timing {
            Timing::Metrical(ticks_per_beat) => ticks_per_beat.into(),
            Timing::Timecode(_, _) => todo!("timecode timing is unimplemented"),
        };

        // This looks like this performs far too many allocations, but
        // in the optimal case, the parsing library would make most of
        // the allocations here. There are only `tracks.len()` + 1 extra allocations:
        // `tracks.len()` to collect into `VecDeque`s, and one to collect into
        // a `Vec`. If the parsing library also used `Vec<VecDeque<_>>`, this would
        // need no extra allocations.
        let tracks: Vec<VecDeque<OwnedTrackEvent>> = parsed_file
            .tracks
            .into_iter()
            .map(|track| track.into_iter().map(OwnedTrackEvent::from).collect())
            .collect();

        let progress = vec![Default::default(); tracks.len()];

        Self {
            ticks_per_beat,
            microseconds_per_tick: 0.0,
            timer: 0.0,
            loop_point: None,
            format: parsed_file.header.format.into(),
            tracks,
            progress,
            paused: false,
        }
    }

    pub fn set_paused(&mut self, paused: bool, connection: &mut MidiOutputConnection) {
        self.paused = paused;

        if paused {
            all_sound_off(connection);
        }
    }

    // TODO: is passing `None` here useful?
    pub fn set_loop_point(&mut self, loop_point: Option<f64>) {
        self.loop_point = loop_point;
    }

    /// Like [`Self::is_finished`], but ignores the loop point.
    fn at_end_of_track(&self) -> bool {
        match self.format {
            MidiFileFormat::Sequential { current } => self.tracks.len() <= current,

            MidiFileFormat::Parallel => self
                .tracks
                .iter()
                .zip(self.progress.iter())
                .all(|(track, progress)| progress.next_event >= track.len()),
        }
    }

    pub fn is_finished(&self) -> bool {
        if self.loop_point.is_some() {
            return false;
        }

        self.at_end_of_track()
    }

    /// Seek to the given time in seconds.
    pub fn seek_to(&mut self, seconds: f64, connection: &mut MidiOutputConnection) {
        all_sound_off(connection);
        let loop_point_in_ticks = (seconds * 1_000_000.0 / self.microseconds_per_tick) as u32;

        for track_id in 0..self.tracks.len() {
            let mut cumulative_delta = 0;

            for (i, event) in self.tracks[track_id].iter().enumerate() {
                if cumulative_delta + event.delta.as_int() > loop_point_in_ticks {
                    self.progress[track_id].next_event = i;
                    break;
                }

                cumulative_delta += event.delta.as_int();
            }

            // `cumulative_delta` is the time needed to get to the event before
            // `self.progress[track_id].next_event` in ticks.
            self.progress[track_id].ticks_since_last_update =
                loop_point_in_ticks.saturating_sub(cumulative_delta);
        }
    }

    fn update_track(&mut self, track_id: usize, connection: &mut MidiOutputConnection) {
        let track = &self.tracks[track_id];
        let progress = &mut self.progress[track_id];
        progress.ticks_since_last_update += 1;

        while progress.next_event < track.len() {
            let event = &track[progress.next_event];
            if event.delta > progress.ticks_since_last_update {
                // Not ready to proceed yet
                break;
            }

            // update!
            progress.ticks_since_last_update = 0;
            progress.next_event += 1;

            match &event.kind {
                OwnedTrackEventKind::ToSynth(event_bytes) => {
                    connection.send(event_bytes).unwrap();
                }

                OwnedTrackEventKind::Tempo(tempo) => {
                    self.microseconds_per_tick =
                        u32::from(*tempo) as f64 / self.ticks_per_beat as f64;
                }

                OwnedTrackEventKind::InessentialMeta => {}
            }
        }

        if self.at_end_of_track() {
            if let Some(loop_point) = self.loop_point {
                self.seek_to(loop_point, connection);
            }
        }
    }

    pub fn update(&mut self, delta_time: f64, connection: &mut MidiOutputConnection) {
        if self.paused || self.is_finished() {
            return;
        }

        self.timer += delta_time;

        while self.timer > self.microseconds_per_tick {
            match self.format {
                MidiFileFormat::Sequential { current } => {
                    self.update_track(current, connection);

                    if self.tracks[current].is_empty() {
                        // This track is finished; play the next track.
                        // If this is the last track, this will cause
                        // `current` to go out of the range of valid track
                        // indices. This will make `Self::is_finished`
                        // return `true`, skipping any future updates and
                        // avoiding "index out of bounds" panics.
                        // TODO: this will have to reset if looping is
                        // enabled
                        self.format = MidiFileFormat::Sequential {
                            current: current + 1,
                        };
                    }
                }

                MidiFileFormat::Parallel => {
                    for track_id in 0..self.tracks.len() {
                        self.update_track(track_id, connection);
                    }
                }
            }

            self.timer -= self.microseconds_per_tick;
        }
    }
}

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
