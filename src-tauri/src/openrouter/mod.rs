pub mod error;
pub mod key_info;
pub mod models;

/// Join an API path onto a base URL, with or without a trailing slash.
pub(crate) fn endpoint(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// Send a GET and return the body of a successful response, or the mapped error.
pub(crate) async fn get_text(request: reqwest::RequestBuilder) -> Result<String, error::ApiError> {
    let resp = request
        .send()
        .await
        .map_err(|e| error::ApiError::Network(e.to_string()))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| error::ApiError::Network(e.to_string()))?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(error::ApiError::from_response(status.as_u16(), &body))
    }
}
