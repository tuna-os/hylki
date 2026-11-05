# Hylki Tuna

Hylki Tuna is [Hylki](https://github.com/hyprlab/hylki) with native
[Tuta](https://tuta.com) and [Proton Mail](https://proton.me/mail) accounts,
maintained by TunaOS at [tuna-os/hylki](https://github.com/tuna-os/hylki).
Upstream declined both providers, so this soft fork carries them and follows
upstream for everything else. It ships as its own app, `org.tunaos.Hylki`,
which installs beside upstream Hylki and shares none of its state.

Neither Tuta nor Proton endorses third-party clients built this way; use them
knowing that. What belongs in the fork and what goes upstream, and how
touchpoints are kept small, is in [MAINTAINING.md](MAINTAINING.md).

## What is in this directory

| File | What it does |
| --- | --- |
| `rebrand.sh` | Turns a copy of the tree into Hylki Tuna before it is built |
| `import-from-hylki.sh` | Copies upstream Hylki's accounts, settings and passwords into Hylki Tuna |
| `MAINTAINING.md` | The fork's boundary and upstreaming policy |
| `touchpoints.txt`, `check-touchpoints.sh` | The upstream files the fork modifies, and the check that keeps that list honest |

The workflows are `.github/workflows/upstream-sync.yml` and
`.github/workflows/fork-release.yml`.

## How the rebrand works

The source keeps upstream's `co.hyprlab.Hylki` strings. Renaming them in the
source would touch dozens of upstream lines, and each would conflict on a
later merge. Instead `fork/rebrand.sh <tree>` rewrites a copy of the tree just
before it is built:

- **App ID** `co.hyprlab.Hylki` becomes `org.tunaos.Hylki`: the Rust
  constant in `main.rs`, the desktop file, metainfo, D-Bus restart service,
  icons (file names and contents) and the GNOME Files extension. Every file
  named after the ID is renamed.
- **Keyring** entries are stored under the service `org.tunaos.Hylki`, so
  they never collide with upstream's.
- **Display name** "Hylki Tuna": launcher, metainfo name and summary, the
  window and About titles, a line under the About window's wordmark (an
  image that says "Hylki"), and the tray. The po catalogues translate "Hylki"
  and the old summary, so these strings show untranslated, which is right for
  a name.
- **Directories** under the XDG config, data, cache and runtime bases are
  `hylki-tuna` instead of `hylki`, so a native build does not share state with
  upstream either. The Files extension gets its own file name.
- **Links**: the About window, the welcome page and the metainfo homepage,
  bug tracker, developer and update contact point at tuna-os/hylki.
- **Manifest**: `fork/org.tunaos.Hylki.yml` is written from upstream's
  `co.hyprlab.Hylki.yml` through `tools/local-manifest.sh`. Upstream's pins
  a tagged commit of hyprlab/hylki, which would build without the rebrand, so
  the fork's builds the rebranded tree it sits in. Every module, including the
  Tuta (Rust) and Proton (Go) helpers, is upstream's recipe unchanged.

Left alone on purpose: the bundled resource prefix `/co/hyprlab/Hylki`, which
is private to the process (`main.rs` pins it), the `hylki` binary and gettext
domain, which live inside the Flatpak, and the Vireo and Veem carry-over,
which is upstream's.

Every rewrite names the text it expects and stops with the file and pattern
when upstream changes it. A final sweep fails on any `co.hyprlab.Hylki` or
`.join("hylki…")` left in `src/`, `data/` or `resources/`. So drift in either
direction fails the daily sync instead of shipping. The script refuses to run
on its own checkout unless `HYLKI_REBRAND_IN_PLACE=1` is set (CI sets it on
its throwaway checkout). Running it again on a rebranded tree only repeats the
sweep.

## How upstream is followed

`.github/workflows/upstream-sync.yml` runs daily and on demand:

1. It fetches hyprlab/hylki `main` and merges it into our `main` with a merge
   commit. It never rebases and never force-pushes.
2. It runs `fork/check-touchpoints.sh`.
3. It rebrands a copy and runs `cargo check` on it, in a Fedora container
   with the app's build dependencies.
4. If all of that passed, it pushes.

If any step fails, nothing is pushed. One issue labelled `upstream-sync` is
opened, or updated if one is already open, with the step that failed and any
conflicting files. The next clean run closes it.

`GITHUB_TOKEN` may not push a commit that changes `.github/workflows`. When
upstream changes its workflows, the push fails and the issue says so. Merge
that one by hand, or add an `UPSTREAM_SYNC_TOKEN` secret holding a token with
the `workflow` scope; the job pushes with it when it is set.

## Building

Releases are tagged `v<upstream-version>-tuna<N>`, for example `v1.47.0-tuna1`.
A tag, or a manual run of `.github/workflows/fork-release.yml` given such a
tag, builds `org.tunaos.Hylki-<version>.flatpak` (x86_64) and
`org.tunaos.Hylki-<version>-aarch64.flatpak` and attaches both to the GitHub
release. A manual run on a branch builds the same bundles as workflow
artifacts only. Upstream's `build-arm64.yml` also runs for these tags, because
it matches every `v*` tag; its upstream-branded artifact can be ignored.

A local build follows the same steps as `fork-release.yml`. It needs
`flatpak-builder`, the GNOME 50 SDK, and the `rust-stable` and `golang`
extensions for 25.08 (see [docs/BUILDING.md](../docs/BUILDING.md)):

```sh
git submodule update --init --recursive
sh native/tuta-unofficial-sdk/prepare.sh
flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json

rsync -a --delete --exclude=.git --exclude=/target \
    --exclude=/native/tuta-unofficial-sdk/target \
    --exclude=/native/tuta-unofficial-sdk/upstream ./ /tmp/hylki-tuna/
fork/rebrand.sh /tmp/hylki-tuna
flatpak-builder --user --install --force-clean \
    --state-dir=/tmp/hylki-tuna-state /tmp/hylki-tuna-build \
    /tmp/hylki-tuna/fork/org.tunaos.Hylki.yml
flatpak run org.tunaos.Hylki
```

The bundles are unsigned single files. Publishing a signed OSTree repository
on GitHub Pages, so that `flatpak update` follows releases, is still to do.

## Moving over from upstream Hylki

Hylki Tuna starts empty. Hylki's own carry-over from its earlier names
(`src/desktop/legacy.rs`) does not fit here. It is built for a predecessor
that is being replaced: it renames that app's directories in place and takes
over its Files extension. Upstream Hylki keeps running beside the fork, so
its data has to be copied instead.

Quit both apps, then run:

```sh
fork/import-from-hylki.sh
```

It copies the config, data and cache directories from upstream's Flatpak
(`~/.var/app/co.hyprlab.Hylki`), or from a native install (`~/.config/hylki`
and its siblings), into `~/.var/app/org.tunaos.Hylki/*/hylki-tuna`. It then
copies every keyring entry from the `co.hyprlab.Hylki` service to
`org.tunaos.Hylki` with `secret-tool`. Nothing that already exists is
overwritten, and upstream's data is left as it was. Without `secret-tool`,
passwords and sign-ins are asked for again on first use.
