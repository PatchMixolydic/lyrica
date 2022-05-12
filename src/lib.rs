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
use std::{collections::VecDeque, time::Instant, vec};

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
    format: MidiFileFormat,
    tracks: Vec<VecDeque<OwnedTrackEvent>>,
    ticks_since_last_update: Vec<u32>,
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

        let ticks_since_last_update = vec![0; tracks.len()];

        Self {
            ticks_per_beat,
            microseconds_per_tick: 0.0,
            timer: 0.0,
            format: parsed_file.header.format.into(),
            tracks,
            ticks_since_last_update,
            paused: false,
        }
    }

    pub fn set_paused(&mut self, paused: bool, connection: &mut MidiOutputConnection) {
        self.paused = paused;

        if paused {
            all_sound_off(connection);
        }
    }

    pub fn is_finished(&self) -> bool {
        match self.format {
            MidiFileFormat::Sequential { current } => self.tracks.len() <= current,
            MidiFileFormat::Parallel => self.tracks.iter().all(|track| track.is_empty()),
        }
    }

    fn update_track(&mut self, track_id: usize, connection: &mut MidiOutputConnection) {
        self.ticks_since_last_update[track_id] += 1;

        while let Some(event) = self.tracks[track_id].front() {
            if event.delta > self.ticks_since_last_update[track_id] {
                // Not ready to proceed yet
                break;
            }

            // let's remove the event now
            let event = self.tracks[track_id].pop_front().unwrap();
            self.ticks_since_last_update[track_id] = 0;

            match event.kind {
                OwnedTrackEventKind::ToSynth(event_bytes) => {
                    connection.send(&event_bytes).unwrap();
                }

                OwnedTrackEventKind::Tempo(tempo) => {
                    self.microseconds_per_tick =
                        u32::from(tempo) as f64 / self.ticks_per_beat as f64;
                }

                OwnedTrackEventKind::InessentialMeta => {}
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

    pub fn update(&mut self) {
        let now = Instant::now();
        let delta_time = now.duration_since(self.last_update_time).as_micros() as f64;

        if let Some(midi_file) = &mut self.maybe_midi_file {
            midi_file.update(delta_time, &mut self.connection);
        }

        self.last_update_time = now;
    }
}
