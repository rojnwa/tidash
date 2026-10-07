//! Plays a file on the default output: `cargo run -p player --example play_file -- <path.mp4>`.
#![allow(clippy::print_stdout)]

use std::time::{Duration, Instant};

use player::{PlayState, Player, PlayerCmd, PlayerEvent, Source};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: play_file <path.mp4>")?;
    let file = std::fs::File::open(&path)?;
    let len = file.metadata()?.len();
    let (player, events) = Player::spawn(fastframe_audio::OutputOptions::default())?;
    let started = Instant::now();
    player.send(PlayerCmd::Load(Source {
        id: 1,
        reader: Box::new(file),
        len: Some(len),
    }));
    player.send(PlayerCmd::Play);
    loop {
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
