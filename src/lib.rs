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

mod events;
pub mod file;
pub mod player;

use midly::{
    live::LiveEvent,
    num::{u4, u7},
    MidiMessage,
};

pub use midir::{MidiOutput, MidiOutputConnection, MidiOutputPort};

pub use crate::{file::MidiFile, player::MidiPlayer};

const ALL_SOUND_OFF_CC: u7 = u7::new(123);

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
