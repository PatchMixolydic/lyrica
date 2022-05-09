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
//! * Pause
//! * Loop
//! * Switch to a different file
//!
//! ... all without blocking the thread!
//! [Now how much do you think Lyrica is worth? ***Don't answer!***](https://www.youtube.com/watch?v=DgJS2tQPGKQ)

#![warn(clippy::single_char_lifetime_names)]

use midly::{
    live::LiveEvent,
    num::{u4, u7},
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

pub struct MidiFile<'file> {
    ticks_per_beat: u16,
    // borrowed for life from `nodi`
    microseconds_per_tick: f64,
    timer: f64,
    format: MidiFileFormat,
    tracks: Vec<VecDeque<TrackEvent<'file>>>,
    ticks_since_last_update: Vec<u32>,
    paused: bool,
}

impl<'file> MidiFile<'file> {
    pub fn from_bytes(bytes: &'file [u8]) -> Self {
        let parsed_file = Smf::parse(bytes).unwrap();

        let ticks_per_beat = match parsed_file.header.timing {
            Timing::Metrical(ticks_per_beat) => ticks_per_beat.into(),
            Timing::Timecode(_, _) => todo!("timecode timing is unimplemented"),
        };

        let tracks: Vec<VecDeque<TrackEvent<'file>>> = parsed_file
            .tracks
            .into_iter()
            .map(FromIterator::from_iter)
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

    pub fn update(&mut self, delta_time: f64, connection: &mut MidiOutputConnection) {
        if self.paused {
            return;
        }

        self.timer += delta_time;

        // TODO: assumes `format` is `Parallel`
        while self.timer > self.microseconds_per_tick {
            let tracks = self
                .ticks_since_last_update
                .iter_mut()
                .zip(self.tracks.iter_mut());

            for (ticks_since_last_update, track) in tracks {
                *ticks_since_last_update += 1;

                while let Some(event) = track.front() {
                    if event.delta > *ticks_since_last_update {
                        // Not ready to proceed yet
                        break;
                    }

                    // let's remove the event now
                    let event = track.pop_front().unwrap();
                    *ticks_since_last_update = 0;

                    match event.kind {
                        TrackEventKind::Midi { channel, message } => {
                            let event = LiveEvent::Midi { channel, message };
                            let mut event_bytes = Vec::new();
                            event.write_std(&mut event_bytes).unwrap();
                            connection.send(&event_bytes).unwrap();
                        }

                        TrackEventKind::SysEx(message) => {
                            // converting to a `LiveEvent` won't work for split SysEx,
                            // so let's do our best! `message` is missing a leading 0xF0,
                            // so we need to put it back
                            // TODO: allocation here
                            let mut event_bytes = Vec::with_capacity(message.len() + 1);
                            event_bytes.push(0xF0);
                            event_bytes.extend_from_slice(message);
                            connection.send(&event_bytes).unwrap();
                        }

                        TrackEventKind::Escape(_) => todo!("escape events are unhandled"),

                        TrackEventKind::Meta(message) => {
                            if let MetaMessage::Tempo(tempo) = message {
                                self.microseconds_per_tick =
                                    u32::from(tempo) as f64 / self.ticks_per_beat as f64;
                            }
                        }
                    }
                }
            }

            self.timer -= self.microseconds_per_tick;
        }
    }
}

pub struct MidiPlayer<'file> {
    midi_file: MidiFile<'file>,
    connection: MidiOutputConnection,
    last_update_time: Instant,
}

impl<'file> MidiPlayer<'file> {
    pub fn new(midi_file: MidiFile<'file>, connection: MidiOutputConnection) -> Self {
        Self {
            midi_file,
            connection,
            last_update_time: Instant::now(),
        }
    }

    pub fn set_midi_file(&mut self, midi_file: MidiFile<'file>) {
        all_sound_off(&mut self.connection);
        self.midi_file = midi_file;
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.midi_file.set_paused(paused, &mut self.connection);
        // Don't suddenly jump ahead when unpausing.
        self.last_update_time = Instant::now();
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let delta_time = now.duration_since(self.last_update_time).as_micros() as f64;
        self.midi_file.update(delta_time, &mut self.connection);
        self.last_update_time = now;
    }
}
