# Releasing

A tag `vX.Y.Z` (or `vX.Y.Z-rc.N`) that equals `[workspace.package] version` in `Cargo.toml` runs
`.github/workflows/release.yml`. It builds every target, signs `checksums.txt` and publishes the
GitHub release; then `packaging.yml` builds the deb, rpm and AppImage from the published Linux
tarballs and attaches them. A tag with a `-` is a pre-release.

## One-time setup

The `release-signing` environment, with `rojnwa` as required reviewer and deployments from `v*`
tags only. The publish job waits there for approval and is the only job that can read the key.

```sh
id=$(gh api users/rojnwa --jq .id)
gh api -X PUT repos/rojnwa/tidash/environments/release-signing --input - <<EOF
{"reviewers":[{"type":"User","id":$id}],"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}
EOF
gh api -X POST repos/rojnwa/tidash/environments/release-signing/deployment-branch-policies -f name='v*' -f type=tag
```

The secret `TIDASH_UPDATE_SIGNING_KEY` in that environment: the whole PEM file.

```sh
gh secret set TIDASH_UPDATE_SIGNING_KEY --env release-signing < ~/.tidash-release/update-signing-key.pem
```

Store the PEM in a password manager as the offline backup; only then delete the local file with
`rm -P ~/.tidash-release/update-signing-key.pem`. GitHub never shows a secret again, so a key
without a backup is lost, and every install then needs one manual download.

Optional: the Apple secrets, as repository secrets. With all six the DMG is signed with a Developer
ID and notarized; with none it is ad-hoc signed; a partial set fails the build.

```sh
base64 -i developer-id.p12 | gh secret set APPLE_CERTIFICATE_P12  # Developer ID Application certificate and key
gh secret set APPLE_CERTIFICATE_PASSWORD  # the .p12's password
gh secret set APPLE_SIGNING_IDENTITY      # Developer ID Application: Name (TEAMID)
gh secret set APPLE_ID                    # the Apple ID's email
gh secret set APPLE_TEAM_ID
gh secret set APPLE_APP_PASSWORD          # an app-specific password for that Apple ID
```

### Rotating the signing key

Keep signing with the current key while installs learn the next one:

1. Generate the next key and back it up outside GitHub:
   ```sh
   openssl genpkey -algorithm ed25519 -out ~/.tidash-release/next-signing-key.pem
   openssl pkey -in ~/.tidash-release/next-signing-key.pem -pubout -outform DER | tail -c 32 | xxd -p -c 32
   ```
   Add that hex public key to `additional_publisher_keys` in `crates/app/src/update.rs`. Releases
   stay signed with the current key.
2. Once most installs run a version that trusts both, write the next public key to
   `crates/app/assets/update-public-key.hex` (the `publisher_key`) and store the next PEM as
   `TIDASH_UPDATE_SIGNING_KEY`.

An install that skips every release in between refuses the newly signed one and needs a manual
download once.

## Each release

1. Bump `[workspace.package] version` in `Cargo.toml`, run `cargo check` (it updates `Cargo.lock`),
   and write `packaging/release-notes/vX.Y.Z.md`. The release workflow fails without it.
2. Run the budget on each OS at hand, logged in, and paste the numbers into the notes:
   `bash packaging/budget.sh target/release/tidash` (macOS, Linux) or
   `pwsh packaging/budget.ps1 target\release\tidash.exe` (Windows), after
   `cargo build --release -p tidash`. Still by hand: CPU paused with the window hidden (0 %), CPU
   playing with the window minimized (< 1 %), and scroll pacing on Windows (PresentMon).
3. Commit and merge to `main`, then tag that commit and push the tag:
   ```sh
   git fetch origin
   git tag vX.Y.Z origin/main
   git push origin vX.Y.Z
   ```
4. Approve `release-signing`: Actions › Release › the tag's run › Review deployments.
5. Check the assets with `gh release view vX.Y.Z --json assets --jq '.assets[].name'`:
   - the 7 updater assets: `tidash-vX.Y.Z-{x86_64,aarch64}-unknown-linux-gnu.tar.gz`,
     `tidash-vX.Y.Z-{x86_64,aarch64}-pc-windows-msvc.zip`,
     `tidash-vX.Y.Z-{x86_64,aarch64}-pc-windows-msvc-setup.exe` and
     `tidash-vX.Y.Z-macos-universal.dmg`;
   - `tidash-vX.Y.Z-source.tar.gz`, `checksums.txt` and `checksums.txt.sig`;
   - a few minutes later, the deb, rpm and AppImage for amd64 and arm64, with
     `packaging-checksums.txt`. If that job failed, rebuild them once it is fixed:
     `gh workflow run packaging.yml -f version=vX.Y.Z -f publish=true`.

## A dry run

A pull request (a draft is fine) that changes `.github/workflows/release.yml`,
`.github/workflows/packaging.yml`, `packaging/**`, `native-packages.yaml`, `crates/app/build.rs` or
`.cargo/config.toml` runs the release workflow as a dry run. Every target is built, checked and
packaged, and uploaded as a workflow artifact. Nothing is published and the checksums are not
signed; the DMG is ad-hoc signed, or notarized once the `APPLE_*` secrets exist (a pull request from a
fork never gets them).

Any other branch: Actions › Release › Run workflow, or `gh workflow run release.yml --ref <branch>`.
This only works once `release.yml` is on `main`.

## Unsigned builds, for users

- macOS: the first open is refused. Open it once anyway in System Settings › Privacy & Security ›
  Open Anyway.
- Windows: SmartScreen warns. More info › Run anyway.
