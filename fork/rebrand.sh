#!/usr/bin/env bash
#
# Turn a checkout of Hylki into Hylki Tuna (app ID org.tunaos.Hylki), in place.
#
#   cp -a . /tmp/hylki-tuna && fork/rebrand.sh /tmp/hylki-tuna
#
# The fork's source keeps upstream's co.hyprlab.Hylki strings so that merging
# hyprlab/hylki stays cheap; the rebrand is applied to a copy of the tree just
# before it is built (CI runs it on its own throwaway checkout). It rewrites:
#
#   - the app ID in the Rust constants, the keyring service name, the desktop
#     file, metainfo, D-Bus service, icons and the Files extension, renaming
#     every file named after the ID;
#   - the display name to "Hylki Tuna" (launcher, metainfo, window and About
#     titles, tray);
#   - the config, data, cache and runtime directory names from hylki to
#     hylki-tuna, so a native build never shares state with upstream Hylki;
#   - the project links and metainfo contacts to tuna-os/hylki;
#   - the Flatpak manifest, written as fork/org.tunaos.Hylki.yml, which builds
#     the rebranded tree it sits in.
#
# Every rewrite names the text it expects. When upstream changes one of those
# sites the script stops with the file and the pattern, and a final sweep
# fails on any upstream ID or "hylki" directory join left behind, so drift in
# either direction is caught in CI rather than shipped.
#
# Running it again on a rebranded tree only re-runs that final sweep.
set -euo pipefail

if [ $# -ne 1 ]; then
    echo "usage: $0 <tree>   (rewrites <tree> in place; run it on a copy)" >&2
    exit 2
fi
TREE="$(cd "$1" && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
# The script's own checkout is refused unless asked for: CI's checkout is
# disposable and says so, a developer's working tree is not.
if [ "$TREE" = "$(cd "$HERE/.." && pwd)" ] && [ -z "${HYLKI_REBRAND_IN_PLACE:-}" ]; then
    echo "refusing to rebrand this script's own checkout $TREE; copy it first" >&2
    echo "(or set HYLKI_REBRAND_IN_PLACE=1 on a throwaway checkout)" >&2
    exit 2
fi

exec python3 - "$TREE" <<'PY'
import os
import re
import subprocess
import sys
from pathlib import Path

tree = Path(sys.argv[1])
os.chdir(tree)

OLD_ID = "co.hyprlab.Hylki"
NEW_ID = "org.tunaos.Hylki"
NAME = "Hylki Tuna"
DIR = "hylki-tuna"
REPO = "https://github.com/tuna-os/hylki"
STAMP = Path("fork/.rebranded")
MANIFEST = Path("fork/org.tunaos.Hylki.yml")


def fail(message):
    sys.exit(f"rebrand: {message}\n(upstream changed this site; update fork/rebrand.sh)")


def sub(path, pattern, replacement, count=None, flags=re.M):
    """Replace `pattern` in `path`; it must match, `count` times when given."""
    path = Path(path)
    if not path.is_file():
        fail(f"{path} is missing")
    text = path.read_text()
    new, n = re.subn(pattern, replacement, text, flags=flags)
    if n == 0 or (count is not None and n != count):
        want = "at least once" if count is None else f"{count} time(s)"
        fail(f"{path}: expected {pattern!r} {want}, found {n}")
    path.write_text(new)


def rename(old, new):
    old, new = Path(old), Path(new)
    if not old.exists():
        fail(f"{old} is missing")
    old.rename(new)


def text_files(*roots):
    for root in roots:
        for path in sorted(Path(root).rglob("*")):
            if not path.is_file():
                continue
            try:
                yield path, path.read_text()
            except UnicodeDecodeError:
                continue


# Where a leftover would change what runs or what gets installed.
SWEPT = ("src", "data", "resources")


def sweep_check():
    leftovers = []
    for path, text in text_files(*SWEPT):
        if OLD_ID in text:
            leftovers.append(f"{path}: {OLD_ID}")
        if path.suffix == ".rs" and re.search(r'\.join\("hylki[/"]', text):
            leftovers.append(f'{path}: .join("hylki…")')
    for name in ("build.rs", MANIFEST):
        if OLD_ID in Path(name).read_text():
            leftovers.append(f"{name}: {OLD_ID}")
    for root in SWEPT:
        for path in Path(root).rglob("*" + OLD_ID + "*"):
            leftovers.append(f"{path}: file name")
    if leftovers:
        fail("upstream names left after the rebrand:\n  " + "\n  ".join(leftovers))


if STAMP.exists():
    sweep_check()
    print(f"rebrand: {tree} is already Hylki Tuna")
    sys.exit(0)

# --- Files named after the app ID -----------------------------------------
expected_files = [
    "data/co.hyprlab.Hylki.desktop",
    "data/co.hyprlab.Hylki.metainfo.xml",
    "data/co.hyprlab.Hylki.Restart.service",
    "data/icons/hicolor/256x256/apps/co.hyprlab.Hylki.png",
    "data/icons/hicolor/512x512/apps/co.hyprlab.Hylki.png",
    "data/icons/hicolor/scalable/apps/co.hyprlab.Hylki.svg",
    "data/icons/hicolor/symbolic/apps/co.hyprlab.Hylki-symbolic.svg",
    "resources/icons/scalable/apps/co.hyprlab.Hylki.svg",
]
for name in expected_files:
    if not Path(name).is_file():
        fail(f"{name} is missing")
for root in SWEPT:
    for path in sorted(Path(root).rglob("*" + OLD_ID + "*")):
        rename(path, path.with_name(path.name.replace(OLD_ID, NEW_ID)))

# The GNOME Files extension is installed on the host beside upstream's, so it
# needs a file name, a load marker and a menu item name of its own.
rename("data/nautilus/hylki-nautilus.py", "data/nautilus/hylki-tuna-nautilus.py")
sub(
    "src/desktop/nautilus_ext.rs",
    r'"\.\./\.\./data/nautilus/hylki-nautilus\.py"',
    '"../../data/nautilus/hylki-tuna-nautilus.py"',
    count=1,
)
sub("src/desktop/nautilus_ext.rs", r'FILE_NAME: &str = "hylki-nautilus\.py"',
    'FILE_NAME: &str = "hylki-tuna-nautilus.py"', count=1)
sub("src/desktop/nautilus_ext.rs", r'"\.hylki-nautilus\.loaded"',
    '".hylki-tuna-nautilus.loaded"', count=1)
nautilus = "data/nautilus/hylki-tuna-nautilus.py"
sub(nautilus, r'"\.hylki-nautilus\.loaded"', '".hylki-tuna-nautilus.loaded"', count=1)
sub(nautilus, r'"hylki", "launcher"', f'"{DIR}", "launcher"', count=1)
sub(nautilus, r'name="HylkiMenuProvider::send"', 'name="HylkiTunaMenuProvider::send"', count=1)
sub(nautilus, r'^(\s*"(?:label|tip)": ".*)\bHylki\b', rf"\1{NAME}")

# --- The app ID, everywhere it is read at run time or installed -----------
# Required sites first, so a renamed constant fails here instead of slipping
# past the sweep.
sub("src/main.rs", r'^    "co\.hyprlab\.Hylki"$', f'    "{NEW_ID}"', count=1)
sub("src/main.rs", r'^    "co\.hyprlab\.Hylki\.Beta"$', f'    "{NEW_ID}.Beta"', count=1)
# Credentials live under their own Secret Service name, apart from upstream's.
sub("src/config/secrets.rs", r'KEYRING_SERVICE: &str = "co\.hyprlab\.Hylki";',
    f'KEYRING_SERVICE: &str = "{NEW_ID}";', count=1)
sub("data/org.tunaos.Hylki.Restart.service", r"^Name=co\.hyprlab\.Hylki\.Restart$",
    f"Name={NEW_ID}.Restart", count=1)
sub("data/org.tunaos.Hylki.desktop", r"^Icon=co\.hyprlab\.Hylki$", f"Icon={NEW_ID}", count=1)
sub("data/org.tunaos.Hylki.desktop", r"^StartupWMClass=co\.hyprlab\.Hylki$",
    f"StartupWMClass={NEW_ID}", count=1)
sub("data/org.tunaos.Hylki.metainfo.xml", r"<id>co\.hyprlab\.Hylki</id>", f"<id>{NEW_ID}</id>", count=1)
sub("resources/hylki.gresource.xml", r"icons/scalable/apps/co\.hyprlab\.Hylki\.svg",
    f"icons/scalable/apps/{NEW_ID}.svg", count=1)
# Then every other mention: icon includes, test expectations, comments.
for path, text in text_files(*SWEPT):
    if OLD_ID in text:
        path.write_text(text.replace(OLD_ID, NEW_ID))
# The bundled icons' resource prefix (/co/hyprlab/Hylki) is left alone: it is
# private to the process, and main.rs pins the base path to it explicitly.

# --- The display name -----------------------------------------------------
sub(
    "src/main.rs",
    r'(const APP_NAME: &str = if cfg!\(feature = "beta"\) \{\n    )"Hylki \(beta\)"(\n\} else \{\n    )"Hylki"',
    rf'\1"{NAME} (beta)"\2"{NAME}"',
    count=1,
)
sub("src/main.rs", r'set_application_name\("Hylki"\)', f'set_application_name("{NAME}")', count=1)
sub("src/app/tray/indicator.rs", r'(fn title\(&self\) -> String \{\n\s*)"Hylki"', rf'\1"{NAME}"', count=1)
# The About window's identity is the "Hylki" wordmark, an image; the name
# goes under it as text.
sub(
    "src/app/about.rs",
    r"^(    wm\.set_margin_bottom\(16\);\n    page\.append\(&wm\);\n)",
    "\\1    let name = gtk::Label::new(Some(crate::APP_NAME));\n"
    '    name.add_css_class("title-2");\n'
    "    page.append(&name);\n",
    count=1,
)
desktop = "data/org.tunaos.Hylki.desktop"
sub(desktop, r"^Name=Hylki$", f"Name={NAME}", count=1)
# Not a rename of Vireo or Veem: those are upstream's own predecessors.
sub(desktop, r"^# Earlier names.*\nX-Flatpak-RenamedFrom=.*\n", "", count=1)
metainfo = "data/org.tunaos.Hylki.metainfo.xml"
sub(metainfo, r"^  <name>Hylki</name>$", f"  <name>{NAME}</name>", count=1)
sub(
    metainfo,
    r"^  <summary>.*</summary>$",
    "  <summary>A GNOME email client with native Tuta and Proton Mail accounts</summary>",
    count=1,
)
sub(metainfo, r"^  <!-- Hylki was called .*-->\n  <replaces>\n(?:    <id>.*</id>\n)+  </replaces>\n", "", count=1)
sub(
    metainfo,
    r"^(  <description>\n)",
    r"\1    <p>\n      Hylki Tuna is Hylki with native Tuta and Proton Mail accounts, built\n"
    r"      by the TunaOS project from a fork that follows upstream Hylki. It\n"
    r"      installs beside Hylki and keeps its own settings, mail and passwords.\n"
    r"    </p>\n",
    count=1,
)

# --- Directories: hylki becomes hylki-tuna --------------------------------
# Config, data and cache all join "hylki" onto an XDG base; the MCP socket
# joins it onto the runtime directory outside Flatpak.
required_joins = [
    "src/config.rs",
    "src/config/accounts.rs",
    "src/cache/open.rs",
    "src/cache/bodies.rs",
    "src/desktop/app_icon.rs",
    "src/mcp.rs",
]
joins = re.compile(r'\.join\("hylki([/"])')
for name in required_joins:
    if not joins.search(Path(name).read_text()):
        fail(f'{name}: expected a .join("hylki…") directory')
for path, text in text_files("src"):
    if path.suffix == ".rs" and joins.search(text):
        path.write_text(joins.sub(rf'.join("{DIR}\1', text))
# The name an assistant's MCP client registers the server under.
sub("src/mcp.rs", r'^    "hylki-beta"\n\} else \{\n    "hylki"$', f'    "{DIR}-beta"\n}} else {{\n    "{DIR}"', count=1)

# --- Project links and contacts -------------------------------------------
about = "src/app/about.rs"
sub(about, r'"https://hylki\.hyprlab\.co"', f'"{REPO}"', count=1)
sub(about, r'"https://github\.com/hyprlab/hylki/issues"', f'"{REPO}/issues"', count=1)
sub(about, r'"https://github\.com/hyprlab/hylki",', f'"{REPO}",', count=1)
sub("src/ui/welcome/pages.rs", r'"url", "https://github\.com/hyprlab/hylki"', f'"url", "{REPO}"', count=1)
sub(metainfo, r'<url type="homepage">https://hylki\.hyprlab\.co</url>', f'<url type="homepage">{REPO}</url>', count=1)
sub(metainfo, r'<url type="bugtracker">https://github\.com/hyprlab/hylki/issues</url>',
    f'<url type="bugtracker">{REPO}/issues</url>', count=1)
sub(metainfo, r'<url type="vcs-browser">https://github\.com/hyprlab/hylki</url>',
    f'<url type="vcs-browser">{REPO}</url>', count=1)
sub(metainfo, r'<developer id="co\.hyprlab">\n    <name>Hyprlab</name>\n  </developer>',
    '<developer id="org.tunaos">\n    <name>TunaOS</name>\n  </developer>', count=1)
sub(metainfo, r"<update_contact>.*</update_contact>", f"<update_contact>{REPO}/issues</update_contact>", count=1)
sub(metainfo, r"raw\.githubusercontent\.com/hyprlab/hylki/", "raw.githubusercontent.com/tuna-os/hylki/")

# --- The Flatpak manifest -------------------------------------------------
# Upstream's manifest pins a tagged commit of hyprlab/hylki. A pinned source
# would be built without this rebrand, so the fork's manifest builds the tree
# it sits in instead: tools/local-manifest.sh makes that conversion, and its
# absolute paths are turned into ones relative to fork/.
manifest = Path("co.hyprlab.Hylki.yml").read_text()
manifest, n = re.subn(r"\A(?:#.*\n)+",
    "# Flatpak manifest for Hylki Tuna (org.tunaos.Hylki), the TunaOS fork of\n"
    f"# Hylki: {REPO}. Generated by fork/rebrand.sh from upstream's\n"
    "# manifest; it builds the rebranded tree it sits in (fork/README.md).\n",
    manifest)
if n != 1:
    fail("co.hyprlab.Hylki.yml: expected a leading comment block")
if not re.search(r"^id: co\.hyprlab\.Hylki$", manifest, re.M):
    fail("co.hyprlab.Hylki.yml: expected `id: co.hyprlab.Hylki`")
for needed in ("  - name: hylki-tuta\n", "  - name: hylki-proton\n", "org.freedesktop.Sdk.Extension.golang"):
    if needed not in manifest:
        fail(f"co.hyprlab.Hylki.yml: expected {needed.strip()!r}")
manifest = manifest.replace(OLD_ID, NEW_ID)
staged = Path(f"{NEW_ID}.rebrand.yml")
staged.write_text(manifest)
try:
    subprocess.run(["tools/local-manifest.sh", str(MANIFEST.resolve()), str(staged.resolve())],
                   check=True, stdout=subprocess.DEVNULL)
finally:
    staged.unlink()
manifest = MANIFEST.read_text()
root = str(tree)
if f"        path: {root}\n" not in manifest:
    fail(f"{MANIFEST}: tools/local-manifest.sh did not point the sources at {root}")
manifest = manifest.replace(f"        path: {root}\n", "        path: ..\n")
manifest = manifest.replace(f"      - {root}/", "      - ../")
if root in manifest:
    fail(f"{MANIFEST}: an absolute path to {root} is left")
MANIFEST.write_text(manifest)

sweep_check()
STAMP.write_text(f"{NEW_ID}\n")
print(f"rebrand: {tree} is now {NAME} ({NEW_ID})")
PY
