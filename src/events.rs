use midly::{
    live::LiveEvent,
    num::{u24, u28},
    MetaMessage, TrackEvent, TrackEventKind,
};

pub(crate) enum OwnedTrackEventKind {
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

pub(crate) struct OwnedTrackEvent {
    pub(crate) delta: u28,
    pub(crate) kind: OwnedTrackEventKind,
}

impl<'file> From<TrackEvent<'file>> for OwnedTrackEvent {
    fn from(event: TrackEvent<'file>) -> Self {
        Self {
            delta: event.delta,
            kind: event.kind.into(),
        }
    }
}
