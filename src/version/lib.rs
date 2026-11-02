// Allow the crate to have a non-snake-case name (touchHLE).
// This also allows items in the crate to have non-snake-case names.
#![allow(non_snake_case)]

/// Current version. See `build.rs` for how this is generated.
pub const VERSION: &str = include_str!(concat!(env!("OUT_DIR"), "/version.txt"));

// Environment variables set by GitHub Actions
pub const GITHUB_REPOSITORY: Option<&str> = option_env!("GITHUB_REPOSITORY");
pub const GITHUB_SERVER_URL: Option<&str> = option_env!("GITHUB_SERVER_URL");
pub const GITHUB_RUN_ID: Option<&str> = option_env!("GITHUB_RUN_ID");
pub const GITHUB_REF_NAME: Option<&str> = option_env!("GITHUB_REF_NAME");
pub const GITHUB_EVENT_NAME: Option<&str> = option_env!("GITHUB_EVENT_NAME");

/// Set when building locally to get the same branding as a GitHub Actions
/// build of this fork (see dev-scripts/build-android-uf.sh).
pub const TOUCHHLE_BRANDING: Option<&str> = option_env!("TOUCHHLE_BRANDING");

pub fn branding() -> &'static str {
    if let Some(branding) = TOUCHHLE_BRANDING {
        return branding;
    }
    if GITHUB_RUN_ID.is_none() {
        return "";
    }
    if GITHUB_EVENT_NAME == Some("create")
        && VERSION
            .strip_prefix('v')
            .is_some_and(|v| v == env!("CARGO_PKG_VERSION"))
        && GITHUB_REF_NAME.unwrap() == VERSION
    {
        return "";
    }
    if (GITHUB_REPOSITORY, GITHUB_REF_NAME) == (Some("touchHLE/touchHLE"), Some("trunk")) {
        "PREVIEW"
    } else {
        // Short for "unofficial". Note that the Android package name for
        // this is org.podroid.android, see android/app/build.gradle.kts
        // and src/paths.rs.
        "UF"
    }
}
