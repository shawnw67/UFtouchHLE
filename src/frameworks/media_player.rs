/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Media Player framework.

mod media_entity;
mod media_item_collection;
mod media_library;
mod media_picker_controller;
mod media_playlist;
mod media_query;
pub mod mf_mail_compose_view_controller;
pub(crate) mod movie_decoder;
mod movie_player;
mod mp_volume_view;
mod music_player;

use crate::dyld::{ConstantExports, HostConstant};

#[allow(dead_code)]
pub(crate) fn mp4_duration(bytes: &[u8]) -> Option<f64> {
    movie_player::read_mp4_duration(&mut std::io::Cursor::new(bytes))
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_MPMoviePlayerLoadStateDidChangeNotification",
        HostConstant::NSString("MPMoviePlayerLoadStateDidChangeNotification"),
    ),
    (
        "_MPMoviePlayerPlaybackStateDidChangeNotification",
        HostConstant::NSString("MPMoviePlayerPlaybackStateDidChangeNotification"),
    ),
    (
        "_MPMediaItemPropertyAssetURL",
        HostConstant::NSString("assetURL"),
    ),
    (
        "_MPMediaLibraryDidChangeNotification",
        HostConstant::NSString("MPMediaLibraryDidChangeNotification"),
    ),
    (
        "_MPMediaPlaylistPropertyName",
        HostConstant::NSString("name"),
    ),
];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/MediaPlayer.framework/MediaPlayer",
    aliases: &[],
    class_exports: &[
        mp_volume_view::CLASSES,
        mf_mail_compose_view_controller::CLASSES,
        movie_player::CLASSES,
        music_player::CLASSES,
        media_entity::CLASSES,
        media_item_collection::CLASSES,
        media_library::CLASSES,
        media_picker_controller::CLASSES,
        media_playlist::CLASSES,
        media_query::CLASSES,
    ],
    constant_exports: &[movie_player::CONSTANTS, music_player::CONSTANTS, CONSTANTS],
    function_exports: &[],
};

#[derive(Default)]
pub struct State {
    movie_player: movie_player::State,
}

/// For use by `NSRunLoop`: check media players' status, send notifications if
/// necessary.
pub fn handle_players(env: &mut crate::Environment) {
    movie_player::handle_players(env);
}
