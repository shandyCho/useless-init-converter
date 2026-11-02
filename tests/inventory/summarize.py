#!/usr/bin/python3
"""Summarise tests/user-inventory.sh's captures: per bus, each method called
on org.freedesktop.systemd1 with its callers, error replies, the signals
the manager sent, and the properties read.

    summarize.py OUTDIR
"""
import collections
import glob
import json
import os
import re
import sys

out = sys.argv[1]


def prog(frm):
    """The program name from "PID cmdline", or "?"."""
    if not frm or frm.startswith("?"):
        return "?"
    parts = frm.split(" ", 2)
    cmd = parts[1] if len(parts) > 1 else "?"
    if os.path.basename(cmd).startswith("python") and len(parts) > 2:
        cmd = parts[2].split(" ")[0]
    return os.path.basename(cmd)


SD = "org.freedesktop.systemd1"

for path in sorted(glob.glob(os.path.join(out, "*.jsonl"))):
    recs = [json.loads(l) for l in open(path) if l.strip()]
    # The monitor sees the whole bus: keep what goes to or comes from the
    # manager, by its well-known name or its unique name (GDBusProxy uses
    # the latter), found from its replies.
    progs = {r["unique"]: r["prog"] for r in recs if r["type"] == "name"}
    sent = {(r["sender"], r["serial"]): r for r in recs if r["type"] == "call"}
    sd = set()
    for r in recs:
        if r["type"] in ("return", "error"):
            c = sent.get((r.get("dest"), r.get("reply_to")))
            if c and c.get("dest") == SD:
                sd.add(r["sender"])
    for r in recs:
        if r["type"] == "call":
            r["from"] = progs.get(r["sender"]) or r.get("from")
    recs = [r for r in recs if r.get("dest") in sd | {SD} or r.get("sender") in sd]
    calls = collections.defaultdict(collections.Counter)
    first = {}
    errors = collections.Counter()
    signals = collections.Counter()
    props = collections.Counter()
    by_serial = {}
    mine = collections.Counter()
    for r in recs:
        if r["type"] == "call" and r.get("sender") in sd:
            mine["%s.%s" % ((r.get("iface") or "").rsplit(".", 1)[-1], r.get("member"))] += 1
        elif r["type"] == "call":
            key = "%s.%s" % ((r.get("iface") or "").rsplit(".", 1)[-1], r.get("member"))
            calls[key][prog(r.get("from"))] += 1
            first.setdefault(key, r["t"])
            by_serial[(r.get("sender"), r.get("serial"))] = (key, r.get("body"))
            if r.get("iface") == "org.freedesktop.DBus.Properties" and r.get("body"):
                q = re.findall(r"'([A-Za-z0-9_.]+)'", r["body"])
                props["%s %s" % (r.get("member"), ".".join(x.rsplit(".", 1)[-1] for x in q[:2]))] += 1
        elif r["type"] == "error":
            k = by_serial.get((r.get("dest"), r.get("reply_to")), ("?", None))
            errors["%s -> %s %s" % (k[0], r.get("error"), (r.get("body") or "")[:120])] += 1
        elif r["type"] == "signal" and r.get("iface"):
            iface = r["iface"].rsplit(".", 1)[-1]
            if r.get("member") == "PropertiesChanged" and r.get("body"):
                m = re.match(r"\('([^']+)'", r["body"])
                iface += "(%s)" % (m.group(1).rsplit(".", 1)[-1] if m else "?")
            signals["%s.%s" % (iface, r.get("member"))] += 1
    print("=" * 70)
    print(os.path.basename(path), "-", len(recs), "messages")
    print("-- methods called on org.freedesktop.systemd1 (first call order): count, callers")
    for key in sorted(calls, key=lambda k: first[k]):
        c = calls[key]
        print("  %-42s %4d  %s" % (key, sum(c.values()),
                                    ", ".join("%s %d" % kv for kv in c.most_common())))
    if mine:
        print("-- calls the manager itself made")
        for k, n in mine.most_common():
            print("  %-42s %4d" % (k, n))
    if props:
        print("-- properties read (Get, first argument after the interface)")
        for k, n in props.most_common():
            print("  %-50s %4d" % (k, n))
    if errors:
        print("-- error replies")
        for k, n in errors.most_common():
            print("  %4d  %s" % (n, k))
    if signals:
        print("-- signals from the manager")
        for k, n in signals.most_common():
            print("  %-42s %4d" % (k, n))
for name in ("mem-session.txt", "mem-greeter.txt", "app.txt"):
    p = os.path.join(out, name)
    if os.path.exists(p):
        print("=" * 70)
        print(name)
        print(open(p).read().rstrip())
