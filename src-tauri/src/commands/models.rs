//! The model list, cached on disk.

use super::auth::note;
use crate::engine::UiError;
use crate::openrouter::models::{self, ModelCache, ModelInfo};
use crate::state::AppState;
use std::path::Path;
use std::time::Duration;

const CACHE_FOR: Duration = Duration::from_secs(10 * 60);

fn is_fresh(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < CACHE_FOR)
}

/// A cache younger than 10 minutes is used unless `force`. When fetching
/// fails, any cached list is returned, however old.
pub async fn list_models(s: &AppState, force: bool) -> Result<Vec<ModelInfo>, UiError> {
    let cache = s.model_cache_file();
    if !force
        && is_fresh(&cache)
        && let Some(cached) = ModelCache::load(&cache)
    {
        return Ok(cached);
    }
    match models::list_models(&s.api_http, &s.api_base()).await {
        Ok(list) => {
            let _ = ModelCache::save(&cache, &list);
            Ok(list)
        }
        Err(e) => {
            let e = note(s, e.into());
            ModelCache::load(&cache).ok_or(e)
        }
    }
}
