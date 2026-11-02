#!/usr/bin/env python3
# This Source Code Form is subject to the terms of the Mozilla Public
# License, v. 2.0. If a copy of the MPL was not distributed with this
# file, You can obtain one at https://mozilla.org/MPL/2.0/.
"""LAN console/process tests. All TCP/UDP addresses are 127.0.0.1.

UDP advertisements are injected via an explicit local unicast destination;
this verifies discovery semantics, NOT physical LAN broadcast reachability.
Build cargo build --release, then run this script. No IPA or GameKit used.
"""
import hashlib
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("UF_LAN_TEST_BINARY", ROOT / "target/release/Podroid"))
OUT = ROOT / "target/lan-tests"
NODES = []


class Console:
    def __init__(self, label, *args):
        self.saved = []
        self.queue = queue.Queue()
        self.process = subprocess.Popen([str(BINARY), "--multiplayer", *args, "--allow-network-access"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
        self.label = label
        NODES.append(self)
        def read():
            with (OUT / (label + ".jsonl")).open("w") as log:
                for line in self.process.stdout:
                    log.write(line); log.flush()
                    self.queue.put(json.loads(line))
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def wait(self, event, predicate=lambda value: True, timeout=6):
        for index, value in enumerate(self.saved):
            if value["event"] == event and predicate(value):
                return self.saved.pop(index)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                value = self.queue.get(timeout=max(.001, deadline - time.monotonic()))
            except queue.Empty:
                break
            if value["event"] == event and predicate(value):
                return value
            self.saved.append(value)
        raise AssertionError(f"{self.label} missing {event}, exit={self.process.poll()}, recent={self.saved[-6:]}")

    def command(self, value):
        line = value if isinstance(value, str) else json.dumps(value)
        self.process.stdin.write(line + "\n"); self.process.stdin.flush()
        return self.wait("command_result")["error"]

    def quit(self):
        assert self.command("quit") is None
        assert self.process.wait(timeout=4) == 0

    def list_rooms(self):
        self.saved = [e for e in self.saved if e["event"] != "rooms"]
        assert self.command("list") is None
        snapshots = [e for e in self.saved if e["event"] == "rooms"]
        assert snapshots
        self.saved = [e for e in self.saved if e["event"] != "rooms"]
        return snapshots[-1]["rooms"]


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    denied = subprocess.run([str(BINARY), "--multiplayer", "host"], input="", capture_output=True, text=True)
    assert denied.returncode and "disabled" in denied.stderr
    invalid = subprocess.run([str(BINARY), "--multiplayer", "join", "--address=[::1]:1234", "--allow-network-access"],
                             input="", capture_output=True, text=True)
    assert invalid.returncode and "IPv6" in invalid.stderr
    browser = Console("browser", "browse", "--discovery-bind=127.0.0.1:0", "--interval-ms=100", "--ttl-ms=600")
    target = browser.wait("discovery_ready")["address"]
    hosts = []
    for name, number in [("Alpha 雪山",1),("Beta",2)]:
        host = Console("host"+str(number), "host", "--name="+name, "--bind=127.0.0.1:0",
                       "--announce-bind=127.0.0.1:0", "--broadcast="+target,
                       "--player-id="+f"{number:032x}", "--interval-ms=100", "--ttl-ms=600")
        address = host.wait("listening")["address"]
        room = host.wait("joined")["session"]
        host.wait("discovery_ready")
        hosts.append((host,address,room))
    rooms = browser.wait("rooms", lambda e: len(e["rooms"])==2)["rooms"]
    assert {r["name"] for r in rooms} == {"Alpha 雪山","Beta"}
    assert all(r["players"]==1 and r["capacity"]==4 and r["protocol"]==1 for r in rooms)
    # Repeat heartbeats must not create duplicate room IDs.
    time.sleep(.25)
    current = browser.list_rooms()
    assert len(current)==2 and len({r["id"] for r in current})==2
    print("PASS multiple rooms, UTF-8 names, IP/port/count/version fields and duplicate suppression")

    host,address,room = hosts[0]
    assert browser.command({"command":"join","room":room}) is None
    browser.wait("joined", lambda e: e["session"]==room)
    browser_player = browser.wait("listening")["player"]
    host.wait("connected")
    browser.wait("connected")
    browser.wait("rooms", lambda e: any(r["id"]==room and r["players"]==2 for r in e["rooms"]))
    assert browser.command({"command":"send","to":f"{1:032x}","bytes":[0,255,0,128]}) is None
    assert host.wait("data")["bytes"] == [0,255,0,128]
    assert host.command({"command":"send","to":browser_player,"bytes":[128,0,255,0]}) is None
    assert browser.wait("data")["bytes"] == [128,0,255,0]
    print("PASS discovery selection joins the existing TCP transport and updates player count")

    manual = Console("manual", "join", "--address="+address, "--bind=127.0.0.1:0", "--player-id="+f"{3:032x}")
    manual.wait("joined")
    manual.wait("connected", lambda e: e["peer"]==f"{1:032x}")
    assert manual.command({"command":"send","to":f"{1:032x}","bytes":[7,0,9]}) is None
    assert host.wait("data")["bytes"] == [7,0,9]
    assert host.command({"command":"send","to":f"{3:032x}","bytes":[9,0,7]}) is None
    assert manual.wait("data")["bytes"] == [9,0,7]
    browser.wait("rooms", lambda e: any(r["id"]==room and r["players"]==3 for r in e["rooms"]))
    manual.quit()
    browser.wait("rooms", lambda e: any(r["id"]==room and r["players"]==2 for r in e["rooms"]))
    print("PASS independent manual-IP process, binary delivery and departure count update")

    # A malformed UDP packet must not stop a browser.
    ip,port=target.split(":")
    with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as udp:
        for packet in [b"", b"UFLD", b"x"*1000]:
            udp.sendto(packet,(ip,int(port)))
    assert len(browser.list_rooms())==2
    host.quit()
    browser.wait("disconnected",lambda e:e["peer"]==f"{1:032x}")
    browser.wait("rooms",lambda e:len(e["rooms"])==1 and e["rooms"][0]["id"]==hosts[1][2],timeout=.4)
    browser.saved = [e for e in browser.saved if e["event"]!="rooms"]
    hosts[1][0].process.kill(); hosts[1][0].process.wait(timeout=3)
    browser.wait("rooms",lambda e:not e["rooms"],timeout=4)
    print("PASS malformed datagrams, graceful Host goodbye, killed Host expiry and surviving browser")
    browser.quit()
    # No discovery event is ever emitted in manual mode.
    manual.reader.join(timeout=2)
    assert "discovery_" not in (OUT / "manual.jsonl").read_text()
    (OUT / "manifest.json").write_text(json.dumps({"binary_sha256":hashlib.sha256(BINARY.read_bytes()).hexdigest(),
        "scope":"127.0.0.1 UDP unicast fixtures and TCP; no physical LAN broadcast or VPN verification"},indent=2)+"\n")
    print("LAN PROCESS TESTS PASS; real two-device broadcast, VPN and Android/Windows UNVERIFIED")


if __name__ == "__main__":
    try:
        main()
    finally:
        for node in NODES:
            if node.process.poll() is None: node.process.kill()
            node.process.wait(timeout=3)
            node.reader.join(timeout=2)
            (OUT / (node.label + ".stderr.log")).write_text(node.process.stderr.read())
