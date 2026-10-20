//! The user's playlists and folders: their edits, and the listings of the library tree.

use crate::backend::{Emitter, session_expired};
use crate::i18n::{tr, tr_n};
use crate::messages::{Event, PlaylistEdit};
use crate::playback::Msg;
use std::sync::Arc;
use tidal::Client;
use tokio::sync::mpsc::UnboundedSender;

/// Makes `edit`, then reloads the tree: the top level, `folders` (those listed this session)
/// and the own playlists. A failed edit reloads too: a nested create may have made the item at
/// the top before its move failed. A stale ETag (412) reloads the playlist and says so; another
/// failure sends one error. The backend spawns it.
pub async fn edit(
    c: Arc<Client>,
    edit: PlaylistEdit,
    folders: Vec<String>,
    emit: Arc<Emitter>,
    msgs: UnboundedSender<Msg>,
) {
    match write(&c, &edit).await {
        Ok(events) => events.into_iter().for_each(|ev| emit.send(ev)),
        Err(e) if session_expired(&e, &c, &msgs) => return,
        Err(tidal::Error::Status { status: 412, .. }) => {
            if let PlaylistEdit::Add { uuid, .. }
            | PlaylistEdit::Remove { uuid, .. }
            | PlaylistEdit::Move { uuid, .. } = edit
            {
                emit.send(Event::PlaylistChanged(uuid));
            }
            let why = tr("This playlist was changed elsewhere and has been reloaded. Try again.");
            emit.send(Event::Error(why.into_owned()));
        }
        Err(e) => emit.send(Event::Error(format!(
            "{}: {e}",
            tr("Could not update your playlists")
        ))),
    }
    reload(c, folders, emit, msgs).await;
}

/// `edit`'s requests, and the events of its success.
async fn write(c: &Client, edit: &PlaylistEdit) -> tidal::Result<Vec<Event>> {
    let changed = |uuid: &String| vec![Event::PlaylistChanged(uuid.clone())];
    Ok(match edit {
        PlaylistEdit::Create {
            title,
            description,
            folder,
            tracks,
        } => {
            let uuid = c
                .create_playlist(title, description, folder.as_deref())
                .await?;
            if !tracks.is_empty() {
                c.add_to_playlist(&uuid, tracks).await?;
            }
            Vec::new()
        }
        PlaylistEdit::Edit {
            uuid,
            title,
            description,
        } => {
            c.edit_playlist(uuid, title, description).await?;
            changed(uuid)
        }
        PlaylistEdit::Delete { uuid } => {
            c.delete_playlist(uuid).await?;
            vec![Event::PlaylistDeleted(uuid.clone())]
        }
        PlaylistEdit::Add { uuid, tracks } => {
            let title = c.playlist(uuid).await?.title;
            let said = match c.add_to_playlist(uuid, tracks).await? {
                0 => tr("Already in this playlist").into_owned(),
                n => tr_n(
                    "Added {n} track to {playlist}",
                    "Added {n} tracks to {playlist}",
                    n as u32,
                )
                .replace("{playlist}", &title),
            };
            let mut events = changed(uuid);
            events.push(Event::Toast(said));
            events
        }
        PlaylistEdit::Remove {
            uuid,
            etag,
            positions,
        } => {
            c.remove_from_playlist(uuid, etag, positions).await?;
            changed(uuid)
        }
        PlaylistEdit::Move {
            uuid,
            etag,
            from,
            to,
        } => {
            c.move_in_playlist(uuid, etag, *from, *to).await?;
            changed(uuid)
        }
        PlaylistEdit::CreateFolder { name, parent } => {
            c.create_folder(name, parent.as_deref()).await?;
            Vec::new()
        }
        PlaylistEdit::RenameFolder { id, name } => {
            c.rename_folder(id, name).await?;
            Vec::new()
        }
        PlaylistEdit::DeleteFolder { id } => {
            c.delete_folder(id).await?;
            Vec::new()
        }
        PlaylistEdit::MoveToFolder { uuid, folder } => {
            let uuids = std::slice::from_ref(uuid);
            c.move_to_folder(uuids, folder.as_deref()).await?;
            Vec::new()
        }
    })
}

/// The tree's top level, then `folders`, then the own playlists (`Event::OwnPlaylists`).
pub async fn reload(
    c: Arc<Client>,
    folders: Vec<String>,
    emit: Arc<Emitter>,
    msgs: UnboundedSender<Msg>,
) {
    for id in std::iter::once("root".to_owned()).chain(folders) {
        load_folder(c.clone(), id, emit.clone(), msgs.clone()).await;
    }
    let own = match c.own_playlists().await {
        Err(tidal::Error::Http(_)) => c.cache_only().own_playlists().await,
        own => own,
    };
    match own {
        Ok(own) => emit.send(Event::OwnPlaylists(own)),
        Err(e) if session_expired(&e, &c, &msgs) => {}
        Err(e) => log::warn!("own playlists: {e}"),
    }
}

/// Folder `id` (`"root"`: the top level) for the tree (`Event::Folder`); offline, its saved
/// copy. A failure is logged: the tree keeps what it shows.
pub async fn load_folder(
    c: Arc<Client>,
    id: String,
    emit: Arc<Emitter>,
    msgs: UnboundedSender<Msg>,
) {
    let items = match c.folder(&id).await {
        Err(tidal::Error::Http(_)) => c.cache_only().folder(&id).await,
        items => items,
    };
    match items {
        Ok(items) => emit.send(Event::Folder { id, items }),
        Err(e) if session_expired(&e, &c, &msgs) => {}
        Err(e) => log::warn!("folder {id}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Route;
    use crate::pages::tests::{cache, client, events_of};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ROOT: &str = include_str!("../../tidal/tests/fixtures/folder_root.json");
    const FOLDER: &str = include_str!("../../tidal/tests/fixtures/folder_items.json");
    const OWN: &str = include_str!("../../tidal/tests/fixtures/own_playlists.json");
    const FOLDERS: &str = "/v2/my-collection/playlists/folders";
    const EMPTY: &str = r#"{"items": [], "totalNumberOfItems": 0}"#;

    fn json(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_raw(body, "application/json")
    }

    /// Serves the listing of folder `id` and the own playlists.
    async fn serve_library(server: &MockServer, id: &str, folder: &str, own: &str) {
        Mock::given(method("GET"))
            .and(path(FOLDERS))
            .and(query_param("folderId", id))
            .respond_with(json(folder))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/users/1/playlists"))
            .respond_with(json(own))
            .mount(server)
            .await;
    }

    /// The events of `edit` on `server`, with `folders` listed this session.
    async fn events(
        server: &MockServer,
        name: &str,
        e: PlaylistEdit,
        folders: &[&str],
    ) -> Vec<Event> {
        let c = client(&server.uri(), &cache(name));
        let folders = folders.iter().map(|f| f.to_string()).collect();
        let (events, expired) = events_of(|emit, msgs| edit(c, e, folders, emit, msgs)).await;
        assert!(!expired);
        events
    }

    fn errors(events: &[Event]) -> Vec<&str> {
        fn error(e: &Event) -> Option<&str> {
            match e {
                Event::Error(e) => Some(e),
                _ => None,
            }
        }
        events.iter().filter_map(error).collect()
    }

    /// The write, then the top level, the folders listed this session (a move's source or a
    /// renamed folder's parent may be any of them) and the own playlists.
    #[tokio::test]
    async fn edit_success_reloads_the_library() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(format!("{FOLDERS}/rename")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        serve_library(&server, "root", ROOT, OWN).await;
        let sub = "f0000000-0000-4000-8000-000000000001";
        Mock::given(method("GET"))
            .and(path(FOLDERS))
            .and(query_param("folderId", sub))
            .respond_with(json(FOLDER))
            .mount(&server)
            .await;
        let rename = PlaylistEdit::RenameFolder {
            id: "f-new".into(),
            name: "Road".into(),
        };
        let events = events(&server, "edit-ok", rename, &[sub]).await;
        let got: Vec<String> = events
            .iter()
            .map(|e| match e {
                Event::Folder { id, items } => format!("{id}: {}", items.len()),
                Event::OwnPlaylists(own) => format!("own: {}", own.len()),
                e => format!("{e:?}"),
            })
            .collect();
        assert_eq!(got, ["root: 3", format!("{sub}: 2").as_str(), "own: 3"]);
        let requests = server.received_requests().await.expect("recorded");
        let order: Vec<String> = requests
            .iter()
            .map(|r| {
                let at = r.url.query_pairs().find(|(k, _)| k == "folderId");
                let at = at.map_or(String::new(), |(_, v)| format!(" {v}"));
                format!("{} {}{at}", r.method, r.url.path())
            })
            .collect();
        let folders = |id| format!("GET {FOLDERS} {id}");
        assert_eq!(
            order,
            [
                format!("PUT {FOLDERS}/rename"),
                folders("root"),
                folders(sub),
                "GET /v1/users/1/playlists".into()
            ]
        );
    }

    /// One error; the library is reloaded all the same: a nested create may have made the
    /// item at the top before its move failed.
    #[tokio::test]
    async fn a_failed_edit_reports_once_and_still_reloads() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path(format!("{FOLDERS}/move")))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        serve_library(&server, "root", EMPTY, EMPTY).await;
        let mv = PlaylistEdit::MoveToFolder {
            uuid: "p".into(),
            folder: Some("f".into()),
        };
        let events = events(&server, "edit-fail", mv, &[]).await;
        assert!(
            matches!(
                &events[..],
                [
                    Event::Error(_),
                    Event::Folder { .. },
                    Event::OwnPlaylists(_)
                ]
            ),
            "{events:?}"
        );
    }

    /// A remove behind a stale ETag is refused (412): the page reloads and the user is told.
    #[tokio::test]
    async fn a_412_reloads_the_playlist() {
        let server = MockServer::start().await;
        let stale = ResponseTemplate::new(412).set_body_raw(
            r#"{"status":412,"subStatus":7002,"userMessage":"You must send the correct Etag value"}"#,
            "application/json",
        );
        Mock::given(method("DELETE"))
            .and(path("/v1/playlists/p/items/1"))
            .respond_with(stale)
            .expect(1)
            .mount(&server)
            .await;
        serve_library(&server, "root", EMPTY, EMPTY).await;
        let remove = PlaylistEdit::Remove {
            uuid: "p".into(),
            etag: "\"L0\"".into(),
            positions: vec![1],
        };
        let events = events(&server, "edit-412", remove, &[]).await;
        let route = Route::Playlist("p".into());
        let mut m = crate::ui::testing::at(route.clone());
        m.apply(Event::Page {
            route: route.clone(),
            data: crate::model::PageData::Tracks(vec![]),
        });
        let now = std::time::Instant::now();
        assert_eq!(m.open_current(now), None, "loaded");
        let said = errors(&events).len();
        for ev in events {
            m.apply(ev);
        }
        assert_eq!(m.open_current(now), Some(route), "asked for again");
        let shown: Vec<&str> = m.errors.iter().map(|(_, e)| e.as_str()).collect();
        let changed = "This playlist was changed elsewhere and has been reloaded. Try again.";
        assert_eq!((shown, said), (vec![changed], 1));
    }

    /// How many were added, by the playlist's name; or that they were all in already. A toast
    /// says it, not an error notice.
    #[tokio::test]
    async fn add_reports_added_or_already_there() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/playlists/p"))
            .respond_with(
                json(r#"{"uuid": "p", "title": "Road trip"}"#).insert_header("etag", "\"1\""),
            )
            .mount(&server)
            .await;
        for added in ["[7]", "[7, 8]", "[]"] {
            Mock::given(method("POST"))
                .and(path("/v1/playlists/p/items"))
                .respond_with(json(&format!(
                    r#"{{"lastUpdated": 1, "addedItemIds": {added}}}"#
                )))
                .up_to_n_times(1)
                .mount(&server)
                .await;
        }
        serve_library(&server, "root", EMPTY, EMPTY).await;
        let mut said = Vec::new();
        for (i, tracks) in [vec![7], vec![7, 8], vec![7, 8]].into_iter().enumerate() {
            let add = PlaylistEdit::Add {
                uuid: "p".into(),
                tracks,
            };
            let events = events(&server, &format!("edit-add-{i}"), add, &[]).await;
            assert!(errors(&events).is_empty(), "{events:?}");
            said.extend(events.into_iter().filter_map(|e| match e {
                Event::Toast(t) => Some(t),
                _ => None,
            }));
        }
        assert_eq!(
            said,
            [
                "Added 1 track to Road trip",
                "Added 2 tracks to Road trip",
                "Already in this playlist"
            ]
        );
    }
}
