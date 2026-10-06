import { Extension } from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import Cairo from 'cairo';
import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import St from 'gi://St';

const STATUS_FILENAME = 'face-auth-status';
const RESULT_SHOW_MS = 1800;
// face-auth exits as soon as it matches and GNOME unlocks immediately, so the
// success animation is finished over the fading lock screen instead.
const UNLOCK_SHOW_MS = 900;
const STALE_SCAN_MS = 10000;
// Statuses older than this are leftovers from an earlier attempt or session.
const STATUS_MAX_AGE_MS = 15000;
const PAUSED_SHOW_MS = 3500;
const RESULT_ANIM_MS = 550;
const GLYPH_SIZE = 96;

// Session modes in which the unlock UI is on screen. GNOME has no mode called
// 'lock' — the shield is 'lock-screen' and the password/unlock prompt is
// 'unlock-dialog'. Testing for 'lock' matched nothing, so the indicator never
// appeared. 'gdm' is the login screen.
const LOCKED_MODES = ['unlock-dialog', 'lock-screen', 'gdm'];

// The login screen runs as a system user that cannot see /run/user/<uid>, so
// face-auth mirrors its status here for GDM's PAM stacks.
const GREETER_STATUS_PATH = '/run/face-auth/status';

const WHITE = [1.0, 1.0, 1.0];
const CYAN = [0.30, 0.82, 0.88];
const GREEN = [0.20, 0.78, 0.35];
const RED = [1.0, 0.27, 0.23];
const AMBER = [1.0, 0.72, 0.18];

const LABELS = {
    scanning: 'Scanning face…',
    ok: 'Face recognised',
    fail: 'Face not recognised — use your password',
    paused: 'Face unlock paused — use fingerprint or password',
};

function setColor(cr, [r, g, b], a = 1) {
    cr.setSourceRGBA(r, g, b, a);
}

function easeOutCubic(p) {
    return 1 - Math.pow(1 - p, 3);
}

// All drawing is in a 100×100 box, scaled to the actor in _draw().

/// Four rounded corner brackets, Face ID style. `inset` pulls them inwards.
function drawCorners(cr, inset) {
    const a = 8 + inset, b = 92 - inset, r = 14, len = 14;
    cr.newPath();
    cr.moveTo(a, a + r + len);
    cr.lineTo(a, a + r);
    cr.arc(a + r, a + r, r, Math.PI, 1.5 * Math.PI);
    cr.lineTo(a + r + len, a);

    cr.moveTo(b - r - len, a);
    cr.lineTo(b - r, a);
    cr.arc(b - r, a + r, r, 1.5 * Math.PI, 2 * Math.PI);
    cr.lineTo(b, a + r + len);

    cr.moveTo(b, b - r - len);
    cr.lineTo(b, b - r);
    cr.arc(b - r, b - r, r, 0, 0.5 * Math.PI);
    cr.lineTo(b - r - len, b);

    cr.moveTo(a + r + len, b);
    cr.lineTo(a + r, b);
    cr.arc(a + r, b - r, r, 0.5 * Math.PI, Math.PI);
    cr.lineTo(a, b - r - len);
    cr.stroke();
}

/// Eyes, nose and mouth. `smile` is 1 for a smile, negative for a frown.
function drawFace(cr, smile = 1) {
    cr.newPath();
    cr.moveTo(36, 37);
    cr.lineTo(36, 45);
    cr.moveTo(64, 37);
    cr.lineTo(64, 45);
    cr.moveTo(51, 37);
    cr.lineTo(51, 56);
    cr.lineTo(46, 56);
    const y = smile >= 0 ? 66 : 70;
    const dip = 7 * smile;
    cr.moveTo(36, y);
    cr.curveTo(43, y + dip, 57, y + dip, 64, y);
    cr.stroke();
}

/// Stroke the polyline `points` up to `fraction` of its length.
function drawPartialPath(cr, points, fraction) {
    const segments = [];
    let total = 0;
    for (let i = 1; i < points.length; i++) {
        const [x0, y0] = points[i - 1], [x1, y1] = points[i];
        const length = Math.hypot(x1 - x0, y1 - y0);
        segments.push([x0, y0, x1, y1, length]);
        total += length;
    }

    let remaining = total * fraction;
    cr.newPath();
    cr.moveTo(points[0][0], points[0][1]);
    for (const [x0, y0, x1, y1, length] of segments) {
        if (remaining <= 0)
            break;
        const f = Math.min(1, remaining / length);
        cr.lineTo(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
        remaining -= length;
    }
    cr.stroke();
}

export default class AuthFaceScanIndicator extends Extension {
    enable() {
        this._monitor = null;
        this._monitorChangedId = null;
        this._hideTimeoutId = null;
        this._staleTimeoutId = null;
        this._state = null;
        this._stateStart = 0;
        this._wasLocked = false;

        this._area = new St.DrawingArea({
            width: GLYPH_SIZE,
            height: GLYPH_SIZE,
            x_align: Clutter.ActorAlign.CENTER,
        });
        this._repaintId = this._area.connect('repaint', area => this._draw(area));

        this._label = new St.Label({
            text: '',
            x_align: Clutter.ActorAlign.CENTER,
        });
        this._label.set_style('font-size: 15px; font-weight: 600; color: #ffffff;');

        this._box = new St.BoxLayout({
            reactive: false,
            style: 'spacing: 14px;',
            x_align: Clutter.ActorAlign.CENTER,
        });
        // `orientation` replaced `vertical` in GNOME 48.
        if ('orientation' in this._box)
            this._box.orientation = Clutter.Orientation.VERTICAL;
        else
            this._box.vertical = true;
        this._box.add_child(this._area);
        this._box.add_child(this._label);

        this._actor = new St.Bin({
            reactive: false,
            visible: false,
            style: `
                background-color: rgba(0, 0, 0, 0.55);
                border-radius: 28px;
                border: 1px solid rgba(255, 255, 255, 0.14);
                padding: 20px 28px 16px 28px;
            `,
        });
        this._actor.set_child(this._box);

        // Drives the animation; runs only while the indicator is visible.
        this._timeline = new Clutter.Timeline({
            actor: this._area,
            duration: 1000,
            repeat_count: -1,
        });
        this._frameId = this._timeline.connect('new-frame', () => this._area.queue_repaint());

        Main.uiGroup.add_child(this._actor);

        // Re-centre whenever the bubble's own size changes (the text length
        // differs between states) or the monitor layout changes.
        this._notifyId = this._actor.connect('notify::size', () => this._place());
        this._monitorsId = Main.layoutManager.connect('monitors-changed', () => this._place());
        this._modeId = Main.sessionMode.connect('updated', () => this._onModeChanged());

        this._place();
        this._onModeChanged();
    }

    disable() {
        this._stopWatching();
        this._clearHideTimeout();
        this._clearStaleTimeout();

        if (this._modeId) {
            Main.sessionMode.disconnect(this._modeId);
            this._modeId = null;
        }
        if (this._monitorsId) {
            Main.layoutManager.disconnect(this._monitorsId);
            this._monitorsId = null;
        }
        if (this._timeline) {
            this._timeline.stop();
            this._timeline.disconnect(this._frameId);
            this._timeline = null;
        }
        if (this._area) {
            this._area.disconnect(this._repaintId);
            this._area = null;
        }
        if (this._actor) {
            if (this._notifyId) {
                this._actor.disconnect(this._notifyId);
                this._notifyId = null;
            }
            Main.uiGroup.remove_child(this._actor);
            this._actor.destroy();
            this._actor = null;
        }
        this._label = null;
        this._box = null;
    }

    _isLocked() {
        return Main.sessionMode && LOCKED_MODES.includes(Main.sessionMode.currentMode);
    }

    _statusPath() {
        if (Main.sessionMode.currentMode === 'gdm')
            return GREETER_STATUS_PATH;
        return GLib.build_filenamev([GLib.get_user_runtime_dir(), STATUS_FILENAME]);
    }

    /// Watch the status file for changes instead of polling it ten times a
    /// second for the whole session.
    _startWatching() {
        if (this._monitor)
            return;
        try {
            const file = Gio.File.new_for_path(this._statusPath());
            this._monitor = file.monitor_file(Gio.FileMonitorFlags.NONE, null);
            this._monitorChangedId = this._monitor.connect('changed', () => this._onStatusChanged());
        } catch (e) {
            logError(e, 'authFace: could not watch scan status file');
            this._monitor = null;
            return;
        }
        // The helper may have written before the watch was established.
        this._onStatusChanged();
    }

    _stopWatching() {
        if (this._monitorChangedId && this._monitor) {
            this._monitor.disconnect(this._monitorChangedId);
            this._monitorChangedId = null;
        }
        if (this._monitor) {
            this._monitor.cancel();
            this._monitor = null;
        }
    }

    _readStatus() {
        try {
            // The greeter's status file cannot be unlinked by this user, so a
            // result from before a logout is still there; ignore stale ones.
            const info = Gio.File.new_for_path(this._statusPath())
                .query_info('time::modified', Gio.FileQueryInfoFlags.NOFOLLOW_SYMLINKS, null);
            const modified = info.get_modification_date_time();
            const ageMs = GLib.DateTime.new_now_utc().difference(modified) / 1000;
            if (ageMs > STATUS_MAX_AGE_MS)
                return null;

            const [ok, contents] = GLib.file_get_contents(this._statusPath());
            if (!ok || contents === null || contents.length === 0)
                return null;
            return new TextDecoder().decode(contents).trim();
        } catch (e) {
            return null;
        }
    }

    _unlinkStatus() {
        try {
            Gio.File.new_for_path(this._statusPath()).delete(null);
        } catch (e) {
            /* already gone */
        }
    }

    _onModeChanged() {
        const locked = this._isLocked();
        const wasLocked = this._wasLocked;
        this._wasLocked = locked;

        if (locked) {
            this._startWatching();
            return;
        }

        this._stopWatching();
        // Just unlocked by face: the 'ok' may have landed after the last file
        // event was delivered, so check once more and let the check mark
        // finish over the fading lock screen.
        if (wasLocked && (this._state === 'ok' || this._readStatus() === 'ok')) {
            this._showResult('ok', UNLOCK_SHOW_MS);
            return;
        }
        this._hide();
        this._unlinkStatus();
    }

    _onStatusChanged() {
        if (!this._isLocked()) {
            this._hide();
            return;
        }

        const status = this._readStatus();
        if (status === null) {
            this._hide();
            return;
        }

        if (status === 'scanning') {
            this._showScanning();
            this._armStaleTimeout();
        } else if (status === 'ok' || status === 'fail') {
            this._clearStaleTimeout();
            this._showResult(status);
        } else if (status === 'paused') {
            this._clearStaleTimeout();
            this._showResult(status, PAUSED_SHOW_MS);
        }
    }

    /// A crashed helper leaves 'scanning' behind with no further file events,
    /// so the bubble is cleared on a timer rather than on the next poll.
    _armStaleTimeout() {
        this._clearStaleTimeout();
        this._staleTimeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, STALE_SCAN_MS, () => {
            this._staleTimeoutId = null;
            if (this._readStatus() === 'scanning') {
                this._unlinkStatus();
                this._hide();
            }
            return GLib.SOURCE_REMOVE;
        });
    }

    _setState(state) {
        this._state = state;
        this._stateStart = GLib.get_monotonic_time() / 1000;
        this._label.set_text(LABELS[state]);
        this._actor.visible = true;
        this._place();
        if (!this._timeline.is_playing())
            this._timeline.start();
        this._area.queue_repaint();
    }

    _showScanning() {
        // A result on screen wins over a late 'scanning' event.
        if (this._state !== null)
            return;
        this._setState('scanning');
    }

    _showResult(state, holdMs = RESULT_SHOW_MS) {
        if (this._state !== state)
            this._setState(state);

        this._clearHideTimeout();
        this._hideTimeoutId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, holdMs, () => {
            this._hideTimeoutId = null;
            this._hide();
            this._unlinkStatus();
            return GLib.SOURCE_REMOVE;
        });
    }

    _hide() {
        this._state = null;
        this._clearHideTimeout();
        if (this._timeline)
            this._timeline.stop();
        if (this._actor)
            this._actor.visible = false;
    }

    _draw(area) {
        const cr = area.get_context();
        const [width, height] = area.get_surface_size();
        const elapsedMs = GLib.get_monotonic_time() / 1000 - this._stateStart;
        const t = elapsedMs / 1000;

        cr.scale(width / 100, height / 100);
        cr.setLineCap(Cairo.LineCap.ROUND);
        cr.setLineJoin(Cairo.LineJoin.ROUND);
        cr.setLineWidth(5);

        if (this._state === 'scanning')
            this._drawScanning(cr, t);
        else if (this._state === 'ok' || this._state === 'fail')
            this._drawResult(cr, Math.min(1, elapsedMs / RESULT_ANIM_MS), t, this._state === 'ok');
        else if (this._state === 'paused')
            this._drawPaused(cr, Math.min(1, elapsedMs / RESULT_ANIM_MS));

        cr.$dispose();
    }

    _drawScanning(cr, t) {
        // Brackets breathe; a beam sweeps down and up across the face.
        const pulse = (Math.sin(t * 2 * Math.PI / 1.4) + 1) / 2;
        setColor(cr, WHITE, 0.55 + 0.45 * pulse);
        drawCorners(cr, 3 * pulse);

        setColor(cr, WHITE, 0.9);
        drawFace(cr);

        const y = 50 - 34 * Math.cos(t * 2 * Math.PI / 1.6);
        const [r, g, b] = CYAN;
        const beam = new Cairo.LinearGradient(14, 0, 86, 0);
        beam.addColorStopRGBA(0, r, g, b, 0);
        beam.addColorStopRGBA(0.5, r, g, b, 0.95);
        beam.addColorStopRGBA(1, r, g, b, 0);
        cr.setSource(beam);
        cr.setLineWidth(3);
        cr.newPath();
        cr.moveTo(14, y);
        cr.lineTo(86, y);
        cr.stroke();
    }

    _drawResult(cr, p, t, success) {
        const e = easeOutCubic(p);
        const color = success ? GREEN : RED;

        if (!success) {
            // A decaying head-shake.
            cr.translate(Math.sin(t * 2 * Math.PI * 7) * 6 * (1 - p), 0);
        }

        setColor(cr, color);
        drawCorners(cr, success ? 3 * e : 0);

        if (success) {
            // The face gives way to a check mark drawn stroke by stroke.
            if (e < 1) {
                setColor(cr, color, 1 - e);
                drawFace(cr);
            }
            setColor(cr, color);
            cr.setLineWidth(6);
            drawPartialPath(cr, [[32, 52], [45, 65], [70, 37]], e);
        } else {
            drawFace(cr, -e);
        }
    }

    _drawPaused(cr, p) {
        // Amber, expressionless face with a pause badge fading in.
        const e = easeOutCubic(p);
        setColor(cr, AMBER);
        drawCorners(cr, 0);
        drawFace(cr, 0);

        // Cut a gap in the bracket so the badge stands clear of it.
        cr.save();
        cr.setOperator(Cairo.Operator.CLEAR);
        cr.newPath();
        cr.arc(80, 80, 19 * e, 0, 2 * Math.PI);
        cr.fill();
        cr.restore();

        setColor(cr, AMBER, e);
        cr.newPath();
        cr.arc(80, 80, 14, 0, 2 * Math.PI);
        cr.fill();
        cr.setSourceRGBA(0, 0, 0, 0.85 * e);
        cr.rectangle(74, 73, 4, 14);
        cr.rectangle(82, 73, 4, 14);
        cr.fill();
    }

    _clearHideTimeout() {
        if (this._hideTimeoutId !== null) {
            GLib.source_remove(this._hideTimeoutId);
            this._hideTimeoutId = null;
        }
    }

    _clearStaleTimeout() {
        if (this._staleTimeoutId !== null) {
            GLib.source_remove(this._staleTimeoutId);
            this._staleTimeoutId = null;
        }
    }

    /// Centre the card horizontally, above the unlock prompt.
    _place() {
        const monitor = Main.layoutManager.primaryMonitor;
        if (!monitor || !this._actor)
            return;
        const width = this._actor.width;
        this._actor.x = monitor.x + Math.max(0, Math.floor((monitor.width - width) / 2));
        this._actor.y = monitor.y + Math.floor(monitor.height * 0.12);
    }
}
