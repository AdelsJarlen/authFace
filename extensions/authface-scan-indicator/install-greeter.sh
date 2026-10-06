#!/usr/bin/env bash
#
# Install the authFace scan indicator system-wide, so it runs on the GDM login
# screen as well as the lock screen.
#
# The login screen runs GNOME Shell as a system user that cannot see anything
# under your home directory, so the extension has to live in /usr/share and be
# enabled in GDM's own dconf database. A per-user copy is removed: it would
# shadow this one in your session and drift out of sync with it.
#
set -euo pipefail

UUID="authface-scan-indicator@samvivan.local"
SRC="$(cd "$(dirname "$0")" && pwd)"
DEST="/usr/share/gnome-shell/extensions/$UUID"
DCONF_FILE="/etc/dconf/db/gdm.d/90-authface-scan-indicator"

if [ "$(id -u)" -ne 0 ]; then
    echo "Error: this installs into /usr/share and /etc — run it with sudo."
    exit 1
fi

if [ "${1:-}" = "--remove" ]; then
    rm -rf "$DEST"
    rm -f "$DCONF_FILE"
    dconf update
    echo "Removed $DEST and the GDM dconf entry."
    exit 0
fi

install -d -m 0755 "$DEST"
install -m 0644 "$SRC/extension.js" "$SRC/metadata.json" "$DEST/"
echo "Installed to $DEST"

install -d -m 0755 "$(dirname "$DCONF_FILE")"
cat > "$DCONF_FILE" <<EOF
[org/gnome/shell]
enabled-extensions=['$UUID']
EOF
dconf update
echo "Enabled for the login screen ($DCONF_FILE)"

if [ -n "${SUDO_USER:-}" ]; then
    user_home="$(getent passwd "$SUDO_USER" | cut -d: -f6)"
    user_copy="$user_home/.local/share/gnome-shell/extensions/$UUID"
    if [ -d "$user_copy" ]; then
        rm -rf "$user_copy"
        echo "Removed per-user copy $user_copy (the system copy now serves both)"
    fi
fi

cat <<'NOTES'

Done. Log out to see it on the login screen; your own session picks up the
system copy at the same time. To remove: sudo ./install-greeter.sh --remove
NOTES
