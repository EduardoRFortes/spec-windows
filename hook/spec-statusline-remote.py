#!/usr/bin/env python3
"""spec-statusline-remote: statusLine hook for Claude Code running on a
remote box (VM/container reached via SSH, including VS Code Remote - SSH)
that specd on the Windows host never sees directly.

Mirrors hook/src/bin/spec-statusline.rs field-for-field (see that file for
the full rationale on the input/output shapes) but POSTs the usage snapshot
over HTTP instead of writing to the named pipe, since a remote box can't
reach a Windows named pipe. See PROTOCOL.md for the wire format and
install/REMOTE.md (or the README's "Sessoes remotas" section) for how the
SSH RemoteForward tunnel that makes localhost:27182 here land on specd's
listener on Windows gets set up.

Stdlib only, Python 3 -- no pip install needed on the remote box.
Fail-open: any network failure here is swallowed silently. This must never
block or fail the status line, let alone Claude Code itself.
"""

import http.client
import json
import sys

DAEMON_HOST = "127.0.0.1"
DAEMON_PORT = 27283
DAEMON_TIMEOUT = 0.3  # seconds -- same order of magnitude as spec-hook's own handshake timeout


def render_status_line(five_hour, seven_day):
    fh = (five_hour or {}).get("used_percentage")
    wk = (seven_day or {}).get("used_percentage")
    if fh is None or wk is None:
        return None
    return f"\U0001FAA8 sessão {fh:.0f}% · semana {wk:.0f}%"


def forward_to_daemon(five_hour, seven_day):
    payload = json.dumps(
        {
            "type": "usage",
            "five_hour_pct": (five_hour or {}).get("used_percentage"),
            "five_hour_resets_at": (five_hour or {}).get("resets_at"),
            "seven_day_pct": (seven_day or {}).get("used_percentage"),
            "seven_day_resets_at": (seven_day or {}).get("resets_at"),
        }
    ).encode("utf-8")
    try:
        conn = http.client.HTTPConnection(DAEMON_HOST, DAEMON_PORT, timeout=DAEMON_TIMEOUT)
        conn.request(
            "POST",
            "/usage",
            body=payload,
            headers={"Content-Type": "application/json", "Content-Length": str(len(payload))},
        )
        conn.getresponse().read()
        conn.close()
    except OSError:
        # Tunnel not up, specd not running, whatever -- fail open, same rule
        # as every other part of this project.
        pass


def main():
    try:
        raw = sys.stdin.read()
        data = json.loads(raw)
    except (ValueError, OSError):
        return

    rate_limits = data.get("rate_limits") or {}
    five_hour = rate_limits.get("five_hour")
    seven_day = rate_limits.get("seven_day")

    # rate_limits is absent for non-Pro/Max accounts and before the first
    # API response of the session -- print nothing rather than a blank or
    # misleading line, matching the Windows-local spec-statusline.exe.
    line = render_status_line(five_hour, seven_day)
    if line:
        print(line)
        # Explicit flush: stdout is a pipe here (not a tty), so Python
        # fully-buffers it by default. Claude Code reaps this process as
        # soon as it has the status line text -- without the flush, the
        # unbuffered print never reaches Claude Code in time and the
        # process gets killed before forward_to_daemon() below ever runs.
        # (Rust's println! doesn't need this: it line-buffers regardless
        # of tty, which is why spec-statusline.exe never hit this.)
        sys.stdout.flush()

    forward_to_daemon(five_hour, seven_day)


if __name__ == "__main__":
    main()
