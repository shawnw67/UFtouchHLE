/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Resolve hierarchical URL references without decoding escaped separators.

use super::split_url;

fn remove_dots(mut input: &str) -> String {
    let mut output = String::new();
    while !input.is_empty() {
        if let Some(rest) = input
            .strip_prefix("../")
            .or_else(|| input.strip_prefix("./"))
        {
            input = rest;
        } else if input.starts_with("/./") {
            input = &input[2..];
        } else if input == "/." {
            input = "/";
        } else if input.starts_with("/../") || input == "/.." {
            input = if input == "/.." { "/" } else { &input[3..] };
            output.truncate(output.rfind('/').unwrap_or(0));
        } else if input == "." || input == ".." {
            input = "";
        } else {
            let skip = usize::from(input.starts_with('/'));
            let end = input[skip..]
                .find('/')
                .map_or(input.len(), |index| index + skip);
            output.push_str(&input[..end]);
            input = &input[end..];
        }
    }
    output
}

pub(super) fn resolve(base: &str, reference: &str) -> Option<String> {
    let reference_parts = split_url(reference);
    if reference_parts.scheme.is_some() {
        return Some(reference.to_string());
    }
    let base = split_url(base);
    let scheme = base.scheme?;
    if !base.decomposable {
        return None;
    }
    let (authority, path, query) = if reference_parts.authority.is_some() {
        (
            reference_parts.authority,
            remove_dots(reference_parts.path),
            reference_parts.query,
        )
    } else if reference_parts.path.is_empty() {
        (
            base.authority,
            base.path.to_string(),
            reference_parts.query.or(base.query),
        )
    } else {
        let path = if reference_parts.path.starts_with('/') {
            reference_parts.path.to_string()
        } else if base.authority.is_some() && base.path.is_empty() {
            format!("/{}", reference_parts.path)
        } else {
            let prefix = base.path.rfind('/').map_or("", |i| &base.path[..=i]);
            format!("{prefix}{}", reference_parts.path)
        };
        (base.authority, remove_dots(&path), reference_parts.query)
    };
    let mut result = format!("{scheme}:");
    if let Some(authority) = authority {
        result.push_str("//");
        result.push_str(authority);
    }
    result.push_str(&path);
    if let Some(query) = query {
        result.push('?');
        result.push_str(query);
    }
    if let Some(fragment) = reference_parts.fragment {
        result.push('#');
        result.push_str(fragment);
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_paths_queries_and_fragments_independently() {
        let base = "http://a/b/c/d;p?q";
        for (reference, expected) in [
            ("g", "http://a/b/c/g"),
            ("./g", "http://a/b/c/g"),
            ("../g", "http://a/b/g"),
            ("../../g", "http://a/g"),
            ("../../../g", "http://a/g"),
            ("//g", "http://g"),
            ("?y", "http://a/b/c/d;p?y"),
            ("#s", "http://a/b/c/d;p?q#s"),
            ("", "http://a/b/c/d;p?q"),
            ("g/../h", "http://a/b/c/h"),
            ("g?y/../x", "http://a/b/c/g?y/../x"),
            ("%2e%2e/g", "http://a/b/c/%2e%2e/g"),
            ("g//h", "http://a/b/c/g//h"),
            ("/../g", "http://a/g"),
        ] {
            assert_eq!(resolve(base, reference).as_deref(), Some(expected));
        }
        assert_eq!(
            resolve("file:///Applications/Game.app/", "data/a%20b").unwrap(),
            "file:///Applications/Game.app/data/a%20b"
        );
    }
}
