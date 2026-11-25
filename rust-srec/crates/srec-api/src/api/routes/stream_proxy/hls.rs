//! HLS manifest detection and URI rewriting for relayed playlists.

#[derive(Clone, Copy, Default)]
pub(super) struct RelayContext<'a> {
    pub(super) headers: Option<&'a str>,
    pub(super) token: Option<&'a str>,
    pub(super) source_url: Option<&'a str>,
    pub(super) web: bool,
    pub(super) playback: Option<ManagedRelay<'a>>,
}

/// The managed context a rewritten manifest's links stay bound to.
#[derive(Clone, Copy)]
pub(super) struct ManagedRelay<'a> {
    pub(super) handle: &'a str,
    pub(super) stream: usize,
    pub(super) data: &'a crate::api::playback_context::PlaybackData,
}

fn build_proxy_url(target: &url::Url, context: RelayContext<'_>) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("url", target.as_str());
    // Links to origins the context may not authenticate get a plain relay URL,
    // so the proxy can refuse the handle for every other host.
    if let Some(playback) = context.playback
        && playback
            .data
            .permits_authentication(target, playback.stream)
    {
        serializer
            .append_pair("playback_handle", playback.handle)
            .append_pair("stream", &playback.stream.to_string());
    }
    if let Some(headers) = context.headers {
        serializer.append_pair("headers", headers);
    }
    if let Some(token) = context.token.filter(|_| !context.web) {
        serializer.append_pair("token", token);
    }
    if let Some(source_url) = context.source_url {
        serializer.append_pair("source_url", source_url);
    }
    let path = if context.web {
        "/stream-proxy"
    } else {
        "/api/stream-proxy"
    };
    format!("{path}?{}", serializer.finish())
}

fn proxy_hls_uri(uri: &str, base_url: &url::Url, context: RelayContext<'_>) -> String {
    let Ok(target) = base_url.join(uri) else {
        return uri.to_string();
    };
    if !matches!(target.scheme(), "http" | "https") {
        return uri.to_string();
    }
    build_proxy_url(&target, context)
}

fn find_uri_attribute(line: &str, from: usize) -> Option<(usize, usize)> {
    let mut search_from = from;
    while let Some(relative_start) = line.get(search_from..)?.find("URI") {
        let attribute_start = search_from + relative_start;
        let preceding = line.get(..attribute_start)?.trim_end().chars().next_back();
        let server_uri = line
            .get(..attribute_start)
            .is_some_and(|prefix| prefix.ends_with("SERVER-"));
        if !matches!(preceding, Some(':') | Some(',')) && !server_uri {
            search_from = attribute_start + 3;
            continue;
        }

        let bytes = line.as_bytes();
        let mut cursor = attribute_start + 3;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'=') {
            search_from = attribute_start + 3;
            continue;
        }
        cursor += 1;
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') {
            search_from = attribute_start + 3;
            continue;
        }

        let value_start = cursor + 1;
        let relative_end = line.get(value_start..)?.find('"')?;
        return Some((value_start, value_start + relative_end));
    }
    None
}

fn rewrite_uri_attributes(line: &str, base_url: &url::Url, context: RelayContext<'_>) -> String {
    let mut output = String::with_capacity(line.len());
    let mut copied_until = 0;
    let mut search_from = 0;

    while let Some((value_start, value_end)) = find_uri_attribute(line, search_from) {
        output.push_str(&line[copied_until..value_start]);
        output.push_str(&proxy_hls_uri(
            &line[value_start..value_end],
            base_url,
            context,
        ));
        copied_until = value_end;
        search_from = value_end + 1;
    }
    output.push_str(&line[copied_until..]);
    output
}

fn rewrite_hls_line(line: &str, base_url: &url::Url, context: RelayContext<'_>) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return line.to_string();
    }
    if trimmed.starts_with('#') {
        return rewrite_uri_attributes(line, base_url, context);
    }

    let start = line.find(trimmed).unwrap_or(0);
    let end = start + trimmed.len();
    format!(
        "{}{}{}",
        &line[..start],
        proxy_hls_uri(trimmed, base_url, context),
        &line[end..]
    )
}

pub(super) fn rewrite_hls_manifest(
    manifest: &str,
    base_url: &url::Url,
    context: RelayContext<'_>,
) -> String {
    let mut output = String::with_capacity(manifest.len());
    for line in manifest.split_inclusive('\n') {
        let (content, newline) = if let Some(content) = line.strip_suffix("\r\n") {
            (content, "\r\n")
        } else if let Some(content) = line.strip_suffix('\n') {
            (content, "\n")
        } else {
            (line, "")
        };
        output.push_str(&rewrite_hls_line(content, base_url, context));
        output.push_str(newline);
    }
    output
}

pub(super) fn looks_like_hls_manifest(bytes: &[u8]) -> bool {
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    bytes.starts_with(b"#EXTM3U")
}

/// The essence of one media type: parameters dropped, trimmed and lowercased,
/// since media types are case-insensitive.
pub(super) fn media_type_essence(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

pub(super) fn is_hls_content_type(headers: &reqwest::header::HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    matches!(
        media_type_essence(content_type).as_str(),
        "application/vnd.apple.mpegurl"
            | "application/x-mpegurl"
            | "audio/mpegurl"
            | "audio/x-mpegurl"
    )
}
