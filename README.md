# Tidash

A fast, native desktop client for TIDAL, built in Rust with egui on [fastframe](https://github.com/crmne/fastframe). It has no browser engine, and it aims for low memory and zero CPU while idle or paused.

## Legal stance

Using an official client ID breaches TIDAL's ToS and can be revoked at any time. One is built in; you can enter a replacement client ID and redirect URL in Settings › Account, which the login page also leads to. The name "Tidash" (TIDAL plus dash, for fast) plays on TIDAL's trademark, as Spotifast does on Spotify's; rename if challenged.

## Build

```sh
cargo run -p tidash
```

On Linux, install the build dependencies first:

```sh
sudo apt-get install -y libasound2-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev
```

### Build-time keys

Last.fm scrobbling and the Discord status need keys compiled in. Set these environment variables when building; without them, those integrations are hidden in Settings. ListenBrainz needs none.

- `TIDASH_LASTFM_API_KEY` and `TIDASH_LASTFM_SECRET`: a Last.fm API account ([last.fm/api/account/create](https://www.last.fm/api/account/create)).
- `TIDASH_DISCORD_APP_ID`: the application ID of a Discord application named `Tidash` (its name is what "Listening to …" shows).

## Themes

Settings › Appearance offers Follow system, Dark (TIDAL's look, the default), Light and every palette in the `themes` folder beside `settings.json` in the config directory (Open themes folder opens it). The eight presets from fastframe (Nord, Catppuccin, Rose Pine, Tokyo Night, …) are installed there on the first run, and a palette file you drop in (a preset copied and edited is the easiest start) is listed the next time Settings opens.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
