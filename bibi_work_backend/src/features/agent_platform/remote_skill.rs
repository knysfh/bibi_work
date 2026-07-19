use std::{
    io::{Cursor, Read},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::Path,
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, Url, header, redirect::Policy};
use serde::Deserialize;
use zip::ZipArchive;

use crate::features::core::errors::AppError;

const GITHUB_API_BASE: &str = "https://api.github.com";
const MAX_REMOTE_REDIRECTS: usize = 3;
const MAX_REMOTE_ATTEMPTS: usize = 3;
const MAX_REMOTE_SKILLS: usize = 64;
const MAX_REMOTE_TREE_ENTRIES: usize = 10_000;
const MAX_ARCHIVE_ENTRIES: usize = 2_048;
pub const MAX_SKILL_FILE_BYTES: usize = 1024 * 1024;
pub const MAX_SKILL_TOTAL_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RemoteSkillDocument {
    pub source_uri: String,
    pub source_name: String,
    pub content: String,
    pub total_bytes: u64,
    pub commit_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GitHubSkillSource {
    owner: String,
    repository: String,
    reference: Option<String>,
    path_prefix: Option<String>,
}

#[derive(Deserialize)]
struct RepositoryResponse {
    default_branch: String,
}

#[derive(Deserialize)]
struct CommitResponse {
    sha: String,
}

#[derive(Deserialize)]
struct TreeResponse {
    tree: Vec<TreeEntry>,
    truncated: bool,
}

#[derive(Deserialize)]
struct TreeEntry {
    path: String,
    sha: String,
    #[serde(rename = "type")]
    entry_type: String,
    size: Option<u64>,
}

pub fn is_remote_skill_url(value: &str) -> bool {
    Url::parse(value.trim()).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

pub fn safe_remote_source_label(value: &str) -> String {
    let Ok(mut url) = Url::parse(value.trim()) else {
        return "remote-skill".to_string();
    };
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

/// Load a skill from a generic remote source.
///
/// GitHub repository/tree URLs use the commit-pinned GitHub adapter. Every
/// other provider uses the portable contract: an HTTPS URL that returns either
/// one UTF-8 SKILL.md document or a ZIP archive containing one or more skills.
pub async fn fetch_remote_skill_documents(
    source_url: &str,
) -> Result<Vec<RemoteSkillDocument>, AppError> {
    if parse_github_skill_source(source_url).is_ok() {
        return fetch_github_skill_documents(source_url).await;
    }
    fetch_direct_skill_documents(source_url).await
}

pub async fn fetch_github_skill_documents(
    source_url: &str,
) -> Result<Vec<RemoteSkillDocument>, AppError> {
    let source = parse_github_skill_source(source_url)?;
    let client = remote_client()?;
    let repository_url = format!(
        "{GITHUB_API_BASE}/repos/{}/{}",
        source.owner, source.repository
    );
    let repository: RepositoryResponse = github_json(&client, &repository_url).await?;
    let reference = source
        .reference
        .as_deref()
        .unwrap_or(&repository.default_branch);
    let commit_url = format!("{repository_url}/commits/{reference}");
    let commit: CommitResponse = github_json(&client, &commit_url).await?;
    let tree_url = format!("{repository_url}/git/trees/{}?recursive=1", commit.sha);
    let tree: TreeResponse = github_json(&client, &tree_url).await?;
    if tree.truncated || tree.tree.len() > MAX_REMOTE_TREE_ENTRIES {
        return Err(code_error("SKILL_IMPORT_REMOTE_TREE_TOO_LARGE"));
    }

    let prefix = source.path_prefix.as_deref().map(normalize_repository_path);
    let mut skill_paths = tree
        .tree
        .into_iter()
        .filter(|entry| entry.entry_type == "blob")
        .filter(|entry| is_skill_markdown_path(&entry.path))
        .filter(|entry| {
            prefix.as_deref().is_none_or(|prefix| {
                entry.path == prefix || entry.path.starts_with(&format!("{prefix}/"))
            })
        })
        .collect::<Vec<_>>();
    skill_paths.sort_by(|left, right| left.path.cmp(&right.path));
    validate_skill_count(skill_paths.len())?;

    let mut documents = Vec::with_capacity(skill_paths.len());
    let mut total_bytes = 0_usize;
    for entry in skill_paths {
        if entry
            .size
            .is_some_and(|size| size as usize > MAX_SKILL_FILE_BYTES)
        {
            return Err(code_error("SKILL_IMPORT_FILE_TOO_LARGE"));
        }
        let blob_url = format!("{repository_url}/git/blobs/{}", entry.sha);
        let response = github_request(&client, &blob_url)
            .header(header::ACCEPT, "application/vnd.github.raw+json")
            .send()
            .await
            .map_err(|_| code_error("SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED"))?;
        if !response.status().is_success() {
            return Err(code_error("SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED"));
        }
        let bytes = read_bounded_response(response, MAX_SKILL_FILE_BYTES).await?;
        total_bytes = total_bytes.saturating_add(bytes.len());
        if total_bytes > MAX_SKILL_TOTAL_BYTES {
            return Err(code_error("SKILL_IMPORT_TOTAL_TOO_LARGE"));
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| code_error("SKILL_IMPORT_REMOTE_CONTENT_NOT_UTF8"))?;
        let source_name = skill_source_name(&entry.path, &source.repository);
        documents.push(RemoteSkillDocument {
            source_uri: format!(
                "github://{}/{}@{}/{}",
                source.owner, source.repository, commit.sha, entry.path
            ),
            source_name,
            total_bytes: content.len() as u64,
            content,
            commit_sha: commit.sha.clone(),
        });
    }
    Ok(documents)
}

pub fn parse_zip_skill_documents(
    bytes: &[u8],
    source_label: &str,
) -> Result<Vec<RemoteSkillDocument>, AppError> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes)).map_err(|_| code_error("SKILL_IMPORT_INVALID_ZIP"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(code_error("SKILL_IMPORT_ARCHIVE_ENTRY_LIMIT"));
    }

    let mut total_bytes = 0_usize;
    let mut documents = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| code_error("SKILL_IMPORT_INVALID_ZIP"))?;
        let path = entry
            .enclosed_name()
            .ok_or_else(|| code_error("SKILL_IMPORT_INVALID_ZIP"))?
            .to_path_buf();
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(code_error("SKILL_IMPORT_SYMLINK_ENTRY"));
        }
        if entry.is_dir() {
            continue;
        }
        let size = usize::try_from(entry.size())
            .map_err(|_| code_error("SKILL_IMPORT_TOTAL_TOO_LARGE"))?;
        if size > MAX_SKILL_FILE_BYTES {
            return Err(code_error("SKILL_IMPORT_FILE_TOO_LARGE"));
        }
        total_bytes = total_bytes.saturating_add(size);
        if total_bytes > MAX_SKILL_TOTAL_BYTES {
            return Err(code_error("SKILL_IMPORT_TOTAL_TOO_LARGE"));
        }
        if !is_skill_markdown_path(&path.to_string_lossy()) {
            continue;
        }
        if documents.len() >= MAX_REMOTE_SKILLS {
            return Err(code_error("SKILL_IMPORT_REMOTE_SKILL_COUNT_EXCEEDED"));
        }
        let mut content = Vec::with_capacity(size);
        entry
            .read_to_end(&mut content)
            .map_err(|_| code_error("SKILL_IMPORT_INVALID_ZIP"))?;
        if content.len() > MAX_SKILL_FILE_BYTES {
            return Err(code_error("SKILL_IMPORT_FILE_TOO_LARGE"));
        }
        let content = String::from_utf8(content)
            .map_err(|_| code_error("SKILL_IMPORT_REMOTE_CONTENT_NOT_UTF8"))?;
        let path_text = path.to_string_lossy();
        documents.push(RemoteSkillDocument {
            source_uri: format!("zip://{}!/{}", source_label, path_text),
            source_name: skill_source_name(&path_text, source_label),
            content,
            total_bytes: total_bytes as u64,
            commit_sha: String::new(),
        });
    }
    validate_skill_count(documents.len())?;
    Ok(documents)
}

async fn fetch_direct_skill_documents(
    source_url: &str,
) -> Result<Vec<RemoteSkillDocument>, AppError> {
    let client = remote_client()?;
    let mut url = validate_public_https_url(source_url).await?;
    let mut redirects = 0_usize;
    let response = loop {
        let response = send_direct_request(&client, &url).await?;
        if response.status().is_redirection() {
            if redirects >= MAX_REMOTE_REDIRECTS {
                return Err(code_error("SKILL_IMPORT_REMOTE_REDIRECT_LIMIT"));
            }
            let location = response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| code_error("SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED"))?;
            let next = url
                .join(location)
                .map_err(|_| code_error("SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED"))?;
            url = validate_public_https_url(next.as_str()).await?;
            redirects += 1;
            continue;
        }
        break response;
    };
    if response.status() != StatusCode::OK {
        return Err(code_error("SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED"));
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if content_type.contains("text/html") {
        return Err(code_error("SKILL_IMPORT_UNSUPPORTED_REMOTE_CONTENT"));
    }
    let bytes = read_bounded_response(response, MAX_SKILL_TOTAL_BYTES).await?;
    let source_label = safe_remote_source_label(url.as_str());
    if bytes.starts_with(b"PK\x03\x04")
        || content_type.contains("zip")
        || url.path().to_ascii_lowercase().ends_with(".zip")
    {
        return parse_zip_skill_documents(&bytes, &source_label);
    }
    if bytes.len() > MAX_SKILL_FILE_BYTES {
        return Err(code_error("SKILL_IMPORT_FILE_TOO_LARGE"));
    }
    let content =
        String::from_utf8(bytes).map_err(|_| code_error("SKILL_IMPORT_REMOTE_CONTENT_NOT_UTF8"))?;
    let source_name = direct_skill_source_name(&url);
    Ok(vec![RemoteSkillDocument {
        source_uri: source_label,
        source_name,
        total_bytes: content.len() as u64,
        content,
        commit_sha: String::new(),
    }])
}

async fn validate_public_https_url(value: &str) -> Result<Url, AppError> {
    let url =
        Url::parse(value.trim()).map_err(|_| code_error("SKILL_IMPORT_REMOTE_URL_INVALID"))?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(code_error("SKILL_IMPORT_REMOTE_URL_INVALID"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| code_error("SKILL_IMPORT_REMOTE_URL_INVALID"))?;
    let lower_host = host.to_ascii_lowercase();
    if lower_host == "localhost"
        || lower_host.ends_with(".localhost")
        || lower_host.ends_with(".local")
        || lower_host.ends_with(".internal")
    {
        return Err(code_error("SKILL_IMPORT_REMOTE_ADDRESS_BLOCKED"));
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| code_error("SKILL_IMPORT_REMOTE_LOOKUP_FAILED"))?
        .map(|socket| socket.ip())
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.into_iter().any(is_non_public_ip) {
        return Err(code_error("SKILL_IMPORT_REMOTE_ADDRESS_BLOCKED"));
    }
    Ok(url)
}

fn is_non_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_non_public_ipv4(ip),
        IpAddr::V6(ip) => is_non_public_ipv6(ip),
    }
}

fn is_non_public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    a == 0
        || a == 10
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && matches!(b, 18 | 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
}

fn is_non_public_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || ip.to_ipv4_mapped().is_some_and(is_non_public_ipv4)
}

async fn read_bounded_response(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, AppError> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(code_error("SKILL_IMPORT_TOTAL_TOO_LARGE"));
    }
    let mut output = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| code_error("SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED"))?;
        if output.len().saturating_add(chunk.len()) > limit {
            return Err(code_error("SKILL_IMPORT_TOTAL_TOO_LARGE"));
        }
        output.extend_from_slice(&chunk);
    }
    Ok(output)
}

fn remote_client() -> Result<Client, AppError> {
    Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none())
        .build()
        .map_err(|_| code_error("SKILL_IMPORT_REMOTE_FAILED"))
}

async fn send_direct_request(client: &Client, url: &Url) -> Result<reqwest::Response, AppError> {
    for attempt in 0..MAX_REMOTE_ATTEMPTS {
        let response = client
            .get(url.clone())
            .header(header::USER_AGENT, "bibi-work-skill-importer")
            .send()
            .await;
        match response {
            Ok(response)
                if (response.status().is_server_error()
                    || response.status() == StatusCode::TOO_MANY_REQUESTS)
                    && attempt + 1 < MAX_REMOTE_ATTEMPTS => {}
            Ok(response) => return Ok(response),
            Err(_) if attempt + 1 < MAX_REMOTE_ATTEMPTS => {}
            Err(_) => return Err(code_error("SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED")),
        }
        let delay_ms = 150_u64.saturating_mul(1_u64 << attempt);
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    Err(code_error("SKILL_IMPORT_REMOTE_DOWNLOAD_FAILED"))
}

fn validate_skill_count(count: usize) -> Result<(), AppError> {
    if count == 0 {
        return Err(code_error("SKILL_IMPORT_NO_SKILL_FOUND"));
    }
    if count > MAX_REMOTE_SKILLS {
        return Err(code_error("SKILL_IMPORT_REMOTE_SKILL_COUNT_EXCEEDED"));
    }
    Ok(())
}

fn is_skill_markdown_path(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("SKILL.md"))
}

fn skill_source_name(path: &str, fallback: &str) -> String {
    Path::new(path)
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn direct_skill_source_name(url: &Url) -> String {
    let path = Path::new(url.path());
    if is_skill_markdown_path(url.path()) {
        return path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("remote-skill")
            .to_string();
    }
    path.file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("remote-skill")
        .to_string()
}

async fn github_json<T: for<'de> Deserialize<'de>>(
    client: &Client,
    url: &str,
) -> Result<T, AppError> {
    let response = github_request(client, url)
        .send()
        .await
        .map_err(|_| code_error("SKILL_IMPORT_REMOTE_LOOKUP_FAILED"))?;
    if !response.status().is_success() {
        return Err(code_error("SKILL_IMPORT_REMOTE_LOOKUP_FAILED"));
    }
    response
        .json::<T>()
        .await
        .map_err(|_| code_error("SKILL_IMPORT_REMOTE_LOOKUP_FAILED"))
}

fn github_request(client: &Client, url: &str) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header(header::USER_AGENT, "bibi-work-skill-importer")
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

fn parse_github_skill_source(source_url: &str) -> Result<GitHubSkillSource, AppError> {
    let raw = source_url.trim();
    let raw_lower = raw.to_ascii_lowercase();
    if raw.contains('\\')
        || raw_lower.contains("/../")
        || raw_lower.ends_with("/..")
        || raw_lower.contains("/./")
        || raw_lower.contains("%2e")
    {
        return Err(code_error("SKILL_IMPORT_REMOTE_URL_INVALID"));
    }
    let url = Url::parse(raw).map_err(|_| code_error("SKILL_IMPORT_REMOTE_URL_INVALID"))?;
    if url.scheme() != "https" || url.host_str() != Some("github.com") {
        return Err(code_error("SKILL_IMPORT_REMOTE_URL_INVALID"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(code_error("SKILL_IMPORT_REMOTE_URL_INVALID"));
    }
    let segments = url
        .path_segments()
        .ok_or_else(|| code_error("SKILL_IMPORT_REMOTE_URL_INVALID"))?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() < 2 {
        return Err(code_error("SKILL_IMPORT_REMOTE_URL_INVALID"));
    }
    validate_github_identifier(segments[0])?;
    let repository = segments[1].strip_suffix(".git").unwrap_or(segments[1]);
    validate_github_identifier(repository)?;
    let (reference, path_prefix) = match segments.get(2).copied() {
        None => (None, None),
        Some("tree" | "blob") if segments.len() >= 4 => (
            Some(segments[3].to_string()),
            (segments.len() > 4).then(|| segments[4..].join("/")),
        ),
        _ => return Err(code_error("SKILL_IMPORT_REMOTE_URL_INVALID")),
    };
    if let Some(path) = path_prefix.as_deref() {
        validate_repository_path(path)?;
    }
    Ok(GitHubSkillSource {
        owner: segments[0].to_string(),
        repository: repository.to_string(),
        reference,
        path_prefix,
    })
}

fn validate_github_identifier(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 100
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_alphanumeric() && !matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(code_error("SKILL_IMPORT_REMOTE_URL_INVALID"));
    }
    Ok(())
}

fn normalize_repository_path(value: &str) -> String {
    value.trim_matches('/').to_string()
}

fn validate_repository_path(value: &str) -> Result<(), AppError> {
    if value.len() > 512
        || value.starts_with('/')
        || value.split('/').any(|segment| {
            segment.is_empty() || segment == "." || segment == ".." || segment.contains('\0')
        })
    {
        return Err(code_error("SKILL_IMPORT_REMOTE_URL_INVALID"));
    }
    Ok(())
}

fn code_error(code: &str) -> AppError {
    AppError::InvalidInput(code.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn parses_bounded_github_repository_and_tree_urls() {
        assert_eq!(
            parse_github_skill_source("https://github.com/agentskills/agentskills").unwrap(),
            GitHubSkillSource {
                owner: "agentskills".to_string(),
                repository: "agentskills".to_string(),
                reference: None,
                path_prefix: None,
            }
        );
        assert_eq!(
            parse_github_skill_source("https://github.com/anthropics/skills/tree/main/skills/pdf")
                .unwrap(),
            GitHubSkillSource {
                owner: "anthropics".to_string(),
                repository: "skills".to_string(),
                reference: Some("main".to_string()),
                path_prefix: Some("skills/pdf".to_string()),
            }
        );
    }

    #[test]
    fn recognizes_generic_remote_urls_and_sanitizes_signed_urls() {
        assert!(is_remote_skill_url("https://gitlab.example/skill.zip"));
        assert!(is_remote_skill_url("http://example.test/SKILL.md"));
        assert!(!is_remote_skill_url("/tmp/SKILL.md"));
        assert_eq!(
            safe_remote_source_label("https://cdn.example/skill.zip?token=secret#download"),
            "https://cdn.example/skill.zip"
        );
    }

    #[test]
    fn parses_zip_packages_without_extracting_to_disk() {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut output);
            writer
                .start_file("skills/weather/SKILL.md", SimpleFileOptions::default())
                .unwrap();
            writer
                .write_all(b"---\nname: weather\ndescription: Weather lookup\n---\n")
                .unwrap();
            writer
                .start_file(
                    "skills/weather/references/api.md",
                    SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(b"reference").unwrap();
            writer.finish().unwrap();
        }
        let documents = parse_zip_skill_documents(&output.into_inner(), "upload.zip").unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].source_name, "weather");
        assert!(documents[0].source_uri.contains("skills/weather/SKILL.md"));
    }

    #[test]
    fn rejects_zip_slip_entries() {
        let mut output = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut output);
            writer
                .start_file("../SKILL.md", SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"# bad").unwrap();
            writer.finish().unwrap();
        }
        let error = parse_zip_skill_documents(&output.into_inner(), "bad.zip").unwrap_err();
        assert!(error.to_string().contains("SKILL_IMPORT_INVALID_ZIP"));
    }

    #[test]
    fn blocks_private_network_ranges() {
        assert!(is_non_public_ip("127.0.0.1".parse().unwrap()));
        assert!(is_non_public_ip("10.0.0.1".parse().unwrap()));
        assert!(is_non_public_ip("::1".parse().unwrap()));
        assert!(!is_non_public_ip("8.8.8.8".parse().unwrap()));
    }

    #[tokio::test]
    #[ignore = "requires GitHub network access"]
    async fn downloads_a_pinned_example_skill_document() -> Result<(), Box<dyn std::error::Error>> {
        let documents = fetch_github_skill_documents(
            "https://github.com/anthropics/skills/tree/main/skills/mcp-builder",
        )
        .await?;
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].source_name, "mcp-builder");
        assert_eq!(documents[0].commit_sha.len(), 40);
        Ok(())
    }
}
