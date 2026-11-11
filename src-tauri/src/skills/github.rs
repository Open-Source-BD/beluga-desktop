//! GitHub links for adding skills: which links are accepted, and pinning a
//! link to one commit through the GitHub API. Requests carry the app's name
//! as `User-Agent` and never any credential.

use super::library::valid_rel_path;
use crate::brand::APP_NAME;

/// A parsed `https://github.com/{owner}/{repo}[/tree/{ref}[/{sub/path}]]` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubLink {
    pub owner: String,
    pub repo: String,
    /// `None` means the repository's default branch.
    pub git_ref: Option<String>,
    /// A folder inside the repository, with `/` separators.
    pub sub_path: Option<String>,
}

impl GithubLink {
    /// `owner/repo`, as the archive download takes it.
    pub fn repo_path(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// Largest API response read; a commit SHA or a repository's metadata.
const MAX_API_BYTES: usize = 256 * 1024;

/// An owner, repository or ref: `[A-Za-z0-9._-]`, not `.` or `..`.
fn safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The link's parts, or `None` for anything that is not an accepted GitHub
/// link: `https://github.com/{owner}/{repo}`, optionally followed by
/// `/tree/{ref}` and a sub-path, with an optional trailing `/` or `.git`.
/// Other hosts, `http`, user info, ports, queries and fragments are refused.
pub fn parse_github_url(url: &str) -> Option<GithubLink> {
    let rest = url.trim().strip_prefix("https://github.com/")?;
    if rest
        .chars()
        .any(|c| matches!(c, '?' | '#' | '\\') || c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    let segs: Vec<&str> = rest.split('/').collect();
    let owner = *segs.first()?;
    let repo = *segs.get(1)?;
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if !safe_segment(owner) {
        return None;
    }
    let link = |repo: &str, git_ref: Option<&str>, sub: Option<String>| {
        safe_segment(repo).then(|| GithubLink {
            owner: owner.to_string(),
            repo: repo.to_string(),
            git_ref: git_ref.map(str::to_string),
            sub_path: sub,
        })
    };
    match segs.len() {
        2 => link(repo, None, None),
        n if n >= 4 && segs[2] == "tree" && safe_segment(segs[3]) => {
            let sub = segs[4..].join("/");
            if sub.is_empty() {
                link(repo, Some(segs[3]), None)
            } else if valid_rel_path(&sub, false) {
                link(repo, Some(segs[3]), Some(sub))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// GETs `url` with the app's name as `User-Agent` and no credential, reading
/// at most `MAX_API_BYTES`. The error says what went wrong, for logs and
/// tests; the user sees one fixed message.
async fn get_small(http: &reqwest::Client, url: &str, accept: &str) -> Result<Vec<u8>, String> {
    use futures_util::StreamExt;
    let resp = http
        .get(url)
        .header(reqwest::header::USER_AGENT, APP_NAME)
        .header(reqwest::header::ACCEPT, accept)
        .send()
        .await
        .map_err(|_| "could not reach GitHub".to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub answered {}", resp.status().as_u16()));
    }
    let mut body = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.map_err(|_| "the reply was interrupted".to_string())?);
        if body.len() > MAX_API_BYTES {
            return Err("the reply is too large".to_string());
        }
    }
    Ok(body)
}

/// The commit SHA the link points at, resolving the default branch first
/// when the link names no ref. `Err` describes the failure.
pub async fn resolve_commit(
    http: &reqwest::Client,
    api_base: &str,
    link: &GithubLink,
) -> Result<String, String> {
    let base = format!(
        "{}/repos/{}/{}",
        api_base.trim_end_matches('/'),
        link.owner,
        link.repo
    );
    let git_ref = match &link.git_ref {
        Some(r) => r.clone(),
        None => {
            let body = get_small(http, &base, "application/vnd.github+json").await?;
            let v: serde_json::Value = serde_json::from_slice(&body)
                .map_err(|_| "the repository reply is not valid".to_string())?;
            let branch = v["default_branch"]
                .as_str()
                .ok_or("the repository has no default branch")?;
            if !safe_segment(branch) {
                return Err("the default branch name is not supported".to_string());
            }
            branch.to_string()
        }
    };
    let body = get_small(
        http,
        &format!("{base}/commits/{git_ref}"),
        "application/vnd.github.sha",
    )
    .await?;
    let sha = String::from_utf8(body)
        .map_err(|_| "the commit reply is not text".to_string())?
        .trim()
        .to_ascii_lowercase();
    if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(sha)
    } else {
        Err("the commit reply is not a SHA".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(owner: &str, repo: &str, r: Option<&str>, sub: Option<&str>) -> GithubLink {
        GithubLink {
            owner: owner.into(),
            repo: repo.into(),
            git_ref: r.map(str::to_string),
            sub_path: sub.map(str::to_string),
        }
    }

    #[test]
    fn accepts_repository_tree_and_sub_path_links() {
        let cases = [
            ("https://github.com/o/r", link("o", "r", None, None)),
            ("https://github.com/o/r/", link("o", "r", None, None)),
            ("https://github.com/o/r.git", link("o", "r", None, None)),
            ("https://github.com/o/r.git/", link("o", "r", None, None)),
            (
                "https://github.com/Some-Org/my.repo_1/tree/v1.2",
                link("Some-Org", "my.repo_1", Some("v1.2"), None),
            ),
            (
                "https://github.com/o/r/tree/main/skills/x",
                link("o", "r", Some("main"), Some("skills/x")),
            ),
            (
                "https://github.com/o/r.git/tree/main",
                link("o", "r", Some("main"), None),
            ),
            (
                "https://github.com/o/r.git/tree/main/a",
                link("o", "r", Some("main"), Some("a")),
            ),
            (
                "https://github.com/o/r/tree/main/skills/x/",
                link("o", "r", Some("main"), Some("skills/x")),
            ),
        ];
        for (url, want) in cases {
            assert_eq!(parse_github_url(url), Some(want), "{url}");
        }
    }

    #[test]
    fn refuses_everything_else() {
        for url in [
            "",
            "github.com/o/r",
            "http://github.com/o/r",
            "https://gitlab.com/o/r",
            "https://www.github.com/o/r",
            "https://github.com.evil.com/o/r",
            "https://user@github.com/o/r",
            "https://user:pw@github.com/o/r",
            "https://github.com:443/o/r",
            "https://github.com/o",
            "https://github.com/o/",
            "https://github.com//r",
            "https://github.com/o/r?tab=readme",
            "https://github.com/o/r#readme",
            "https://github.com/o/r/tree/main?x=1",
            "https://github.com/o/r/tree/main/a#b",
            "https://github.com/o/r/blob/main/SKILL.md",
            "https://github.com/o/r/tree",
            "https://github.com/o/r/tree/",
            "https://github.com/o/r/tree/ma%20in",
            "https://github.com/o/r/tree/main/../x",
            "https://github.com/o/r/tree/main/a/./b",
            "https://github.com/o/r/tree/main/a//b",
            "https://github.com/o/r/tree/main/c:/x",
            "https://github.com/o/r/tree/main/a\\b",
            "https://github.com/../r",
            "https://github.com/o/..",
            "https://github.com/o/.git",
            "https://github.com/o w/r",
            "https://github.com/o/r/extra",
        ] {
            assert_eq!(parse_github_url(url), None, "{url}");
        }
    }
}
