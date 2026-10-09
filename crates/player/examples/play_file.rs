//! Plays a file on the default output: `cargo run -p player --example play_file --
//! [--exclusive] <path.mp4> [next.mp4]`. A second file is preloaded, to hear that it follows
//! without a gap. `--exclusive` holds the device at each track's own format, if it lets us.
#![allow(clippy::print_stdout)]

use std::time::{Duration, Instant};

use player::{OutputConfig, PlayState, Player, PlayerCmd, PlayerEvent, Source};

fn source(id: u64, path: &str) -> std::io::Result<Source> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    Ok(Source {
        id,
        reader: Box::new(file),
        len: Some(len),
        gain: 1.0,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (flags, paths): (Vec<_>, Vec<_>) =
        std::env::args().skip(1).partition(|a| a == "--exclusive");
    let mut paths = paths.iter();
    let path = paths
        .next()
        .ok_or("usage: play_file [--exclusive] <path.mp4> [next.mp4]")?;
    let first = source(1, path)?;
    let next = paths.next().map(|p| source(2, p)).transpose()?;
    let config = OutputConfig {
        exclusive: !flags.is_empty(),
        ..OutputConfig::default()
    };
    let (player, events) = Player::spawn(config)?;
    let started = Instant::now();
    player.send(PlayerCmd::Load(first));
    if next.is_some() {
        player.send(PlayerCmd::Preload(next));
    }
    player.send(PlayerCmd::Play);
    loop {
        // Every event, `Format` and `Fallback` among them.
        let event = events.recv_timeout(Duration::from_secs(60))?;
        println!(
            "{:>6} ms  {event:?}  position {:?}",
            started.elapsed().as_millis(),
            player.position()
        );
        if event == PlayerEvent::State(PlayState::Stopped) {
            return Ok(());
        }
    }
}
