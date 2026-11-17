//! The app's rules for the map's requests ([`RequestRule`]), as GL JS's
//! `transformRequest` answers for each request, written as data, since a host
//! such as Exact can't run a function of the app's for each request. A request
//! takes the first rule whose `prefix` its URL starts with: the rule's `url`
//! replaces the prefix, its `query` parameters are added, and its `headers`
//! go with the request.

use mln_runtime::{Error, Runtime};
use mln_storage::resource::Kind;

use crate::model::{RequestRule, ResourceType};

/// Has `runtime` send its requests as `rules` say, from its next request on.
/// No rules clear the runtime's transforms.
pub(crate) fn apply(runtime: &Runtime, rules: &[RequestRule]) -> Result<(), Error> {
    if rules.iter().any(|rule| rule.url.is_some() || !rule.query.is_empty()) {
        let rules = rules.to_vec();
        let _set = runtime.set_resource_transform(move |kind, url, done| done(Ok(rewrite(&rules, kind, url))))?;
    } else {
        let _cleared = runtime.clear_resource_transform()?;
    }
    if rules.iter().any(|rule| !rule.headers.is_empty()) {
        let rules = rules.to_vec();
        let _set = runtime.set_http_header_transform(move |kind, url| headers(&rules, kind, url))?;
    } else {
        let _cleared = runtime.clear_http_header_transform()?;
    }
    Ok(())
}

/// Whether `rule` covers a request of `kind`.
fn takes(rule: &RequestRule, kind: Kind) -> bool {
    rule.resource_types.as_ref().is_none_or(|types| types.iter().any(|t| kind_of(*t) == kind))
}

fn kind_of(resource_type: ResourceType) -> Kind {
    match resource_type {
        ResourceType::Unknown => Kind::Unknown,
        ResourceType::Style => Kind::Style,
        ResourceType::Source => Kind::Source,
        ResourceType::Tile => Kind::Tile,
        ResourceType::Glyphs => Kind::Glyphs,
        ResourceType::SpriteImage => Kind::SpriteImage,
        ResourceType::SpriteJson => Kind::SpriteJSON,
        ResourceType::Image => Kind::Image,
    }
}

/// The URL a request of `kind` for `url` goes to.
fn rewrite(rules: &[RequestRule], kind: Kind, url: &str) -> String {
    let Some(rule) = rules.iter().find(|rule| takes(rule, kind) && url.starts_with(&rule.prefix)) else {
        return url.to_owned();
    };
    let mut url = match &rule.url {
        Some(to) => format!("{to}{}", &url[rule.prefix.len()..]),
        None => url.to_owned(),
    };
    if !rule.query.is_empty() {
        let fragment = url.find('#').map(|at| url.split_off(at)).unwrap_or_default();
        for (name, value) in &rule.query {
            url.push(if url.contains('?') { '&' } else { '?' });
            url.push_str(&format!("{}={}", encode(name), encode(value)));
        }
        url.push_str(&fragment);
    }
    url
}

/// The headers that go with a request of `kind` to `url`, the URL it goes to
/// after [`rewrite`]: those of the first rule whose requests go to URLs
/// starting as `url` does.
fn headers(rules: &[RequestRule], kind: Kind, url: &str) -> Vec<(String, String)> {
    rules
        .iter()
        .find(|rule| takes(rule, kind) && url.starts_with(rule.url.as_deref().unwrap_or(&rule.prefix)))
        .map(|rule| rule.headers.iter().map(|(name, value)| (name.clone(), value.clone())).collect())
        .unwrap_or_default()
}

/// `text` as a URL's query writes it, its bytes other than letters, digits
/// and `-._~` percent-encoded.
fn encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(json: &str) -> Vec<RequestRule> {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_request_goes_where_the_first_rule_for_it_says() {
        let rules = rules(
            r#"[{"prefix":"https://old.example/tiles/","resourceTypes":["Tile"],"url":"https://new.example/t/"},
                {"prefix":"https://old.example/","url":"https://cdn.example/"}]"#,
        );
        assert_eq!(
            rewrite(&rules, Kind::Tile, "https://old.example/tiles/3/4/5.pbf"),
            "https://new.example/t/3/4/5.pbf"
        );
        assert_eq!(
            rewrite(&rules, Kind::Style, "https://old.example/tiles/style.json"),
            "https://cdn.example/tiles/style.json"
        );
        assert_eq!(rewrite(&rules, Kind::Tile, "https://elsewhere.example/1.pbf"), "https://elsewhere.example/1.pbf");
    }

    #[test]
    fn query_parameters_are_added_before_the_fragment() {
        let rules = rules(r#"[{"prefix":"https://tiles.example/","query":{"key":"a b&c"}}]"#);
        assert_eq!(
            rewrite(&rules, Kind::Source, "https://tiles.example/v.json"),
            "https://tiles.example/v.json?key=a%20b%26c"
        );
        assert_eq!(
            rewrite(&rules, Kind::Source, "https://tiles.example/v.json?x=1#top"),
            "https://tiles.example/v.json?x=1&key=a%20b%26c#top"
        );
    }

    #[test]
    fn headers_go_with_the_requests_a_rule_sends() {
        let rules = rules(
            r#"[{"prefix":"https://old.example/","url":"https://tiles.example/","headers":{"Authorization":"Bearer t"}},
                {"prefix":"https://glyphs.example/","resourceTypes":["Glyphs"],"headers":{"X-Key":"k"}}]"#,
        );
        let bearer = vec![("Authorization".to_owned(), "Bearer t".to_owned())];
        assert_eq!(headers(&rules, Kind::Tile, "https://tiles.example/1/2/3.pbf"), bearer);
        assert_eq!(headers(&rules, Kind::Glyphs, "https://glyphs.example/Noto/0-255.pbf").len(), 1);
        assert_eq!(headers(&rules, Kind::Tile, "https://glyphs.example/Noto/0-255.pbf"), Vec::new());
        assert_eq!(headers(&rules, Kind::Tile, "https://old.example/1/2/3.pbf"), Vec::new());
    }
}
