use super::error::ApiError;
use super::{endpoint, get_text};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FreeDaily {
    pub used: u32,
    pub limit: u32,
    pub remaining: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyStatus {
    pub is_free_tier: bool,
    pub free_daily: Option<FreeDaily>,
    pub usage_usd: f64,
    pub limit_remaining_usd: Option<f64>,
}

// Wire shape of `GET /api/v1/key`: everything is under `data`.
#[derive(Deserialize)]
struct Envelope {
    data: KeyData,
}

#[derive(Deserialize)]
struct KeyData {
    #[serde(default)]
    is_free_tier: bool,
    free_model_daily_requests: Option<Daily>,
    #[serde(default)]
    usage: f64,
    limit_remaining: Option<f64>,
}

#[derive(Deserialize)]
struct Daily {
    used: u32,
    limit: u32,
    remaining: u32,
}

pub(crate) fn parse_key_status(body: &str) -> Result<KeyStatus, ApiError> {
    let env: Envelope = serde_json::from_str(body).map_err(|_| ApiError::Other {
        status: 200,
        message: "unexpected key status response".to_string(),
    })?;
    let d = env.data;
    Ok(KeyStatus {
        is_free_tier: d.is_free_tier,
        free_daily: d.free_model_daily_requests.map(|f| FreeDaily {
            used: f.used,
            limit: f.limit,
            remaining: f.remaining,
        }),
        usage_usd: d.usage,
        limit_remaining_usd: d.limit_remaining,
    })
}

pub async fn key_status(
    http: &reqwest::Client,
    base_url: &str,
    key: &str,
) -> Result<KeyStatus, ApiError> {
    let body = get_text(http.get(endpoint(base_url, "key")).bearer_auth(key)).await?;
    parse_key_status(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const FREE: &str = include_str!("../../tests/fixtures/key_free.json");
    const PAID: &str = include_str!("../../tests/fixtures/key_paid.json");

    #[test]
    fn free_tier_fixture() {
        let s = parse_key_status(FREE).unwrap();
        assert!(s.is_free_tier);
        assert_eq!(
            s.free_daily,
            Some(FreeDaily {
                used: 12,
                limit: 50,
                remaining: 38
            })
        );
    }

    #[test]
    fn paid_fixture() {
        let s = parse_key_status(PAID).unwrap();
        assert!(!s.is_free_tier);
        assert_eq!(s.limit_remaining_usd, Some(4.2));
        assert_eq!(s.usage_usd, 0.8);
    }

    #[tokio::test]
    async fn key_status_sends_bearer_and_parses() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/key"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_string(FREE))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        for base in [
            format!("{}/api/v1", server.uri()),
            format!("{}/api/v1/", server.uri()),
        ] {
            assert!(
                key_status(&http, &base, "sk-test")
                    .await
                    .unwrap()
                    .is_free_tier
            );
        }
    }

    #[tokio::test]
    async fn key_status_maps_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401).set_body_string("{}"))
            .mount(&server)
            .await;
        let r = key_status(&reqwest::Client::new(), &server.uri(), "bad").await;
        assert_eq!(r, Err(ApiError::Unauthorized));
    }
}
