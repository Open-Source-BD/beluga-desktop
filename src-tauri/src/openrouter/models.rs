use super::error::ApiError;
use super::{endpoint, get_text};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub prompt_usd_per_mtok: f64,
    pub completion_usd_per_mtok: f64,
    pub is_free: bool,
    pub supports_tools: bool,
    pub accepts_images: bool,
    pub context_length: u64,
}

#[derive(Deserialize)]
struct RawModel {
    id: String,
    name: String,
    #[serde(default)]
    context_length: Option<u64>,
    pricing: RawPricing,
    #[serde(default)]
    architecture: RawArchitecture,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

#[derive(Deserialize)]
struct RawPricing {
    prompt: String,
    completion: String,
}

#[derive(Deserialize, Default)]
struct RawArchitecture {
    #[serde(default)]
    input_modalities: Vec<String>,
}

/// USD per token (a decimal string) to USD per million tokens. `None` for
/// anything that is not a finite, non-negative number (e.g. the `-1` that
/// marks variable pricing).
fn per_mtok(price: &str) -> Option<f64> {
    let per_token: f64 = price.trim().parse().ok()?;
    if !per_token.is_finite() || per_token < 0.0 {
        return None;
    }
    // Round away binary noise: 0.0000005 per token is exactly 0.5 per million.
    Some((per_token * 1e12).round() / 1e6)
}

fn convert(raw: RawModel) -> Option<ModelInfo> {
    let prompt = per_mtok(&raw.pricing.prompt)?;
    let completion = per_mtok(&raw.pricing.completion)?;
    Some(ModelInfo {
        id: raw.id,
        name: raw.name,
        prompt_usd_per_mtok: prompt,
        completion_usd_per_mtok: completion,
        is_free: prompt == 0.0 && completion == 0.0,
        supports_tools: raw.supported_parameters.iter().any(|p| p == "tools"),
        accepts_images: raw
            .architecture
            .input_modalities
            .iter()
            .any(|m| m == "image"),
        context_length: raw.context_length.unwrap_or(0),
    })
}

/// Parse `{"data":[...]}`. An entry that cannot be parsed is skipped rather
/// than failing the whole list.
pub(crate) fn parse_models(body: &str) -> Result<Vec<ModelInfo>, ApiError> {
    #[derive(Deserialize)]
    struct List {
        data: Vec<serde_json::Value>,
    }
    let list: List = serde_json::from_str(body).map_err(|_| ApiError::Other {
        status: 200,
        message: "unexpected model list response".to_string(),
    })?;
    Ok(list
        .data
        .into_iter()
        .filter_map(|v| serde_json::from_value::<RawModel>(v).ok())
        .filter_map(convert)
        .collect())
}

pub async fn list_models(
    http: &reqwest::Client,
    base_url: &str,
) -> Result<Vec<ModelInfo>, ApiError> {
    let body = get_text(http.get(endpoint(base_url, "models"))).await?;
    parse_models(&body)
}

/// On-disk copy of the last model list, so the picker works offline.
pub struct ModelCache;

impl ModelCache {
    /// `None` when the file is missing or unreadable as a model list.
    pub fn load(path: &Path) -> Option<Vec<ModelInfo>> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(path: &Path, models: &[ModelInfo]) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string(models).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const FIXTURE: &str = include_str!("../../tests/fixtures/models.json");

    #[test]
    fn parses_fixture() {
        let models = parse_models(FIXTURE).unwrap();
        assert_eq!(models.len(), 6);
        for m in models.iter().filter(|m| m.id.ends_with(":free")) {
            assert!(m.is_free);
            assert_eq!(m.prompt_usd_per_mtok, 0.0);
            assert_eq!(m.completion_usd_per_mtok, 0.0);
        }
        assert!(
            models
                .iter()
                .filter(|m| !m.id.ends_with(":free"))
                .all(|m| !m.is_free)
        );
    }

    #[test]
    fn price_is_per_million() {
        let body = r#"{"data":[{"id":"a/b","name":"B","context_length":1000,
            "pricing":{"prompt":"0.0000005","completion":"0.000002"},
            "architecture":{"input_modalities":["text"]},"supported_parameters":[]}]}"#;
        let m = &parse_models(body).unwrap()[0];
        assert_eq!(m.prompt_usd_per_mtok, 0.5);
        assert_eq!(m.completion_usd_per_mtok, 2.0);
        assert!(!m.is_free);
        assert_eq!(m.context_length, 1000);
    }

    #[test]
    fn supports_tools_from_supported_parameters() {
        let models = parse_models(FIXTURE).unwrap();
        let tools: Vec<_> = models.iter().filter(|m| m.supports_tools).collect();
        assert_eq!(tools.len(), 4);
        assert!(
            !models
                .iter()
                .find(|m| m.id == "example/no-tools:free")
                .unwrap()
                .supports_tools
        );
    }

    #[test]
    fn accepts_images_from_architecture_input_modalities() {
        let models = parse_models(FIXTURE).unwrap();
        let images: Vec<_> = models
            .iter()
            .filter(|m| m.accepts_images)
            .map(|m| m.id.as_str())
            .collect();
        assert_eq!(
            images,
            ["google/gemini-4-carbon", "google/gemini-omni-1.2-flash"]
        );
    }

    #[test]
    fn skips_an_entry_with_missing_pricing_instead_of_failing_the_whole_list() {
        let body = r#"{"data":[
            {"id":"bad/one","name":"Bad","context_length":1,"architecture":{"input_modalities":["text"]}},
            {"id":"good/one","name":"Good","context_length":1,"pricing":{"prompt":"0","completion":"0"},
             "architecture":{"input_modalities":["text"]},"supported_parameters":["tools"]}]}"#;
        let models = parse_models(body).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "good/one");
    }

    #[test]
    fn cache_roundtrip() {
        let dir = std::env::temp_dir().join(format!("beluga-cache-{}-rt", std::process::id()));
        let p = dir.join("nested").join("models.json");
        let models = parse_models(FIXTURE).unwrap();
        ModelCache::save(&p, &models).unwrap();
        assert_eq!(ModelCache::load(&p), Some(models));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_load_returns_none_on_corrupt_file() {
        let dir = std::env::temp_dir().join(format!("beluga-cache-{}-bad", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("models.json");
        std::fs::write(&p, "{not json").unwrap();
        assert_eq!(ModelCache::load(&p), None);
        assert_eq!(ModelCache::load(&dir.join("missing.json")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn list_models_fetches_and_accepts_trailing_slash() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string(FIXTURE))
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        for base in [
            format!("{}/api/v1", server.uri()),
            format!("{}/api/v1/", server.uri()),
        ] {
            assert_eq!(list_models(&http, &base).await.unwrap().len(), 6);
        }
    }

    #[tokio::test]
    async fn list_models_maps_http_errors() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;
        let r = list_models(&reqwest::Client::new(), &server.uri()).await;
        assert!(matches!(r, Err(ApiError::RateLimited { .. })));
    }
}
