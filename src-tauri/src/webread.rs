//! read_url: 공개 웹 페이지의 글만 읽어 온다.
//! 내 PC에서 도는 앱이라 내부망(공유기 관리 화면, 사내 서버, 127.0.0.1의 다른 프로그램)을 건드리면 위험하다.
//! 그래서 주소를 직접 DNS로 풀어 모든 IP가 공개 주소인지 확인하고, 확인한 IP로만 연결한다(DNS 바꿔치기 방지).
//! 리디렉션은 자동으로 따라가지 않고 매번 다시 검사한다. 크기 1.5MB, 시간 10초, 글 12,000자 제한.

use anyhow::{bail, Context, Result};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

const MAX_BYTES: usize = 1_500_000;
const MAX_CHARS: usize = 12_000;

pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let o = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_unspecified()
                || v.is_multicast()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0))
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let s = v.segments();
            !(v.is_loopback()
                || v.is_unspecified()
                || v.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00
                || (s[0] & 0xffc0) == 0xfe80
                || (s[0] == 0x64 && s[1] == 0xff9b))
        }
    }
}

/// 주소 모양 검사 + DNS로 풀어 공개 IP인지 확인. 연결할 (호스트, 소켓 주소)를 돌려준다.
async fn check(url: &reqwest::Url) -> Result<(String, SocketAddr)> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("http·https 주소만 읽을 수 있습니다");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("계정 정보가 든 주소는 읽지 않습니다");
    }
    if let Some(p) = url.port() {
        if ![80, 443, 8080, 8443].contains(&p) {
            bail!("일반적이지 않은 포트는 읽지 않습니다");
        }
    }
    let host = url
        .host_str()
        .context("주소에 호스트가 없습니다")?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    let local_name = host == "localhost"
        || [".localhost", ".local", ".internal", ".home.arpa", ".lan"]
            .iter()
            .any(|s| host.ends_with(s))
        || (!host.contains('.') && !host.contains(':'));
    if local_name {
        bail!("내부망·로컬 주소는 읽을 수 없습니다");
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .context("주소를 찾지 못했습니다")?
        .collect();
    if addrs.is_empty() || addrs.iter().any(|a| !is_public_ip(a.ip())) {
        bail!("내부망·로컬 주소는 읽을 수 없습니다");
    }
    Ok((host, addrs[0]))
}

pub async fn read(_shared: &reqwest::Client, raw: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(raw.trim()).context("주소 형식이 올바르지 않습니다")?;
    url.set_fragment(None);
    for _ in 0..4 {
        let (host, addr) = check(&url).await?;
        // 검사한 IP로만 연결하고, 리디렉션은 직접 처리한다
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (compatible; JARVIS-reader/1.0)")
            .resolve(&host, addr)
            .build()?;
        let mut resp = client
            .get(url.clone())
            .header("accept", "text/html,text/plain;q=0.9,*/*;q=0.1")
            .send()
            .await
            .context("페이지에 연결하지 못했습니다")?;
        let status = resp.status();
        if status.is_redirection() {
            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .context("리디렉션 주소가 없습니다")?;
            url = url.join(loc).context("리디렉션 주소가 이상합니다")?;
            url.set_fragment(None);
            continue;
        }
        if !status.is_success() {
            bail!("페이지가 {status} 오류를 돌려줬습니다");
        }
        let ctype = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ![
            "text/html",
            "text/plain",
            "application/xhtml",
            "application/json",
            "text/markdown",
        ]
        .iter()
        .any(|t| ctype.contains(t))
        {
            bail!("글로 된 페이지만 읽을 수 있습니다");
        }
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = resp.chunk().await.context("페이지를 읽다가 끊겼습니다")?
        {
            buf.extend_from_slice(&chunk);
            if buf.len() > MAX_BYTES {
                buf.truncate(MAX_BYTES);
                break;
            }
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let (title, body) = if ctype.contains("html") {
            html_to_text(&text)
        } else {
            (String::new(), text.trim().to_string())
        };
        let truncated = body.chars().count() > MAX_CHARS;
        let body: String = body.chars().take(MAX_CHARS).collect();
        return Ok(
            json!({ "url": url.as_str(), "title": title, "text": body, "truncated": truncated })
                .to_string(),
        );
    }
    bail!("리디렉션이 너무 많습니다")
}

/// 대소문자 무시로 needle을 찾는다 (ASCII 태그 이름용)
fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || from >= h.len() {
        return None;
    }
    (from..=h.len().saturating_sub(n.len())).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

pub fn html_to_text(html: &str) -> (String, String) {
    let title = find_ci(html, "<title", 0)
        .and_then(|i| html[i..].find('>').map(|j| i + j + 1))
        .and_then(|s| find_ci(html, "</title>", s).map(|e| html[s..e].to_string()))
        .map(|t| decode(&t).split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    // 통째로 버릴 블록
    let mut s = html.to_string();
    for tag in [
        "head", "script", "style", "noscript", "template", "svg", "iframe", "nav", "footer",
        "header", "form",
    ] {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        let mut from = 0;
        while let Some(i) = find_ci(&s, &open, from) {
            let after = s.as_bytes().get(i + open.len()).copied().unwrap_or(b'>');
            if !(after == b'>' || after.is_ascii_whitespace() || after == b'/') {
                from = i + open.len();
                continue;
            }
            match find_ci(&s, &close, i) {
                Some(j) => s.replace_range(i..j + close.len(), " "),
                None => {
                    s.truncate(i);
                    break;
                }
            }
            from = i;
        }
    }
    // 태그 지우기 (블록 태그는 줄바꿈으로)
    let mut out = String::with_capacity(s.len() / 2);
    let mut rest = s.as_str();
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        match rest[i..].find('>') {
            Some(j) => {
                let tag = rest[i + 1..i + j]
                    .trim_start_matches('/')
                    .to_ascii_lowercase();
                let name: String = tag
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                if [
                    "br", "p", "div", "li", "h1", "h2", "h3", "h4", "h5", "h6", "tr", "section",
                    "article",
                ]
                .contains(&name.as_str())
                {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
                rest = &rest[i + j + 1..];
            }
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let text = decode(&out);
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    (title, lines.join("\n"))
}

fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let end = tail.find(';').filter(|&e| e <= 10);
        if let Some(e) = end {
            let ent = &tail[1..e];
            let ch = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" | "#39" => Some('\''),
                "nbsp" => Some(' '),
                "middot" => Some('·'),
                "hellip" => Some('…'),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            if let Some(c) = ch {
                out.push(c);
                rest = &tail[e + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_ips() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.1",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn strips_html() {
        let (t, b) = html_to_text("<html><head><title>제목 &amp; 부제</title><style>.a{}</style></head><body><nav>메뉴</nav><p>첫 줄&nbsp;&#xAC00;</p><script>steal()</script><p>둘째</p></body></html>");
        assert_eq!(t, "제목 & 부제");
        assert!(!b.contains("steal") && !b.contains("메뉴") && !b.contains(".a{"));
        assert_eq!(b, "첫 줄 가\n둘째");
    }
}
