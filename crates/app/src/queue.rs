//! The play queue: a track list and the current position in it. Unavailable tracks
//! (`stream_ready == false`) are never current.

use crate::model::QueueSnapshot;
use tidal::Track;

#[derive(Default)]
pub struct Queue {
    tracks: Vec<Track>,
    current: Option<usize>,
}

impl Queue {
    /// Replaces the list; the first playable track at or after `start` becomes current.
    pub fn replace(&mut self, tracks: Vec<Track>, start: usize) -> Option<&Track> {
        self.tracks = tracks;
        self.current = self.ready_from(start);
        self.current()
    }

    pub fn current(&self) -> Option<&Track> {
        self.tracks.get(self.current?)
    }

    /// The next playable track becomes current; `None` (staying put) at the end.
    pub fn next(&mut self) -> Option<&Track> {
        self.current = Some(self.ready_from(self.current? + 1)?);
        self.current()
    }

    /// The previous playable track becomes current; `None` (staying put) at the start.
    pub fn previous(&mut self) -> Option<&Track> {
        let before = self.tracks.get(..self.current?)?;
        self.current = Some(before.iter().rposition(|t| t.stream_ready)?);
        self.current()
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            tracks: self.tracks.clone(),
            current: self.current,
        }
    }

    fn ready_from(&self, start: usize) -> Option<usize> {
        Some(
            start
                + self
                    .tracks
                    .get(start..)?
                    .iter()
                    .position(|t| t.stream_ready)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tracks `0..ready.len()`, id = index, available as given.
    fn tracks(ready: &[bool]) -> Vec<Track> {
        ready
            .iter()
            .enumerate()
            .map(|(i, &stream_ready)| Track {
                id: i as u64,
                title: format!("t{i}"),
                duration: 100,
                track_number: i as u32 + 1,
                volume_number: 1,
                artists: vec![],
                album: tidal::AlbumRef {
                    id: 9,
                    title: "a".into(),
                    cover: None,
                },
                stream_ready,
            })
            .collect()
    }

    fn id(t: Option<&Track>) -> Option<u64> {
        t.map(|t| t.id)
    }

    #[test]
    fn replace_skips_unavailable_start() {
        let mut q = Queue::default();
        assert_eq!(id(q.replace(tracks(&[true, false, true]), 1)), Some(2));
        assert_eq!(id(q.current()), Some(2));
        assert_eq!(
            id(q.replace(tracks(&[true, false]), 1)),
            None,
            "nothing playable from start"
        );
        assert_eq!(id(q.current()), None);
    }

    #[test]
    fn next_skips_unavailable_and_ends() {
        let mut q = Queue::default();
        assert_eq!(
            id(q.replace(tracks(&[true, false, true, false]), 0)),
            Some(0)
        );
        assert_eq!(id(q.next()), Some(2));
        assert_eq!(id(q.next()), None);
        assert_eq!(id(q.current()), Some(2), "stays on the last playable track");
    }

    #[test]
    fn previous_stops_at_start() {
        let mut q = Queue::default();
        assert_eq!(
            id(q.replace(tracks(&[false, true, false, true]), 3)),
            Some(3)
        );
        assert_eq!(id(q.previous()), Some(1));
        assert_eq!(id(q.previous()), None);
        assert_eq!(
            id(q.current()),
            Some(1),
            "stays on the first playable track"
        );
        assert_eq!(id(Queue::default().previous()), None);
    }

    #[test]
    fn snapshot_reflects_current() {
        let mut q = Queue::default();
        q.replace(tracks(&[true, true, true]), 1);
        q.next();
        let s = q.snapshot();
        assert_eq!(s.tracks.len(), 3);
        assert_eq!(s.current, Some(2));
    }
}
