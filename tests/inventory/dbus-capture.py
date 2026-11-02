#!/usr/bin/python3
"""Record every D-Bus message to or from org.freedesktop.systemd1 on a bus,
with the sending program, as JSON lines (step 0 of the user-manager plan,
work_history part 136).

    dbus-capture.py --session OUT.jsonl     # the bus in DBUS_SESSION_BUS_ADDRESS
    dbus-capture.py --system OUT.jsonl      # the system bus (as root)

Runs until killed. Uses GLib's Gio (python3-gobject), which GNOME installs.
"""
import json
import os
import sys
import threading
import time

from gi.repository import Gio, GLib

which, out = sys.argv[1], sys.argv[2]
bus_type = Gio.BusType.SESSION if which == "--session" else Gio.BusType.SYSTEM
address = Gio.dbus_address_get_for_bus_sync(bus_type, None)
flags = (Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT
         | Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION)
monitor = Gio.DBusConnection.new_for_address_sync(address, flags, None, None)
lookup = Gio.DBusConnection.new_for_address_sync(address, flags, None, None)

lock = threading.Lock()
names = {}
pending = []
f = open(out, "a", buffering=1)


def program(unique):
    """The command line behind a bus name, looked up once (it may be gone)."""
    if not unique:
        return None
    if unique in names:
        return names[unique]
    try:
        r = lookup.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus",
                             "org.freedesktop.DBus", "GetConnectionUnixProcessID",
                             GLib.Variant("(s)", (unique,)), None,
                             Gio.DBusCallFlags.NONE, 500, None)
        pid = r.unpack()[0]
        with open("/proc/%d/cmdline" % pid, "rb") as c:
            cmd = c.read().replace(b"\0", b" ").decode(errors="replace").strip()
        names[unique] = "%d %s" % (pid, cmd)
    except Exception as e:  # gone already, or not allowed
        names[unique] = "? (%s)" % type(e).__name__
    return names[unique]


active = False


def on_message(conn, msg, incoming):
    # Until BecomeMonitor has returned, its own reply must get through.
    if not active:
        return msg
    if not incoming:
        return msg
    kinds = {Gio.DBusMessageType.METHOD_CALL: "call",
             Gio.DBusMessageType.METHOD_RETURN: "return",
             Gio.DBusMessageType.ERROR: "error",
             Gio.DBusMessageType.SIGNAL: "signal"}
    body = msg.get_body()
    rec = {
        "t": round(time.time(), 3),
        "type": kinds.get(msg.get_message_type(), "?"),
        "serial": msg.get_serial(),
        "reply_to": msg.get_reply_serial(),
        "sender": msg.get_sender(),
        "dest": msg.get_destination(),
        "path": msg.get_path(),
        "iface": msg.get_interface(),
        "member": msg.get_member(),
        "error": msg.get_error_name(),
        "sig": msg.get_signature(),
        "body": body.print_(True)[:800] if body is not None else None,
    }
    with lock:
        f.write(json.dumps(rec) + "\n")
        if rec["type"] == "call" and rec["sender"] not in names:
            names[rec["sender"]] = None
            pending.append(rec["sender"])
    return None  # monitors only receive; drop it here


def resolve():
    """Name records for new senders, on the main loop (lookups from the
    filter's thread time out)."""
    while True:
        with lock:
            if not pending:
                break
            unique = pending.pop(0)
            names.pop(unique, None)
        p = program(unique)
        with lock:
            f.write(json.dumps({"t": round(time.time(), 3), "type": "name",
                                "unique": unique, "prog": p}) + "\n")
    return True


monitor.add_filter(on_message)
rules = ["destination='org.freedesktop.systemd1'",
         "sender='org.freedesktop.systemd1'"]
monitor.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus",
                  "org.freedesktop.DBus.Monitoring", "BecomeMonitor",
                  GLib.Variant("(asu)", (rules, 0)), None,
                  Gio.DBusCallFlags.NONE, -1, None)
active = True
f.write(json.dumps({"t": round(time.time(), 3), "type": "start",
                    "bus": which, "uid": os.getuid()}) + "\n")
GLib.timeout_add(20, resolve)
GLib.MainLoop().run()
