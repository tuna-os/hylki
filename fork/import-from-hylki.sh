#!/usr/bin/env bash
#
# Copy upstream Hylki's accounts, settings, mail cache and keyring entries into
# Hylki Tuna, once, before Hylki Tuna's first start.
#
#   fork/import-from-hylki.sh
#
# Hylki Tuna keeps its own state (org.tunaos.Hylki, hylki-tuna directories, its
# own keyring service) so the two apps can run side by side, and so it does not
# carry upstream's data over by itself: the app's own carry-over (src/desktop/
# legacy.rs) is built for a predecessor that is being replaced, and renames its
# directories in place. This copies instead, and leaves Hylki untouched.
#
# The source is upstream's Flatpak (~/.var/app/co.hyprlab.Hylki) when it has
# data, else a native install (~/.config/hylki and its siblings). The target is
# Hylki Tuna's Flatpak. Quit both apps first. Nothing is overwritten: a target
# directory that already exists is left alone, as is a keyring entry.
set -euo pipefail

old_id=co.hyprlab.Hylki
new_id=org.tunaos.Hylki
target="$HOME/.var/app/$new_id"

if [ -d "$HOME/.var/app/$old_id/config/hylki" ]; then
    source_dir() { echo "$HOME/.var/app/$old_id/$1/hylki"; }
else
    source_dir() {
        case "$1" in
            config) echo "${XDG_CONFIG_HOME:-$HOME/.config}/hylki" ;;
            data) echo "${XDG_DATA_HOME:-$HOME/.local/share}/hylki" ;;
            cache) echo "${XDG_CACHE_HOME:-$HOME/.cache}/hylki" ;;
        esac
    }
fi
if [ ! -d "$(source_dir config)" ]; then
    echo "no Hylki settings found at $(source_dir config)" >&2
    exit 1
fi

for base in config data cache; do
    from=$(source_dir "$base")
    to="$target/$base/hylki-tuna"
    if [ ! -d "$from" ]; then
        continue
    fi
    if [ -e "$to" ]; then
        echo "kept $to (already there)"
        continue
    fi
    mkdir -p "$(dirname "$to")"
    cp -a --reflink=auto "$from" "$to"
    echo "copied $from to $to"
done

# Keyring entries: passwords, OAuth refresh tokens and Tuta or Proton
# sessions, keyed by the same names under the new service. The attributes are
# the ones the keyring crate writes and searches by.
if ! command -v secret-tool >/dev/null; then
    echo "secret-tool not found (libsecret-tools); passwords will be asked for again" >&2
    exit 0
fi
copied=0
while IFS= read -r key; do
    [ -n "$key" ] || continue
    if secret-tool lookup service "$new_id" username "$key" >/dev/null 2>&1; then
        continue
    fi
    secret-tool lookup service "$old_id" username "$key" \
        | secret-tool store --label="$key@$new_id:default" \
            service "$new_id" username "$key" target default application rust-keyring
    copied=$((copied + 1))
done < <(secret-tool search --all --unlock service "$old_id" 2>&1 \
    | sed -n 's/^attribute\.username = //p' | sort -u)
echo "copied $copied keyring entr$([ "$copied" = 1 ] && echo y || echo ies) from $old_id to $new_id"
