/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! GameKit framework.
//!
//! Some features of this framework are only in iOS 4.1+, but some games (like
//! "Cut the Rope") may use it to check for game center availability with
//! a `respondsToSelector:` call to some objects of this framework.
//! Thus, we need to provide some stubs in order to not crash on that call.
//! Local networking is opt-in and does not use Apple Game Center services.

pub mod ad_banner_view;
pub mod fb_session;
pub mod gk_achievement;
pub mod gk_challenge_event_handler;
mod gk_leaderboard;
pub mod gk_leaderboard_view_controller;
mod gk_local_player;
mod gk_match;
mod gk_matchmaker;
mod gk_score;
mod gk_session;
mod gk_turn_based_event_handler;
mod local_network;
mod rooms;
mod route;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/GameKit.framework/GameKit",
    aliases: &[],
    class_exports: &[
        gk_turn_based_event_handler::CLASSES,
        gk_leaderboard_view_controller::CLASSES,
        gk_challenge_event_handler::CLASSES,
        gk_achievement::CLASSES,
        fb_session::CLASSES,
        ad_banner_view::CLASSES,
        gk_leaderboard::CLASSES,
        gk_local_player::CLASSES,
        gk_match::CLASSES,
        gk_matchmaker::CLASSES,
        gk_score::CLASSES,
        gk_session::CLASSES,
    ],
    constant_exports: &[ad_banner_view::CONSTANTS, gk_local_player::CONSTANTS],
    function_exports: &[],
};

#[derive(Default)]
pub struct State {
    gk_matchmaker: gk_matchmaker::State,
    pub challenge_event_handler: gk_challenge_event_handler::State,
    pub turn_based_event_handler: gk_turn_based_event_handler::State,
    local: local_network::State,
    route: route::State,
}

pub(crate) use local_network::pump;
pub(crate) use route::local_route;
