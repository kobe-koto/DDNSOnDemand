//! Run modes.

pub mod daemon;
pub mod oneshot;

use crate::config::{Config, Profile};

/// Iterate the enabled profiles that pass the optional name filter.
pub fn selected_profiles<'a>(
    config: &'a Config,
    filter: &'a [String],
) -> impl Iterator<Item = (&'a String, &'a Profile)> {
    config.profiles.iter().filter(move |(name, profile)| {
        profile.enabled && (filter.is_empty() || filter.contains(name))
    })
}
