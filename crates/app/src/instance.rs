//! One Tidash per user (fastframe-instance): a later launch hands its link, or a Show, to
//! the running one and ends.

use crate::link::{self, Link};
use crate::tray::TrayAction;
use fastframe_instance::{Claim, Guard, Slot};
use fastframe_shell::Waker;
use std::sync::{Arc, Mutex, PoisonError};

/// What later launches asked for, for the app to take on its own thread.
pub type Inbox = Arc<Mutex<Vec<TrayAction>>>;

/// This process becomes the running Tidash, its guard kept until it exits, or hands its
/// request to the one already running and gets `None`: it then ends. The request is `link`,
/// else a Show. The running one queues what later launches ask for in `inbox` and wakes the
/// app.
pub fn claim(link: Option<&Link>, waker: &Waker, inbox: Inbox) -> Option<Guard> {
    claim_in(&Slot::new("tidash"), link, waker, inbox)
}

/// Queues `asked` for the app and wakes it.
pub fn hand_over(inbox: &Inbox, waker: &Waker, asked: TrayAction) {
    inbox
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(asked);
    waker.wake();
}

/// Hands `text` over as a link if it parses as one; `false` for anything else.
#[cfg_attr(
    not(any(target_os = "macos", test)),
    expect(
        dead_code,
        reason = "only macOS's Apple Events (`mac_links`) hand links over here"
    )
)]
pub fn open_link(inbox: &Inbox, waker: &Waker, text: &str) -> bool {
    let opened = link::parse(text)
        .map(|l| hand_over(inbox, waker, TrayAction::Open(l)))
        .is_some();
    if !opened {
        // Not the text: it comes from outside and may be long.
        log::warn!("declined a link that is not a TIDAL link");
    }
    opened
}

fn claim_in(slot: &Slot, link: Option<&Link>, waker: &Waker, inbox: Inbox) -> Option<Guard> {
    // Every launch without a link is the user asking for the window, even with Start
    // minimized on. `ping` is answered (nothing shown) but nothing sends it: start at login
    // is not built (docs/follow-ups.md).
    let request = link.map_or("show".into(), |link| format!("open {}", link::to_uri(link)));
    match slot.claim(&request, handler(waker.clone(), inbox)) {
        Claim::First(guard) => Some(guard),
        Claim::Running(_) => None,
        // A second copy would double the tray item, the media controls and the player.
        #[expect(
            clippy::print_stderr,
            reason = "this launch ends before logging starts"
        )]
        other => {
            let why =
                format!("Tidash runs already and did not take this launch's request: {other:?}");
            log::warn!("{why}");
            eprintln!("{why}");
            None
        }
    }
}

/// Answers a later launch, on the channel's thread: `show`, `ping`, or `open` with a link that
/// parses here again, as the other process is not trusted. Queues what it asks for and wakes
/// the app; anything else is declined.
fn handler(waker: Waker, inbox: Inbox) -> impl FnMut(&str) -> Option<String> + Send + 'static {
    move |request| {
        let asked = match request {
            "ping" => return Some("ok".into()),
            "show" => TrayAction::Show,
            _ => TrayAction::Open(link::parse(request.strip_prefix("open ")?)?),
        };
        hand_over(&inbox, &waker, asked);
        Some("ok".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(inbox: &Inbox) -> String {
        format!("{:?}", inbox.lock().expect("the inbox"))
    }

    #[test]
    fn instance_handler_accepts_only_links() {
        let inbox = Inbox::default();
        let mut handle = handler(Waker::default(), Arc::clone(&inbox));
        assert_eq!(handle("open tidal://album/1").as_deref(), Some("ok"));
        assert_eq!(asked(&inbox), "[Open(Album(1))]");
        for declined in [
            "open https://evil.com/x",
            "open tidal://track/1%2F..%2Fusers",
            "open",
            "rm -rf",
            "",
        ] {
            assert_eq!(handle(declined), None, "{declined}");
        }
        assert_eq!(asked(&inbox), "[Open(Album(1))]", "nothing more queued");
        assert_eq!(handle("show").as_deref(), Some("ok"));
        assert_eq!(handle("ping").as_deref(), Some("ok"));
        assert_eq!(
            asked(&inbox),
            "[Open(Album(1)), Show]",
            "ping asks for nothing"
        );
    }

    #[test]
    fn open_link_takes_only_links() {
        let (waker, inbox) = (Waker::default(), Inbox::default());
        assert!(open_link(&inbox, &waker, "tidal://album/123"));
        assert_eq!(asked(&inbox), "[Open(Album(123))]");
        let long = format!("tidal://album/{}", "1".repeat(20_000));
        for declined in [
            "tidal://track/1%2F..%2Fusers",
            "spotify:track:1",
            "https://evil.example/album/1",
            &long,
        ] {
            assert!(!open_link(&inbox, &waker, declined), "{declined}");
        }
        assert_eq!(asked(&inbox), "[Open(Album(123))]", "nothing more queued");
    }

    /// A slot of its own in a throwaway directory: a Tidash running on this machine is
    /// left alone.
    #[test]
    fn a_second_launch_hands_its_link_to_the_first() {
        let dir = std::env::temp_dir().join(format!("tidash-instance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let slot = Slot::at(&dir, "tidash");
        let (waker, inbox) = (Waker::default(), Inbox::default());
        let first = claim_in(&slot, None, &waker, Arc::clone(&inbox));
        assert!(first.is_some(), "the first launch runs");
        let album = Link::Album(1);
        let second = claim_in(&slot, Some(&album), &waker, Inbox::default());
        assert!(second.is_none(), "the second launch ends");
        // With Start minimized on too: a relaunch is the user asking for the window.
        let third = claim_in(&slot, None, &waker, Inbox::default());
        assert!(third.is_none());
        assert_eq!(asked(&inbox), "[Open(Album(1)), Show]");
        drop(first);
        let _ = std::fs::remove_dir_all(dir);
    }
}
