/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Standalone transport probe, not a server or an emulator startup mode.
//! JSON lines on stdin/stdout allow independent processes to be tested.
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use Podroid::transport::{Config, Delivery, Event, PlayerId, Role, Transport};

fn id(value: &str) -> Result<PlayerId, uuid::Error> {
    uuid::Uuid::parse_str(value).map(|v| PlayerId(*v.as_bytes()))
}
fn text(id: PlayerId) -> String {
    uuid::Uuid::from_bytes(id.0).simple().to_string()
}
fn output(value: Value) {
    println!("{value}");
    let _ = io::stdout().flush();
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 5 {
        return Err(
            "usage: transport_peer host|client BIND_IP:PORT HOST_IP:PORT|- PLAYER_UUID [--fast]"
                .into(),
        );
    }
    let role = match args[1].as_str() {
        "host" => Role::Host,
        "client" => Role::Client(args[3].parse()?),
        _ => return Err("role must be host or client".into()),
    };
    let mut config = Config::new(id(&args[4])?, args[2].parse()?, *b"UF-TRANSPORT-V1!");
    if args.iter().any(|s| s == "--fast") {
        config.heartbeat_interval = Duration::from_millis(100);
        config.peer_timeout = Duration::from_millis(800);
        config.handshake_timeout = Duration::from_millis(800);
    }
    let mut node = Transport::start(config, role)?;
    let (input, commands) = mpsc::sync_channel(16);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if input.send(line).is_err() {
                return;
            }
        }
    });
    let mut paused_until = Instant::now();
    loop {
        for _ in 0..16 {
            match commands.try_recv() {
                Ok(line) => {
                    let result = (|| -> Result<(), String> {
                        let value: Value =
                            serde_json::from_str(&line).map_err(|e| e.to_string())?;
                        if value["command"] == "pause_events" {
                            let ms = value["ms"]
                                .as_u64()
                                .filter(|ms| *ms <= 2000)
                                .ok_or("invalid diagnostic pause")?;
                            paused_until = Instant::now() + Duration::from_millis(ms);
                            return Ok(());
                        }
                        if value["command"] == "leave" {
                            node.disconnect();
                            return Ok(());
                        }
                        if value["command"] != "send" {
                            return Err("unknown command".into());
                        }
                        let to = id(value["to"].as_str().ok_or("missing player ID")?)
                            .map_err(|e| e.to_string())?;
                        let bytes = value["bytes"]
                            .as_array()
                            .ok_or("missing bytes")?
                            .iter()
                            .map(|v| {
                                v.as_u64()
                                    .and_then(|b| u8::try_from(b).ok())
                                    .ok_or("invalid byte")
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        let delivery = if value["unreliable"] == true {
                            Delivery::Unreliable
                        } else {
                            Delivery::ReliableOrdered
                        };
                        node.try_send(to, bytes, delivery)
                            .map_err(|e| format!("{e:?}"))
                    })();
                    output(json!({"event":"command_result","error":result.err()}));
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    node.disconnect();
                    break;
                }
            }
        }
        while Instant::now() >= paused_until {
            let Some(event) = node.poll() else { break };
            let value = match event {
                Event::Listening(address) => {
                    json!({"event":"listening","address":address.to_string()})
                }
                Event::Joined { session, host } => {
                    json!({"event":"joined","session":text(PlayerId(session.0)),"host":text(host)})
                }
                Event::Roster(peers) => {
                    json!({"event":"roster","peers":peers.iter().map(|p| text(p.id)).collect::<Vec<_>>()})
                }
                Event::PeerConnected(peer) => json!({"event":"connected","peer":text(peer)}),
                Event::Data { from, bytes } | Event::UnreliableData { from, bytes } => {
                    json!({"event":"data","from":text(from),"bytes":bytes})
                }
                Event::PeerDisconnected { peer, reason } => {
                    json!({"event":"disconnected","peer":text(peer),"reason":format!("{reason:?}")})
                }
                Event::Error { peer, reason } => {
                    json!({"event":"error","peer":peer.map(text),"reason":format!("{reason:?}")})
                }
                Event::Stopped(reason) => {
                    output(json!({"event":"stopped","reason":format!("{reason:?}")}));
                    return Ok(());
                }
            };
            output(value);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
