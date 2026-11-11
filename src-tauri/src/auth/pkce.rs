use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::openrouter::error::ApiError;

const VERIFIER_LEN: usize = 64;
const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn generate() -> Pkce {
    let mut rng = rand::rng();
    let verifier: String = (0..VERIFIER_LEN)
        .map(|_| ALPHABET[rng.random_range(0..ALPHABET.len())] as char)
        .collect();
    let challenge = challenge_for(&verifier);
    Pkce {
        verifier,
        challenge,
    }
}

pub fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub fn auth_url(challenge: &str, callback_url: Option<&str>, state: &str) -> String {
    let mut params = vec![
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    if let Some(cb) = callback_url {
        params.push(("callback_url", cb));
    }
    reqwest::Url::parse_with_params("https://openrouter.ai/auth", params)
        .expect("static base URL is valid")
        .into()
}

pub async fn exchange_code(
    http: &reqwest::Client,
    base_url: &str,
    code: &str,
    verifier: &str,
) -> Result<String, ApiError> {
    let resp = http
        .post(format!("{base_url}/api/v1/auth/keys"))
        .json(&serde_json::json!({
            "code": code,
            "code_verifier": verifier,
            "code_challenge_method": "S256",
        }))
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    if !status.is_success() {
        return Err(ApiError::Other {
            status: status.as_u16(),
            message: body,
        });
    }
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("key")?.as_str().map(str::to_owned))
        .ok_or(ApiError::Other {
            status: status.as_u16(),
            message: "response had no key".into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn rfc7636_vector() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_is_64_urlsafe_chars_and_unique() {
        let a = generate();
        let b = generate();
        assert_eq!(a.verifier.len(), 64);
        assert!(
            a.verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(a.verifier, b.verifier);
        assert_eq!(a.challenge, challenge_for(&a.verifier));
    }

    #[test]
    fn auth_url_with_callback() {
        let u = auth_url("CH", Some("http://localhost:5123/callback"), "ST");
        assert!(u.starts_with("https://openrouter.ai/auth?"));
        assert!(u.contains("callback_url=http%3A%2F%2Flocalhost%3A5123%2Fcallback"));
        assert!(
            u.contains("code_challenge=CH")
                && u.contains("code_challenge_method=S256")
                && u.contains("state=ST")
        );
    }

    #[test]
    fn auth_url_without_callback_omits_it() {
        assert!(!auth_url("CH", None, "ST").contains("callback_url"));
    }

    #[tokio::test]
    async fn exchange_posts_code_and_returns_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/keys"))
            .and(body_json(serde_json::json!({
                "code": "c1", "code_verifier": "v1", "code_challenge_method": "S256"
            })))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"key": "sk-or-test"})),
            )
            .mount(&server)
            .await;
        let key = exchange_code(&reqwest::Client::new(), &server.uri(), "c1", "v1").await;
        assert_eq!(key, Ok("sk-or-test".to_string()));
    }

    #[tokio::test]
    async fn exchange_maps_403_to_other_and_never_panics_on_html_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/auth/keys"))
            .respond_with(ResponseTemplate::new(403).set_body_string("<html>Forbidden</html>"))
            .mount(&server)
            .await;
        let r = exchange_code(&reqwest::Client::new(), &server.uri(), "c", "v").await;
        assert!(matches!(r, Err(ApiError::Other { status: 403, .. })));
    }
}
