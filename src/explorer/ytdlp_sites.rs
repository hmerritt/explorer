use gpui::http_client::Url;

mod supported_hosts {
    include!("ytdlp_supported_hosts.rs");
}

use supported_hosts::YTDLP_SUPPORTED_HOSTS;

pub(super) fn video_site_domain(url: &Url) -> Option<String> {
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }

    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    let is_video_url = if host == "youtu.be" {
        youtube_short_url_video_id(url).is_some_and(youtube_video_id_is_valid)
    } else if host == "youtube.com" || host.ends_with(".youtube.com") {
        youtube_url_has_video_id(url)
    } else {
        !url.path().trim_matches('/').is_empty()
            && YTDLP_SUPPORTED_HOSTS.binary_search(&host.as_str()).is_ok()
    };
    if !is_video_url {
        return None;
    }

    Some(psl::domain_str(&host).unwrap_or(&host).to_owned())
}

fn youtube_short_url_video_id(url: &Url) -> Option<&str> {
    let mut segments = url.path_segments()?;
    let video_id = segments.next()?;
    if segments.any(|segment| !segment.is_empty()) {
        return None;
    }
    Some(video_id)
}

fn youtube_url_has_video_id(url: &Url) -> bool {
    if url.path() == "/watch" {
        return url
            .query_pairs()
            .any(|(key, value)| key == "v" && youtube_video_id_is_valid(&value));
    }

    let Some(mut segments) = url.path_segments() else {
        return false;
    };
    let Some(route) = segments.next() else {
        return false;
    };
    if !matches!(route, "shorts" | "live" | "embed" | "v") {
        return false;
    }
    let Some(video_id) = segments.next() else {
        return false;
    };
    if segments.any(|segment| !segment.is_empty()) {
        return false;
    }
    youtube_video_id_is_valid(video_id)
}

fn youtube_video_id_is_valid(video_id: &str) -> bool {
    video_id.len() == 11
        && video_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_registrable_site_domains() {
        for (url, expected) in [
            ("https://player.vimeo.com/video/123", "vimeo.com"),
            ("https://www.bbc.co.uk/iplayer/episode/example", "bbc.co.uk"),
        ] {
            let url = Url::parse(url).expect("URL");
            assert_eq!(video_site_domain(&url).as_deref(), Some(expected));
        }
    }

    #[test]
    fn rejects_root_and_deceptive_hosts() {
        for url in [
            "https://vimeo.com/",
            "ftp://vimeo.com/123",
            "https://example.com/video/123",
            "https://notvimeo.com/123",
            "https://vimeo.com.example/123",
        ] {
            let url = Url::parse(url).expect("URL");
            assert_eq!(video_site_domain(&url), None, "unexpected match for {url}");
        }
    }

    #[test]
    fn generated_hosts_are_sorted_unique_and_production_like() {
        assert!(YTDLP_SUPPORTED_HOSTS.len() > 1_500);
        assert!(
            YTDLP_SUPPORTED_HOSTS
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        );
        assert!(YTDLP_SUPPORTED_HOSTS.iter().all(|host| {
            host != &"localhost"
                && !host.ends_with(".localhost")
                && !host.ends_with(".invalid")
                && !host.ends_with(".test")
                && !matches!(*host, "example.com" | "example.net" | "example.org")
        }));
    }
}
