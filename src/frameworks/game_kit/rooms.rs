/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! LAN directory for real GameKit sessions, containing no guest objects.
//!
//! In automatic mode (`--gamekit-auto`), matchmaking first looks for a room
//! for a short while and joins it, or else hosts one. Hosts keep looking
//! until someone joins, and if two hosted at the same time, the one with the
//! greater room ID joins the other, so that both end up in one room.
use crate::lan::{self, Discovery};
use crate::options::Options;
use crate::transport::{self, Config, Event, Role, SessionId};
use std::net::{SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    browser: Option<Discovery>,
    publisher: Option<Discovery>,
    waiting: Option<Config>,
    selected: Option<SessionId>,
    wanted: Option<SessionId>,
    name: Option<String>,
    targets: Vec<SocketAddrV4>,
    compatibility: [u8; 16],
    capacity: u8,
    players: u8,
    listening: Option<SocketAddrV4>,
    session: Option<SessionId>,
    host: bool,
    advertised_players: Option<u8>,
    observed: Vec<lan::Room>,
    first_seen: Option<Instant>,
    /// Automatic mode: the configuration, kept to switch from hosting to
    /// joining.
    auto: Option<Config>,
    /// Automatic mode: when to give up looking and host instead.
    host_at: Option<Instant>,
}

/// Room name used in automatic mode without `--gamekit-room`.
const AUTO_ROOM_NAME: &str = "Podroid";
impl State {
    pub fn start(
        &mut self,
        options: &Options,
        config: Config,
    ) -> Result<Option<(Config, Role)>, String> {
        *self = Self::default();
        self.host = matches!(options.gamekit_role, Some(Role::Host));
        self.name = options.gamekit_room.clone();
        self.targets = options.gamekit_broadcasts.clone();
        self.compatibility = config.compatibility;
        self.capacity = config.max_players as u8;
        self.players = 1;
        if options.gamekit_auto {
            self.name.get_or_insert_with(|| AUTO_ROOM_NAME.into());
            let mut discovery = lan::Config::browser();
            discovery.bind = options.gamekit_discovery_bind;
            discovery.compatibility = Some(config.compatibility);
            match Discovery::browse(discovery) {
                Ok(browser) => self.browser = Some(browser),
                Err(e) => {
                    // E.g. another instance on this machine is looking.
                    // Others can still find this one if it hosts.
                    log!("[LocalGameKit] room discovery unavailable ({e}), hosting");
                    self.host = true;
                    return Ok(Some((config, Role::Host)));
                }
            }
            // Spread out when instances started together give up looking.
            let jitter = u64::from(config.player.0[0]) * 4;
            self.host_at = Some(Instant::now() + Duration::from_millis(2000 + jitter));
            self.auto = Some(config.clone());
            self.waiting = Some(config);
            return Ok(None);
        }
        if let Some(wanted) = &options.gamekit_discover {
            self.wanted = if wanted == "auto" {
                None
            } else {
                Some(SessionId(
                    *uuid::Uuid::parse_str(wanted)
                        .map_err(|_| "Invalid room UUID")?
                        .as_bytes(),
                ))
            };
            let mut discovery = lan::Config::browser();
            discovery.bind = options.gamekit_discovery_bind;
            discovery.compatibility = Some(config.compatibility);
            self.browser =
                Some(Discovery::browse(discovery).map_err(|e| format!("Room discovery: {e}"))?);
            self.waiting = Some(config);
            Ok(None)
        } else {
            Ok(Some((
                config,
                options.gamekit_role.ok_or("No GameKit connection mode")?,
            )))
        }
    }
    /// Whether this instance hosts the current room.
    pub fn is_host(&self) -> bool {
        self.host
    }
    pub fn poll(&mut self) -> Option<Result<(Config, Role), String>> {
        for _ in 0..16 {
            let Some(event) = self.browser.as_mut()?.poll() else {
                break;
            };
            match event {
                lan::Event::Rooms(rooms) => {
                    if !rooms.is_empty() {
                        self.first_seen.get_or_insert_with(Instant::now);
                    }
                    if self.observed != rooms {
                        for room in &rooms {
                            log!(
                                "[LocalGameKit] room {} {:?} {:?}",
                                uuid::Uuid::from_bytes(room.info.id.0),
                                room.info.name,
                                room.endpoints
                            );
                        }
                    }
                    self.observed = rooms;
                }
                lan::Event::Error(error) if self.auto.is_some() => {
                    log!("[LocalGameKit] room discovery: {error}");
                    self.browser = None;
                    return self.auto_host();
                }
                lan::Event::Stopped(reason) if self.auto.is_some() => {
                    log!("[LocalGameKit] room discovery stopped: {reason}");
                    self.browser = None;
                    return self.auto_host();
                }
                lan::Event::Error(error) => {
                    self.browser = None;
                    return Some(Err(format!("Room discovery: {error}")));
                }
                lan::Event::Stopped(reason) => {
                    self.browser = None;
                    return Some(Err(format!("Room discovery stopped: {reason}")));
                }
                lan::Event::Ready(_) => (),
            }
        }
        if self.auto.is_some() {
            return self.poll_auto();
        }
        if self.wanted.is_some()
            || self
                .first_seen
                .is_some_and(|time| time.elapsed() >= Duration::from_secs(1))
        {
            match select(&self.observed, self.wanted) {
                Ok(Some((id, address))) => {
                    self.selected = Some(id);
                    self.browser = None;
                    return Some(Ok((self.waiting.take()?, Role::Client(address))));
                }
                Ok(None) => (),
                Err(error) => {
                    self.browser = None;
                    return Some(Err(error));
                }
            }
        }
        None
    }
    fn poll_auto(&mut self) -> Option<Result<(Config, Role), String>> {
        let own = self.session;
        let room = auto_select(&self.observed, own);
        if self.waiting.is_some() {
            // Give rooms a moment to show up together, so the same one is
            // picked as others would pick.
            let settled = self
                .first_seen
                .is_some_and(|time| time.elapsed() >= Duration::from_secs(1));
            if let (Some((id, address)), true) = (room, settled) {
                return Some(self.auto_join(id, address));
            }
            if self.host_at.is_some_and(|time| Instant::now() >= time) {
                return self.auto_host();
            }
            return None;
        }
        // Hosting: if another room appeared before anyone joined this one,
        // only one of the two hosts gives way.
        let (Some(own), Some((id, address))) = (own, room) else {
            return None;
        };
        if self.host && self.players <= 1 && id.0 < own.0 {
            log!(
                "[LocalGameKit] joining room {} instead of hosting",
                uuid::Uuid::from_bytes(id.0)
            );
            self.host = false;
            self.publisher = None;
            self.advertised_players = None;
            self.listening = None;
            self.session = None;
            self.waiting = self.auto.clone();
            return Some(self.auto_join(id, address));
        }
        None
    }
    fn auto_join(
        &mut self,
        id: SessionId,
        address: SocketAddrV4,
    ) -> Result<(Config, Role), String> {
        log!(
            "[LocalGameKit] joining room {} at {}",
            uuid::Uuid::from_bytes(id.0),
            address
        );
        self.selected = Some(id);
        self.browser = None;
        self.host_at = None;
        let config = self.waiting.take().ok_or("No GameKit configuration")?;
        Ok((config, Role::Client(address)))
    }
    fn auto_host(&mut self) -> Option<Result<(Config, Role), String>> {
        self.host_at = None;
        let config = self.waiting.take()?;
        log!("[LocalGameKit] no room found, hosting");
        self.host = true;
        Some(Ok((config, Role::Host)))
    }
    pub fn observe(&mut self, event: &Event, accepting: bool) -> Result<(), String> {
        match event {
            Event::Listening(address) => self.listening = Some(*address),
            Event::Joined { session, .. } => {
                if self.selected.is_some_and(|expected| expected != *session) {
                    return Err(
                        "Discovered room restarted or session changed; rediscover before joining"
                            .into(),
                    );
                }
                self.session = Some(*session);
            }
            Event::Roster(peers) => {
                self.players = peers.len() as u8;
                if self.players > 1 {
                    // Someone joined: stop looking for other rooms.
                    self.browser = None;
                }
            }
            Event::Stopped(_) => {
                self.publisher = None;
                self.session = None;
            }
            _ => (),
        }
        if let Err(error) = self.admission(accepting) {
            log!("[LocalGameKit] {error}");
        }
        Ok(())
    }
    pub fn admission(&mut self, open: bool) -> Result<(), String> {
        if !self.host || !open {
            self.publisher = None;
            self.advertised_players = None;
            return Ok(());
        }
        let (Some(name), Some(listen), Some(session)) = (&self.name, self.listening, self.session)
        else {
            return Ok(());
        };
        if let Some(publisher) = &mut self.publisher {
            if self.advertised_players != Some(self.players) {
                publisher
                    .update_players(self.players)
                    .map_err(str::to_string)?;
                self.advertised_players = Some(self.players);
            }
            // Drain bounded discovery diagnostics; publication failure does
            // not turn a live direct game connection into a network success.
            for _ in 0..16 {
                let Some(event) = publisher.poll() else {
                    break;
                };
                if let lan::Event::Error(error) = event {
                    return Err(error);
                }
            }
        } else {
            let mut config = lan::Config::publisher();
            if !self.targets.is_empty() {
                config.targets = self.targets.clone();
            }
            self.publisher = Some(
                Discovery::advertise(
                    config,
                    lan::Advertisement {
                        id: session,
                        name: name.clone(),
                        port: listen.port(),
                        players: self.players,
                        capacity: self.capacity,
                        protocol: transport::PROTOCOL_VERSION,
                        compatibility: self.compatibility,
                    },
                )
                .map_err(|e| format!("Room advertisement: {e}"))?,
            );
            self.advertised_players = Some(self.players);
        }
        Ok(())
    }
}

/// The room to join in automatic mode: the available one with the smallest
/// ID, so instances seeing the same rooms agree. `own` is the room this
/// instance hosts, if any.
fn auto_select(rooms: &[lan::Room], own: Option<SessionId>) -> Option<(SessionId, SocketAddrV4)> {
    rooms
        .iter()
        .filter(|room| {
            room.info.protocol == transport::PROTOCOL_VERSION
                && room.info.players < room.info.capacity
                && Some(room.info.id) != own
        })
        .filter_map(|room| {
            room.endpoints.iter().find_map(|address| match address {
                SocketAddr::V4(address) => Some((room.info.id, *address)),
                _ => None,
            })
        })
        .min_by_key(|(id, _)| id.0)
}

fn select(
    rooms: &[lan::Room],
    wanted: Option<SessionId>,
) -> Result<Option<(SessionId, SocketAddrV4)>, String> {
    let candidates = rooms
        .iter()
        .filter(|room| {
            room.info.protocol == transport::PROTOCOL_VERSION
                && room.info.players < room.info.capacity
                && wanted.is_none_or(|id| id == room.info.id)
        })
        .collect::<Vec<_>>();
    if candidates.len() > 1 {
        return Err("Multiple compatible rooms found; select --gamekit-discover=ROOM_UUID or use --gamekit-join=IP:PORT".into());
    }
    Ok(candidates.first().and_then(|room| {
        room.endpoints.iter().find_map(|address| match address {
            SocketAddr::V4(address) => Some((room.info.id, *address)),
            _ => None,
        })
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn room(id: u8) -> lan::Room {
        lan::Room {
            info: lan::Advertisement {
                id: SessionId([id; 16]),
                name: "test".into(),
                port: 42018,
                players: 1,
                capacity: 4,
                protocol: transport::PROTOCOL_VERSION,
                compatibility: [1; 16],
            },
            endpoints: vec!["127.0.0.1:42018".parse().unwrap()],
        }
    }
    #[test]
    fn discovery_requires_unambiguous_compatible_available_room() {
        assert_eq!(select(&[], None).unwrap(), None);
        assert!(select(&[room(1), room(2)], None).is_err());
        assert_eq!(
            select(&[room(1), room(2)], Some(SessionId([2; 16])))
                .unwrap()
                .unwrap()
                .0,
            SessionId([2; 16])
        );
        let mut full = room(1);
        full.info.players = 4;
        assert!(select(&[full], None).unwrap().is_none());
        let mut newer = room(1);
        newer.info.protocol += 1;
        assert!(select(&[newer], None).unwrap().is_none());
    }
    #[test]
    fn automatic_mode_picks_the_same_room_everywhere() {
        assert_eq!(auto_select(&[], None), None);
        let picked = auto_select(&[room(3), room(2)], None).unwrap();
        assert_eq!(picked.0, SessionId([2; 16]));
        // A host doesn't join itself, and only gives way to smaller IDs
        // (checked by the caller).
        let picked = auto_select(&[room(2), room(3)], Some(SessionId([2; 16]))).unwrap();
        assert_eq!(picked.0, SessionId([3; 16]));
        let mut full = room(1);
        full.info.players = 4;
        assert_eq!(auto_select(&[full], None), None);
    }
    #[test]
    fn selected_room_identity_cannot_be_silently_replaced() {
        let mut state = State {
            selected: Some(SessionId([1; 16])),
            ..Default::default()
        };
        assert!(state
            .observe(
                &Event::Joined {
                    session: SessionId([2; 16]),
                    host: transport::PlayerId([1; 16])
                },
                false
            )
            .is_err());
        assert!(state
            .observe(
                &Event::Joined {
                    session: SessionId([1; 16]),
                    host: transport::PlayerId([1; 16])
                },
                false
            )
            .is_ok());
    }
}
