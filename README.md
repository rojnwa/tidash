# tidalfast

A fast, native desktop client for TIDAL, built in Rust with egui on [fastframe](https://github.com/crmne/fastframe). It has no browser engine, and it aims for low memory and zero CPU while idle or paused.

## Legal stance

Using an official client ID breaches TIDAL's ToS and can be revoked at any time; users can enter a replacement client ID in settings. The name "tidalfast" uses TIDAL's trademark, as Spotifast uses Spotify's; rename if challenged.

## Build

```sh
cargo run -p tidalfast
```

On Linux, install the build dependencies first:

```sh
sudo apt-get install -y libasound2-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
