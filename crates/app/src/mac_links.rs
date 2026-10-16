//! TIDAL links macOS hands the app.
//!
//! An app registered for the `tidal` scheme (`CFBundleURLTypes` in
//! `packaging/macos/Info.plist`) gets each link as an Apple Event, never on the command line:
//! the one it was launched for as well as the ones that arrive while it runs. The handler here
//! puts each in the inbox a later launch hands its link to (`instance`), where it is treated
//! like any other.
//!
//! Adapted from Spotifast's `mac_links.rs` (MIT, © Carmine Paolino).

use crate::instance::{self, Inbox};
use fastframe_shell::Waker;
use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSObject};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

/// `kInternetEventClass` and `kAEGetURL`: both the four characters `GURL`, the Apple Event a
/// URL open is.
const GET_URL: u32 = u32::from_be_bytes(*b"GURL");
/// `keyDirectObject`, the parameter that carries the URL.
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

/// Where links go, and the wake that makes the app read them.
static SINK: Mutex<Option<(Inbox, Waker)>> = Mutex::new(None);

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    // Objective-C class names are global per process.
    #[name = "TidashLinkHandler"]
    struct TidashLinkHandler;

    impl TidashLinkHandler {
        // SAFETY: the signature `setEventHandler:andSelector:…` documents for its handler: two
        // `NSAppleEventDescriptor`s, no return value.
        #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
        #[expect(unsafe_code, reason = "the event's parameter is read through `msg_send!`")]
        fn handle_get_url(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            // SAFETY: `paramDescriptorForKeyword:` takes an `AEKeyword` (a `u32`) and returns a
            // descriptor or nil; the typed binding only differs in wanting Core Services for
            // the keyword's type.
            let url: Option<Retained<NSAppleEventDescriptor>> =
                unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
            if let Some(text) = url.and_then(|url| url.stringValue()) {
                deliver(&text.to_string());
            }
        }
    }
);

/// Answers the desktop's links for the rest of the process. Main thread only, before the
/// event loop that delivers Apple Events starts.
#[expect(
    unsafe_code,
    reason = "the handler is made and registered through `msg_send!`"
)]
pub fn install(inbox: Inbox, waker: Waker) {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!("links can only be taken on the main thread");
        return;
    };
    *SINK.lock().unwrap_or_else(PoisonError::into_inner) = Some((inbox, waker));
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    // SAFETY: `init` is NSObject's initializer, which the class inherits unchanged.
    let handler: Retained<TidashLinkHandler> =
        unsafe { msg_send![mtm.alloc::<TidashLinkHandler>(), init] };
    let target: &NSObject = &handler;
    let manager = NSAppleEventManager::sharedAppleEventManager();
    // SAFETY: the handler answers the selector with the signature the manager calls it with,
    // and the event class and ID are `AEEventClass`/`AEEventID`, both `u32`. The typed binding
    // only differs in wanting Core Services for those two types.
    unsafe {
        let _: () = msg_send![
            &*manager,
            setEventHandler: target,
            andSelector: sel!(handleGetURLEvent:withReplyEvent:),
            forEventClass: GET_URL,
            andEventID: GET_URL,
        ];
    }
    // The manager does not retain its handler, and this one answers for as long as the
    // process runs.
    std::mem::forget(handler);
}

fn deliver(text: &str) {
    if let Some((inbox, waker)) = SINK.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
        instance::open_link(inbox, waker, text);
    }
}
