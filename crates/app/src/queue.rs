//! The play queue: a track list, the current position in it, shuffle and repeat. Unavailable
//! tracks (`stream_ready == false`) are never current.

use crate::model::QueueSnapshot;
use rand::seq::SliceRandom;
use tidal::Track;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    /// After the last track, the first.
    All,
    /// The current track again at its natural end.
    One,
}

struct Entry {
    track: Track,
    /// Its place in the unshuffled order, which shuffle off sorts back to; ties keep the play
    /// order. Meaningless while unshuffled.
    rank: usize,
}

#[derive(Default)]
pub struct Queue {
    entries: Vec<Entry>,
    current: Option<usize>,
    shuffle: bool,
    repeat: Repeat,
}

impl Queue {
    /// Replaces the list; the first playable track at or after `start` becomes current. With
    /// shuffle on, the tracks after it are shuffled.
    pub fn replace(&mut self, tracks: Vec<Track>, start: usize) -> Option<&Track> {
        self.entries = tracks
            .into_iter()
            .map(|track| Entry { track, rank: 0 })
            .collect();
        self.current = self.ready_from(start);
        if self.shuffle {
            self.shuffle_rest();
        }
        self.current()
    }

    pub fn current(&self) -> Option<&Track> {
        self.entries.get(self.current?).map(|e| &e.track)
    }

    /// The next playable track becomes current; at the end, with repeat on, the first one, else
    /// `None` (staying put).
    pub fn next(&mut self) -> Option<&Track> {
        self.current = Some(self.following(self.current? + 1)?);
        self.current()
    }

    /// At a track's natural end: `Repeat::One` keeps it current, else as `next`.
    pub fn advance(&mut self) -> Option<&Track> {
        if self.repeat == Repeat::One {
            self.current()
        } else {
            self.next()
        }
    }

    /// The previous playable track becomes current; `None` (staying put) at the start.
    pub fn previous(&mut self) -> Option<&Track> {
        let before = self.entries.get(..self.current?)?;
        self.current = Some(before.iter().rposition(|e| e.track.stream_ready)?);
        self.current()
    }

    /// The first playable track at or after `i` becomes current, the tracks between staying;
    /// `None` (staying put) if there is none.
    pub fn jump(&mut self, i: usize) -> Option<&Track> {
        self.current = Some(self.ready_from(i)?);
        self.current()
    }

    /// The tracks go right after the current one (at the end without one).
    pub fn insert_next(&mut self, tracks: Vec<Track>) {
        let at = self.current.map_or(self.entries.len(), |c| c + 1);
        // Unshuffled, they follow the current track there too.
        let rank = self.current.map_or(usize::MAX, |c| self.entries[c].rank);
        self.insert(at, rank, tracks);
    }

    pub fn append(&mut self, tracks: Vec<Track>) {
        self.insert(self.entries.len(), usize::MAX, tracks);
    }

    /// Moves the track at `from` to the slot `to` (`0..=len`, counted before the move); the
    /// current track stays current. Out of range does nothing.
    pub fn move_item(&mut self, from: usize, to: usize) {
        let len = self.entries.len();
        if from >= len || to > len {
            return;
        }
        let to = if to > from { to - 1 } else { to };
        let moved = self.entries.remove(from);
        self.entries.insert(to, moved);
        self.current = self.current.map(|c| match c {
            c if c == from => to,
            c if from < c && c <= to => c - 1,
            c if to <= c && c < from => c + 1,
            c => c,
        });
    }

    /// Removes the track at `i`; true if it was current. Then the next playable track (as
    /// `next` picks it) is current, or none.
    pub fn remove(&mut self, i: usize) -> bool {
        if i >= self.entries.len() {
            return false;
        }
        self.entries.remove(i);
        match self.current {
            Some(c) if c == i => {
                self.current = self.following(i);
                true
            }
            Some(c) if c > i => {
                self.current = Some(c - 1);
                false
            }
            _ => false,
        }
    }

    /// On, the tracks after the current one are shuffled; off, the unshuffled order returns,
    /// the current track staying current.
    pub fn set_shuffle(&mut self, on: bool) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            return self.shuffle_rest();
        }
        let mut entries: Vec<_> = std::mem::take(&mut self.entries)
            .into_iter()
            .enumerate()
            .collect();
        entries.sort_by_key(|(_, e)| e.rank); // stable: ties keep the play order
        self.current = self
            .current
            .and_then(|c| entries.iter().position(|(i, _)| *i == c));
        self.entries = entries.into_iter().map(|(_, e)| e).collect();
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        QueueSnapshot {
            tracks: self.entries.iter().map(|e| e.track.clone()).collect(),
            current: self.current,
            shuffle: self.shuffle,
            repeat: self.repeat,
        }
    }

    /// Ranks the play order as it is, then shuffles what comes after the current track.
    fn shuffle_rest(&mut self) {
        for (rank, e) in self.entries.iter_mut().enumerate() {
            e.rank = rank;
        }
        let after = self.current.map_or(0, |c| c + 1);
        self.entries[after..].shuffle(&mut rand::rng());
    }

    /// Inserts at `at`, never before the current track; with none, the first playable one
    /// inserted becomes current.
    fn insert(&mut self, at: usize, rank: usize, tracks: Vec<Track>) {
        let entries = tracks.into_iter().map(|track| Entry { track, rank });
        self.entries.splice(at..at, entries);
        if self.current.is_none() {
            self.current = self.ready_from(at);
        }
    }

    /// The first playable track at or after `i`; with repeat on, past the end the first one.
    fn following(&self, i: usize) -> Option<usize> {
        match self.ready_from(i) {
            None if self.repeat != Repeat::Off => self.ready_from(0),
            found => found,
        }
    }

    fn ready_from(&self, start: usize) -> Option<usize> {
        let rest = self.entries.get(start..)?;
        Some(start + rest.iter().position(|e| e.track.stream_ready)?)
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

    /// Available tracks with these ids.
    fn ready(ids: std::ops::Range<u64>) -> Vec<Track> {
        let t = tracks(&[true]).remove(0);
        ids.map(|id| Track { id, ..t.clone() }).collect()
    }

    fn id(t: Option<&Track>) -> Option<u64> {
        t.map(|t| t.id)
    }

    fn ids(q: &Queue) -> Vec<u64> {
        q.snapshot().tracks.iter().map(|t| t.id).collect()
    }

    #[test]
    fn insert_next_after_current() {
        let mut q = Queue::default();
        q.replace(tracks(&[true, true, true]), 1);
        q.insert_next(ready(10..12));
        assert_eq!(ids(&q), [0, 1, 10, 11, 2]);
        assert_eq!(id(q.current()), Some(1));
        assert_eq!(id(q.next()), Some(10));
        let mut q = Queue::default();
        q.insert_next(tracks(&[false, true]));
        assert_eq!(id(q.current()), Some(1), "an empty queue's first playable");
    }

    #[test]
    fn append_at_end() {
        let mut q = Queue::default();
        q.replace(tracks(&[true, true]), 0);
        q.append(ready(10..12));
        assert_eq!(ids(&q), [0, 1, 10, 11]);
        assert_eq!(id(q.current()), Some(0));
        let mut q = Queue::default();
        q.append(tracks(&[false, true]));
        assert_eq!(id(q.current()), Some(1), "an empty queue's first playable");
    }

    #[test]
    fn move_keeps_current_track() {
        let mut q = Queue::default();
        q.replace(tracks(&[true; 5]), 2);
        let at = |q: &Queue| (ids(q), q.snapshot().current);
        q.move_item(2, 0); // the current one, up
        assert_eq!(at(&q), (vec![2, 0, 1, 3, 4], Some(0)));
        q.move_item(0, 3); // the current one, down
        assert_eq!(at(&q), (vec![0, 1, 2, 3, 4], Some(2)));
        q.move_item(4, 0); // from after the current one to before it
        assert_eq!(at(&q), (vec![4, 0, 1, 2, 3], Some(3)));
        q.move_item(0, 5); // from before it to after it: the end slot
        assert_eq!(at(&q), (vec![0, 1, 2, 3, 4], Some(2)));
        q.move_item(4, 3); // both after it
        assert_eq!(at(&q), (vec![0, 1, 2, 4, 3], Some(2)));
        q.move_item(9, 0);
        q.move_item(0, 6);
        assert_eq!(at(&q), (vec![0, 1, 2, 4, 3], Some(2)), "out of range");
    }

    #[test]
    fn remove_before_current_shifts_index() {
        let mut q = Queue::default();
        q.replace(tracks(&[true; 4]), 2);
        assert!(!q.remove(0));
        assert_eq!(ids(&q), [1, 2, 3]);
        assert_eq!((q.snapshot().current, id(q.current())), (Some(1), Some(2)));
        assert!(!q.remove(2), "after it");
        assert!(!q.remove(9), "out of range");
        assert_eq!((q.snapshot().current, id(q.current())), (Some(1), Some(2)));
    }

    #[test]
    fn remove_current_advances() {
        let mut q = Queue::default();
        q.replace(tracks(&[true, true, false, true]), 1);
        assert!(q.remove(1));
        assert_eq!(id(q.current()), Some(3), "the next playable");
        assert!(q.remove(2));
        assert_eq!(id(q.current()), None, "none after it");
        q.set_repeat(Repeat::All);
        q.replace(tracks(&[true, true]), 1);
        assert!(q.remove(1));
        assert_eq!(id(q.current()), Some(0), "repeat all wraps, as next does");
    }

    #[test]
    fn shuffle_keeps_current_and_restores_order() {
        let mut q = Queue::default();
        q.replace(tracks(&[true; 50]), 10);
        q.set_shuffle(true);
        assert!(q.snapshot().shuffle);
        assert_eq!(
            (id(q.current()), q.snapshot().current),
            (Some(10), Some(10))
        );
        let order = ids(&q);
        assert_eq!(
            order[..11],
            *(0..11).collect::<Vec<_>>(),
            "up to it, as it was"
        );
        let mut rest = order[11..].to_vec();
        assert_ne!(rest, (11..50).collect::<Vec<_>>(), "the rest shuffled");
        rest.sort();
        assert_eq!(rest, (11..50).collect::<Vec<_>>());
        let now = id(q.next());
        // Added while shuffled: back on, they follow the track then current, or end the list.
        q.insert_next(ready(100..102));
        q.append(ready(200..201));
        q.set_shuffle(false);
        assert!(!q.snapshot().shuffle);
        assert_eq!(id(q.current()), now, "the current track stays current");
        let mut want: Vec<u64> = (0..50).collect();
        let after = now.expect("a current track") as usize + 1;
        want.splice(after..after, [100, 101]);
        want.push(200);
        assert_eq!(ids(&q), want);
        q.set_shuffle(true);
        q.replace(tracks(&[true; 50]), 0);
        assert_eq!(ids(&q)[0], 0);
        assert_ne!(ids(&q), (0..50).collect::<Vec<_>>(), "a new list, shuffled");
    }

    #[test]
    fn repeat_all_wraps_and_one_repeats_on_advance_only() {
        let mut q = Queue::default();
        q.replace(tracks(&[false, true, true, false]), 2);
        assert_eq!(id(q.next()), None, "off: the end");
        assert_eq!(id(q.advance()), None);
        q.set_repeat(Repeat::All);
        assert_eq!(q.snapshot().repeat, Repeat::All);
        assert_eq!(id(q.advance()), Some(1), "all: the first playable");
        assert_eq!(id(q.next()), Some(2));
        assert_eq!(id(q.next()), Some(1));
        q.set_repeat(Repeat::One);
        assert_eq!(id(q.advance()), Some(1), "one: the same track at its end");
        assert_eq!(id(q.advance()), Some(1));
        assert_eq!(id(q.next()), Some(2), "a skip moves on");
        assert_eq!(id(q.next()), Some(1), "and wraps, as with all");
    }

    #[test]
    fn jump_skips_nothing_and_sets_current() {
        let mut q = Queue::default();
        q.replace(tracks(&[true, true, false, true, false]), 0);
        assert_eq!(id(q.jump(3)), Some(3));
        assert_eq!(ids(&q), [0, 1, 2, 3, 4], "nothing dropped");
        assert_eq!(id(q.previous()), Some(1), "the tracks before it stay");
        assert_eq!(id(q.jump(2)), Some(3), "unavailable: the next playable");
        assert_eq!(id(q.jump(4)), None, "none playable from there");
        assert_eq!(id(q.jump(9)), None);
        assert_eq!(id(q.current()), Some(3), "stays put");
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
