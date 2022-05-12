use lyrica::{MidiFile, MidiOutput, MidiPlayer};
use std::{
    env,
    error::Error,
    fs,
    io::{stdin, stdout, Write},
    thread,
    time::Duration,
};

fn main() -> Result<(), Box<dyn Error>> {
    let args = env::args().collect::<Vec<_>>();

    if args.len() != 2 {
        println!("usage: {} [filename]", args[0]);

        return Err(format!(
            "incorrect number of arguments (expected 1, got {})",
            args.len() - 1
        )
        .into());
    }

    let filename = &args[1];

    let midi_output = MidiOutput::new("lyrica-play")?;
    let midi_ports = midi_output.ports();

    let port_number = loop {
        println!("Available MIDI ports:");
        for (i, port) in midi_ports.iter().enumerate() {
            println!("{}: {}", i, midi_output.port_name(port)?);
        }

        print!("Please select a port: ");
        stdout().flush()?;
        let mut input = String::new();
        stdin().read_line(&mut input)?;
        let trimmed_input = input.trim();

        let port_number = match trimmed_input.parse::<usize>() {
            Ok(port_number) => port_number,

            Err(_) => {
                println!("port number must be a number (got {trimmed_input})");
                continue;
            }
        };

        if port_number > midi_output.port_count() {
            println!(
                "port number is too high (last port is {})",
                midi_output.port_count()
            );
            continue;
        }

        break port_number;
    };

    let connection = midi_output.connect(&midi_ports[port_number], "lyrica-play")?;
    let file = fs::read(filename)?;
    let midi_file = MidiFile::from_bytes(&file);
    let mut player = MidiPlayer::new(connection);
    player.set_midi_file(midi_file);

    while !player.is_finished() {
        player.update();
        // TODO: Not entirely sure why this is needed, but without this, playback freezes
        // after the first note. Might be because `update` is executed so fast that
        // my slipshod code can't handle it.
        thread::sleep(Duration::from_micros(1));
    }

    Ok(())
}
