//! The sidebar's playlists and folders, as TIDAL keeps them (own and favorited playlists alike,
//! R7), with the dialogs that make and change them.

use super::Ctx;
use crate::i18n::tr;
use crate::model::{Action, Model, PlaylistEdit, Route};
use std::borrow::Cow;
use std::collections::HashSet;
use tidal::FolderItem;

/// A dialog, with what was typed in it so far. It lives in egui's memory, so any view can open
/// one (`open`); the sidebar draws it (`dialog`).
#[derive(Clone)]
pub enum Dialog {
    /// The new playlist starts with `tracks`.
    NewPlaylist {
        title: String,
        description: String,
        tracks: Vec<u64>,
    },
    EditPlaylist {
        uuid: String,
        title: String,
        description: String,
    },
    NewFolder(String),
    RenameFolder {
        id: String,
        name: String,
    },
    DeleteFolder {
        id: String,
        name: String,
    },
    DeletePlaylist {
        uuid: String,
        title: String,
    },
}

/// What the tree keeps between passes, for one session.
#[derive(Default)]
pub struct State {
    /// The folders asked for: each is listed once.
    asked: HashSet<String>,
}

/// The "Playlists" heading with its New buttons, then the tree, which scrolls if long. New
/// playlists and folders go to the top level.
pub fn show(ui: &mut egui::Ui, ctx: &mut Ctx, state: &mut State) {
    ui.strong(tr("Playlists"));
    ui.horizontal_wrapped(|ui| {
        if ui.button(tr("New playlist")).clicked() {
            open(ui.ctx(), new_playlist(Vec::new()));
        }
        if ui.button(tr("New folder")).clicked() {
            open(ui.ctx(), Dialog::NewFolder(String::new()));
        }
    });
    egui::ScrollArea::vertical()
        .id_salt("library")
        .show(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
            folder(ui, ctx, state, "root");
        });
    dialog(ui.ctx(), ctx);
}

/// Folder `id`'s items, once listed. The first time it shows unlisted it is asked for; the
/// backend lists the top level itself, at login.
fn folder(ui: &mut egui::Ui, ctx: &mut Ctx, state: &mut State, id: &str) {
    let Some(items) = ctx.model.folders.get(id) else {
        if id != "root" && state.asked.insert(id.to_owned()) {
            ctx.send(Action::LoadFolder(id.to_owned()));
        }
        return;
    };
    for item in items {
        match item {
            FolderItem::Folder {
                id: sub,
                name,
                count,
            } => {
                let shown = egui::CollapsingHeader::new(name)
                    .id_salt(("folder", sub))
                    .show(ui, |ui| folder(ui, ctx, state, sub));
                let listed = ctx.model.folders.get(sub);
                let empty = *count == 0 && listed.is_none_or(Vec::is_empty);
                shown.header_response.context_menu(|ui| {
                    if ui.button(tr("Rename")).clicked() {
                        let (id, name) = (sub.clone(), name.clone());
                        open(ui.ctx(), Dialog::RenameFolder { id, name });
                        ui.close();
                    }
                    // ponytail: what TIDAL does to the playlists in a deleted folder is unknown
                    // (S5), so only one known empty goes; the edit could move them out first.
                    let delete = egui::Button::new(tr("Delete"));
                    let delete = ui
                        .add_enabled(empty, delete)
                        .on_disabled_hover_text(tr("Move its playlists out first."));
                    if delete.clicked() {
                        let (id, name) = (sub.clone(), name.clone());
                        open(ui.ctx(), Dialog::DeleteFolder { id, name });
                        ui.close();
                    }
                });
            }
            FolderItem::Playlist(p) => playlist(ui, ctx, p, id),
        }
    }
}

/// Playlist `p`, in folder `parent`: it opens its page, and its menu moves it or, if it is the
/// user's own, deletes it.
fn playlist(ui: &mut egui::Ui, ctx: &mut Ctx, p: &tidal::Playlist, parent: &str) {
    let route = Route::Playlist(p.uuid.clone());
    let row = ui.selectable_label(*ctx.model.route() == route, &p.title);
    if row.clicked() {
        ctx.go(route);
    }
    row.context_menu(|ui| {
        ui.menu_button(tr("Move to folder"), |ui| {
            let here = (parent != "root").then_some(parent);
            for (to, name) in destinations(ctx.model) {
                if ui
                    .add_enabled(to != here, egui::Button::new(name))
                    .clicked()
                {
                    let (uuid, folder) = (p.uuid.clone(), to.map(str::to_owned));
                    ctx.send(Action::EditPlaylist(PlaylistEdit::MoveToFolder {
                        uuid,
                        folder,
                    }));
                    ui.close();
                }
            }
        });
        let own = p.creator_id.is_some() && p.creator_id == ctx.model.user_id;
        if own && ui.button(tr("Delete")).clicked() {
            open(ui.ctx(), delete_playlist(p));
            ui.close();
        }
    });
}

/// Where a playlist can move: the top level (`None`), then every folder listed, by name.
fn destinations(model: &Model) -> Vec<(Option<&str>, Cow<'_, str>)> {
    let mut folders: Vec<(&str, &str)> = model
        .folders
        .values()
        .flatten()
        .filter_map(|item| match item {
            FolderItem::Folder { id, name, .. } => Some((name.as_str(), id.as_str())),
            FolderItem::Playlist(_) => None,
        })
        .collect();
    folders.sort_unstable();
    folders.dedup_by_key(|(_, id)| *id);
    let folders = folders
        .into_iter()
        .map(|(name, id)| (Some(id), name.into()));
    std::iter::once((None, tr("Top level")))
        .chain(folders)
        .collect()
}

/// The "Add to playlist" menu (a submenu inside a menu): "New playlist…", then the user's own
/// playlists, newest updated first. `tracks` go into the one picked.
pub fn add_to_playlist(ui: &mut egui::Ui, ctx: &mut Ctx, tracks: &[tidal::Track]) {
    let ids = || tracks.iter().map(|t| t.id).collect();
    let label = tr("Add to playlist");
    let menu = ui.menu_button(label.as_ref(), |ui| {
        if ui.button(tr("New playlist…")).clicked() {
            open(ui.ctx(), new_playlist(ids()));
            ui.close();
        }
        for p in &ctx.model.own_playlists {
            if ui.button(&p.title).clicked() {
                let (uuid, tracks) = (p.uuid.clone(), ids());
                ctx.send(Action::EditPlaylist(PlaylistEdit::Add { uuid, tracks }));
                ui.close();
            }
        }
    });
    // Inside a menu, egui's label for a screen reader ends in the submenu's arrow.
    let resp = menu.response;
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, resp.enabled(), &label)
    });
}

/// A new playlist's dialog, empty; the playlist starts with `tracks`.
pub fn new_playlist(tracks: Vec<u64>) -> Dialog {
    let (title, description) = (String::new(), String::new());
    Dialog::NewPlaylist {
        title,
        description,
        tracks,
    }
}

/// Playlist `p`'s dialog, holding its title and description as they are.
pub fn edit_playlist(p: &tidal::Playlist) -> Dialog {
    Dialog::EditPlaylist {
        uuid: p.uuid.clone(),
        title: p.title.clone(),
        description: p.description.clone().unwrap_or_default(),
    }
}

/// The confirm before deleting playlist `p`.
pub fn delete_playlist(p: &tidal::Playlist) -> Dialog {
    let (uuid, title) = (p.uuid.clone(), p.title.clone());
    Dialog::DeletePlaylist { uuid, title }
}

fn dialog_id() -> egui::Id {
    egui::Id::new("library-dialog")
}

/// Opens `dialog`, in place of any open one.
pub fn open(egui: &egui::Context, dialog: Dialog) {
    egui.data_mut(|d| d.insert_temp(dialog_id(), Some(dialog)));
    egui.request_repaint(); // opened after the sidebar drew, it shows next pass
}

/// Closes the open dialog, unanswered.
pub fn close(egui: &egui::Context) {
    egui.data_mut(|d| d.remove::<Option<Dialog>>(dialog_id()));
}

/// The open dialog, drawn every pass. Answered, it closes; confirmed, its edit is sent.
pub fn dialog(egui: &egui::Context, ctx: &mut Ctx) {
    let taken = egui.data_mut(|d| d.remove_temp::<Option<Dialog>>(dialog_id()));
    let Some(mut open) = taken.flatten() else {
        return;
    };
    // `Some` once answered: the edit if confirmed.
    let answer = match &mut open {
        Dialog::NewPlaylist {
            title,
            description,
            tracks,
        } => {
            let ok = playlist_dialog(egui, &tr("New playlist"), title, description);
            ok.map(|ok| {
                ok.then(|| PlaylistEdit::Create {
                    title: title.trim().to_owned(),
                    description: description.trim().to_owned(),
                    folder: None,
                    tracks: std::mem::take(tracks),
                })
            })
        }
        Dialog::EditPlaylist {
            uuid,
            title,
            description,
        } => {
            let ok = playlist_dialog(egui, &tr("Edit playlist"), title, description);
            ok.map(|ok| {
                ok.then(|| PlaylistEdit::Edit {
                    uuid: uuid.clone(),
                    title: title.trim().to_owned(),
                    description: description.trim().to_owned(),
                })
            })
        }
        Dialog::NewFolder(name) => name_dialog(egui, &tr("New folder"), name).map(|ok| {
            ok.then(|| PlaylistEdit::CreateFolder {
                name: name.trim().to_owned(),
                parent: None,
            })
        }),
        Dialog::RenameFolder { id, name } => {
            let ok = name_dialog(egui, &tr("Rename folder"), name);
            ok.map(|ok| {
                ok.then(|| PlaylistEdit::RenameFolder {
                    id: id.clone(),
                    name: name.trim().to_owned(),
                })
            })
        }
        Dialog::DeleteFolder { id, name } => {
            let ask = tr("Delete the folder {name}?").replace("{name}", name);
            let id = id.clone();
            confirm(egui, &ask).map(|ok| ok.then_some(PlaylistEdit::DeleteFolder { id }))
        }
        Dialog::DeletePlaylist { uuid, title } => {
            let ask = tr("Delete the playlist {title}? This cannot be undone.");
            let ask = ask.replace("{title}", title);
            let uuid = uuid.clone();
            confirm(egui, &ask).map(|ok| ok.then_some(PlaylistEdit::Delete { uuid }))
        }
    };
    match answer {
        None => {
            egui.data_mut(|d| d.insert_temp(dialog_id(), Some(open)));
        }
        Some(Some(edit)) => ctx.send(Action::EditPlaylist(edit)),
        Some(None) => {}
    }
}

/// A dialog asking for a playlist's title and description, under `heading`. `Some(true)` once
/// submitted with a title (OK, or Enter in a field), `Some(false)` once cancelled (Cancel, Esc
/// or a click outside), else `None`: the caller keeps it open while `None`.
pub fn playlist_dialog(
    egui: &egui::Context,
    heading: &str,
    title: &mut String,
    description: &mut String,
) -> Option<bool> {
    modal(egui, heading, &tr("OK"), |ui, first| {
        let enter = field(ui, &tr("Title"), title, first)
            | field(ui, &tr("Description"), description, false);
        (enter, !title.trim().is_empty())
    })
}

/// A dialog asking for a folder's name, as `playlist_dialog`.
fn name_dialog(egui: &egui::Context, heading: &str, name: &mut String) -> Option<bool> {
    modal(egui, heading, &tr("OK"), |ui, first| {
        let enter = field(ui, &tr("Name"), name, first);
        (enter, !name.trim().is_empty())
    })
}

/// A dialog asking `question`, answered Delete or Cancel, as `playlist_dialog`.
fn confirm(egui: &egui::Context, question: &str) -> Option<bool> {
    modal(egui, question, &tr("Delete"), |_, _| (false, true))
}

/// A modal: `heading`, the fields `body` draws (told whether the dialog just opened), then `ok`
/// and Cancel. `body` says whether Enter was pressed in a field, and whether what they hold
/// may be submitted, which enables `ok`. Answers as `playlist_dialog`.
fn modal(
    egui: &egui::Context,
    heading: &str,
    ok: &str,
    body: impl FnOnce(&mut egui::Ui, bool) -> (bool, bool),
) -> Option<bool> {
    let modal = egui::Modal::new(dialog_id());
    let first = !egui.memory(|m| m.areas().visible_last_frame(&modal.area.layer()));
    let shown = modal.show(egui, |ui| {
        ui.heading(heading);
        let (enter, valid) = body(ui, first);
        ui.horizontal(|ui| {
            let ok = ui.add_enabled(valid, egui::Button::new(ok)).clicked();
            (ok || enter && valid, ui.button(tr("Cancel")).clicked())
        })
        .inner
    });
    match shown.inner {
        (true, _) => Some(true),
        (false, cancel) => (cancel || shown.should_close()).then_some(false),
    }
}

/// A labelled one-line field, focused if `focus`: true when Enter was pressed in it.
fn field(ui: &mut egui::Ui, label: &str, text: &mut String, focus: bool) -> bool {
    let label = ui.label(label);
    let edit = ui.text_edit_singleline(text).labelled_by(label.id);
    if focus {
        edit.request_focus();
    }
    edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Action, Event, Model, Route};
    use crate::ui::testing::{at, click, middle, pass_with, playlist, right, shown};
    use egui::Key;
    use tidal::FolderItem;
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    fn folder(id: &str, name: &str, count: u32) -> FolderItem {
        let (id, name) = (id.into(), name.into());
        FolderItem::Folder { id, name, count }
    }

    /// Logged in as user 1, with `root` as the tree's top level.
    fn model(root: Vec<FolderItem>) -> Model {
        let mut m = at(Route::Home);
        let id = "root".into();
        m.apply(Event::Folder { id, items: root });
        m
    }

    /// The tree drawn pass after pass, as a screen reader and a mouse use it.
    struct Tree {
        egui: egui::Context,
        tx: UnboundedSender<Action>,
        rx: UnboundedReceiver<Action>,
        state: State,
    }

    impl Tree {
        fn new() -> Self {
            let egui = egui::Context::default();
            egui.enable_accesskit();
            let (tx, rx) = unbounded_channel();
            let state = State::default();
            Self {
                egui,
                tx,
                rx,
                state,
            }
        }

        /// One pass fed `input`: the events it gave, and its output.
        fn pass(&mut self, m: &Model, input: Vec<egui::Event>) -> (Vec<Event>, egui::FullOutput) {
            let state = &mut self.state;
            pass_with(&self.egui, &self.tx, m, input, |ui, c| show(ui, c, state))
        }

        /// A few passes without input, for dialogs and menus to lay themselves out.
        fn settle(&mut self, m: &Model) -> egui::FullOutput {
            self.pass(m, vec![]);
            self.pass(m, vec![]);
            self.pass(m, vec![]).1
        }

        /// Right-clicks the node labelled `label`: the output with its menu open.
        fn menu(&mut self, m: &Model, label: &str) -> egui::FullOutput {
            let out = self.settle(m);
            let at = middle(&out, label);
            self.pass(m, vec![egui::Event::PointerMoved(at), right(at, true)]);
            self.pass(m, vec![right(at, false)]);
            self.settle(m)
        }

        /// The actions sent since last asked.
        fn sent(&mut self) -> Vec<String> {
            std::iter::from_fn(|| self.rx.try_recv().ok())
                .map(|a| format!("{a:?}"))
                .collect()
        }
    }

    fn key(key: Key) -> egui::Event {
        crate::ui::testing::key(key)
    }

    /// The top level is the backend's to load; a folder is listed once, when first opened,
    /// and its playlists open their pages.
    #[test]
    fn expanding_a_folder_loads_it_once() {
        let mut m = model(vec![folder("f", "Road", 1)]);
        let mut tree = Tree::new();
        let out = tree.settle(&m);
        assert!(tree.sent().is_empty(), "the top level is loaded already");
        tree.pass(&m, vec![click(&out, "Road")]);
        tree.settle(&m);
        assert_eq!(tree.sent(), [r#"LoadFolder("f")"#]);
        let items = vec![FolderItem::Playlist(playlist())];
        m.apply(Event::Folder {
            id: "f".into(),
            items,
        });
        let out = tree.settle(&m);
        let (events, _) = tree.pass(&m, vec![click(&out, "Playlist")]);
        assert!(
            matches!(&events[..], [Event::Navigate(Route::Playlist(p))] if p == "p-1"),
            "{events:?}"
        );
        for _ in 0..2 {
            let out = tree.settle(&m); // closed, then open again
            tree.pass(&m, vec![click(&out, "Road")]);
        }
        tree.settle(&m);
        assert!(tree.sent().is_empty(), "listed already");
    }

    /// "New playlist" asks for a title (and a description): Esc closes the dialog, Enter
    /// creates the playlist at the top level.
    #[test]
    fn new_playlist_dialog_sends_create() {
        let m = model(vec![]);
        let mut tree = Tree::new();
        for submit in [key(Key::Escape), key(Key::Enter)] {
            let out = tree.settle(&m);
            tree.pass(&m, vec![click(&out, "New playlist")]);
            let out = tree.settle(&m);
            assert!(disabled(&out, "OK"), "no title yet");
            tree.pass(&m, vec![egui::Event::Text("Road trip".into())]);
            tree.pass(&m, vec![submit]);
            let out = tree.settle(&m);
            assert!(!shown(&out).contains(&"OK".into()), "closed");
        }
        let create = concat!(
            r#"EditPlaylist(Create { title: "Road trip", description: "", "#,
            "folder: None, tracks: [] })"
        );
        assert_eq!(tree.sent(), [create]);
    }

    /// Whether the node labelled `label` in `out` is shown disabled.
    fn disabled(out: &egui::FullOutput, label: &str) -> bool {
        let tree = out.platform_output.accesskit_update.as_ref();
        let nodes = &tree.expect("AccessKit is on").nodes;
        let node = nodes.iter().find(|(_, n)| n.label() == Some(label));
        node.unwrap_or_else(|| panic!("no {label:?} shown"))
            .1
            .is_disabled()
    }

    /// A folder's Delete asks first. Only a folder known to be empty offers it, since what
    /// TIDAL does to the playlists in one is unknown (S5): not one counting items, nor one
    /// whose listing holds some though its count says none.
    #[test]
    fn delete_folder_asks_first() {
        let mut m = model(vec![
            folder("f", "Road", 0),
            folder("g", "Full", 2),
            folder("s", "Stale", 0),
        ]);
        let items = vec![FolderItem::Playlist(playlist())];
        m.apply(Event::Folder {
            id: "s".into(),
            items,
        });
        let mut tree = Tree::new();
        for full in ["Full", "Stale"] {
            let out = tree.menu(&m, full);
            assert!(disabled(&out, "Delete"), "{full}");
            tree.pass(&m, vec![key(Key::Escape)]); // the menu closes
        }
        let out = tree.menu(&m, "Road");
        assert!(!disabled(&out, "Delete"));
        tree.pass(&m, vec![click(&out, "Delete")]);
        let out = tree.settle(&m);
        assert!(tree.sent().is_empty(), "asked first");
        assert!(shown(&out).contains(&"Delete the folder Road?".into()));
        tree.pass(&m, vec![click(&out, "Delete")]);
        assert_eq!(tree.sent(), [r#"EditPlaylist(DeleteFolder { id: "f" })"#]);
    }
}
